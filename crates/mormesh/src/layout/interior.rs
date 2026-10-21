//! Interiors (the engine's `CSceneIndoor`): a BSP tree of the rooms followed by the surfaces.
//!
//! ```text
//! u32 leaf_count, root node
//! node: u8 children
//!   children != 0  f32x4 plane, front node if bit 0, back node if bit 1
//!   children == 0  leaf: u32 index, u32 n, n * { u32 surface, u32 triangle }, f32x3 point,
//!                  u32 m, m B visibility, u32 k, k * u32 static id,
//!                  u32 t, t * f32x3x3 collision triangle, t * u8 collision flag
//! u32 lights, lights * u32 id
//! u32 surfaces, surfaces * surface
//!
//! surface: u32 reserved_triangles, u32 type, texture, u32 triangle_count,
//!          u8 compressed, [f32x3 origin], triangle_count * 3 vertices, no index list
//!            type 1   position, u32 packed normal, f32x2 uv
//!            type 33  position, f32x2 uv
//!          type 1 only, per light: u32 pages, pages * page,
//!            and if any pages: triangle_count * { u32 page or 0xFFFFFFFF if unlit, u32 lightmap triangle }
//! page:    u32 width, u32 height, 2 * { u32 n, n B image }, u32 t,
//!          t * 3 * { u16 source vertex, f32x2 uv copy, f32x2 lightmap uv }, f32 param
//! ```
//!
//! Positions and textures follow the static mesh conventions, see the crate docs.

use crate::{
    COUNT_SIZE, DIFFUSE, Error, INDEX_SIZE, LightmapPage, LightmapVertex, Material, Reader, Result, UNLIT, UV_SIZE, VEC3_SIZE,
    check_index, read_compressed_position, unpack_normal,
};

const LEAF_TRIANGLE_SIZE:       usize = size_of::<[u32; 2]>();
const COLLISION_TRIANGLE_SIZE:  usize = 3 * VEC3_SIZE;
const LIGHTMAP_VERTEX_SIZE:     usize = INDEX_SIZE + UV_SIZE + UV_SIZE;
const PAGE_MIN_SIZE:            usize = size_of::<[u32; 2]>() + ( 3 * COUNT_SIZE ) + size_of::<f32>();
const COMPRESSED_POSITION_SIZE: usize = size_of::<[i16; 3]>();
const SURFACE_MIN_SIZE:         usize = ( 4 * COUNT_SIZE ) + size_of::<u8>();

const FRONT_CHILD: u8 = 1;
const BACK_CHILD:  u8 = 2;

const VERTICES_PER_TRIANGLE: usize = 3;

/// Page of a surface triangle the light does not reach.
const NO_PAGE: u32 = u32::MAX;

#[ derive( Debug, Clone, PartialEq ) ]
pub enum BspNode
{
    Split( BspSplit ),
    Leaf( BspLeaf ),
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct BspSplit
{
    /// Normal and distance: `dot(normal, p) + d = 0`.
    pub plane: [f32; 4],

    pub front: Option<Box<BspNode>>,
    pub back:  Option<Box<BspNode>>,
}

/// Triangle `triangle` of [`InteriorMesh::surfaces`]`[surface]`.
#[ derive( Debug, Clone, Copy, PartialEq, Eq ) ]
pub struct LeafTriangle
{
    pub surface:  u32,
    pub triangle: u32,
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct BspLeaf
{
    pub index: u32,

    /// Surface triangles inside the leaf.
    pub triangles: Vec<LeafTriangle>,

    /// Meaning not established.
    pub point: [f32; 3],

    /// Visibility data, about a bit per leaf; layout not established.
    pub visibility: Vec<u8>,

    /// `StaticID` of the scene actors inside the leaf.
    pub static_ids: Vec<u32>,

    pub collision:       Vec<[[f32; 3]; 3]>,
    pub collision_flags: Vec<u8>,
}

/// Lightmap pages of one light; `source_triangles` of a page map each of its triangles back.
#[ derive( Debug, Clone, PartialEq ) ]
pub struct InteriorLightmap
{
    pub light: u32,
    pub pages: Vec<LightmapPage>,
}

/// Triangle list: every three vertices make a triangle, `triangles` spells that out.
#[ derive( Debug, Clone, PartialEq ) ]
pub struct InteriorSurface
{
    /// Size of the index buffer the engine fills at run time, in triangles.
    pub reserved_triangles: u32,

    pub material:  Material,
    pub positions: Vec<[f32; 3]>,

    /// Empty for [`Material::Unlit`].
    pub normals: Vec<[f32; 3]>,

    pub uvs:       Vec<[f32; 2]>,
    pub triangles: Vec<[u16; 3]>,
    pub lightmaps: Vec<InteriorLightmap>,
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct InteriorMesh
{
    pub leaf_count: u32,
    pub tree:       BspNode,
    pub lights:     Vec<u32>,
    pub surfaces:   Vec<InteriorSurface>,
}

fn read_node( reader: &mut Reader ) -> Result<BspNode>
{
    let children = reader.u8()?;
    if children == 0
    {
        return Ok( BspNode::Leaf( read_leaf( reader )? ) );
    }

    let plane = reader.floats()?;
    let front = if children & FRONT_CHILD != 0 { Some( Box::new( read_node( reader )? ) ) } else { None };
    let back = if children & BACK_CHILD != 0 { Some( Box::new( read_node( reader )? ) ) } else { None };
    Ok( BspNode::Split( BspSplit { plane, front, back } ) )
}

fn read_leaf( reader: &mut Reader ) -> Result<BspLeaf>
{
    let index = reader.u32()?;
    let triangles = reader.counted_items( LEAF_TRIANGLE_SIZE, |reader| Ok( LeafTriangle { surface: reader.u32()?, triangle: reader.u32()? } ) )?;
    let point = reader.floats()?;
    let visibility = reader.counted_bytes()?;
    let static_ids = reader.counted_items( size_of::<u32>(), Reader::u32 )?;

    let collision = reader.counted_items( COLLISION_TRIANGLE_SIZE, |reader| Ok( [ reader.floats()?, reader.floats()?, reader.floats()? ] ) )?;
    let collision_flags = reader.bytes( collision.len() )?.to_vec();

    let leaf = BspLeaf { index, triangles, point, visibility, static_ids, collision, collision_flags };
    Ok( leaf )
}

fn sequential_triangles( triangle_count: usize ) -> Result<Vec<[u16; 3]>>
{
    let vertex_count = triangle_count.checked_mul( VERTICES_PER_TRIANGLE ).ok_or( Error::TooLarge )?;
    let first_vertices = ( 0..vertex_count ).step_by( VERTICES_PER_TRIANGLE );
    first_vertices
        .map( |first| u16::try_from( first ).map_err( |_| Error::TooLarge ).map( |first| [ first, first + 1, first + 2 ] ) )
        .collect()
}

fn read_page( reader: &mut Reader, vertex_count: usize ) -> Result<LightmapPage>
{
    let width = reader.u32()?;
    let height = reader.u32()?;
    let images = [ reader.counted_bytes()?, reader.counted_bytes()? ];

    let triangle_count = reader.count()?;
    let page_vertices = triangle_count.checked_mul( VERTICES_PER_TRIANGLE ).ok_or( Error::TooLarge )?;
    let vertices = reader.items( page_vertices, LIGHTMAP_VERTEX_SIZE, |reader|
    {
        let source = reader.index( "vertex", vertex_count )?;
        reader.skip( UV_SIZE )?;
        Ok( LightmapVertex { source, uv: reader.floats()? } )
    } )?;

    let page = LightmapPage
    {
        width,
        height,
        images,
        vertices,
        triangles:        sequential_triangles( triangle_count )?,
        source_triangles: vec![ 0; triangle_count ],
        param:            reader.f32()?,
    };
    Ok( page )
}

/// Reads the page and page triangle of every surface triangle into the pages' `source_triangles`.
fn read_triangle_lightmaps( reader: &mut Reader, pages: &mut [LightmapPage], triangle_count: usize ) -> Result<()>
{
    for triangle in 0..triangle_count
    {
        let page = reader.u32()?;
        let page_triangle = reader.u32()?;
        if page == NO_PAGE
        {
            continue;
        }
        check_index( "lightmap page", page, pages.len() )?;

        let page = &mut pages[ usize::try_from( page ).map_err( |_| Error::TooLarge )? ];
        check_index( "lightmap triangle", page_triangle, page.source_triangles.len() )?;
        let slot = usize::try_from( page_triangle ).map_err( |_| Error::TooLarge )?;
        page.source_triangles[ slot ] = u16::try_from( triangle ).map_err( |_| Error::TooLarge )?;
    }
    Ok( () )
}

fn read_surface( reader: &mut Reader, lights: &[u32] ) -> Result<InteriorSurface>
{
    let reserved_triangles = reader.u32()?;
    let kind = reader.u32()?;
    let material = match kind
    {
        DIFFUSE => Material::Diffuse { texture: reader.string()?, alpha_test: false },
        UNLIT => Material::Unlit { texture: reader.string()?, alpha_test: false },
        other => return Err( Error::UnsupportedSurface( other ) ),
    };
    let has_normals = kind == DIFFUSE;

    let triangle_count = reader.count()?;
    let triangles = sequential_triangles( triangle_count )?;
    let vertex_count = triangles.len() * VERTICES_PER_TRIANGLE;

    let origin = if reader.bool()? { Some( reader.floats()? ) } else { None };
    let mut positions = Vec::with_capacity( reader.capacity( vertex_count, COMPRESSED_POSITION_SIZE ) );
    let mut normals = Vec::new();
    let mut uvs = Vec::with_capacity( positions.capacity() );
    for _ in 0..vertex_count
    {
        let position = match origin
        {
            Some( origin ) => read_compressed_position( reader, origin )?,
            None => reader.floats()?,
        };
        positions.push( position );
        if has_normals
        {
            normals.push( unpack_normal( reader.u32()? ) );
        }
        uvs.push( reader.floats()? );
    }

    let mut lightmaps = Vec::new();
    if kind == DIFFUSE
    {
        for &light in lights
        {
            let mut pages = reader.counted_items( PAGE_MIN_SIZE, |reader| read_page( reader, vertex_count ) )?;
            if !pages.is_empty()
            {
                read_triangle_lightmaps( reader, &mut pages, triangle_count )?;
            }
            lightmaps.push( InteriorLightmap { light, pages } );
        }
    }

    let surface = InteriorSurface { reserved_triangles, material, positions, normals, uvs, triangles, lightmaps };
    Ok( surface )
}

fn check_leaf_triangles( node: &BspNode, surfaces: &[InteriorSurface] ) -> Result<()>
{
    match node
    {
        BspNode::Split( split ) =>
        {
            for child in [ &split.front, &split.back ].into_iter().flatten()
            {
                check_leaf_triangles( child, surfaces )?;
            }
            Ok( () )
        }
        BspNode::Leaf( leaf ) =>
        {
            for &LeafTriangle { surface, triangle } in &leaf.triangles
            {
                check_index( "surface", surface, surfaces.len() )?;
                let surface = &surfaces[ usize::try_from( surface ).map_err( |_| Error::TooLarge )? ];
                check_index( "surface triangle", triangle, surface.triangles.len() )?;
            }
            Ok( () )
        }
    }
}

impl InteriorMesh
{
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        let mut reader = Reader::new( data );
        let leaf_count = reader.u32()?;
        let tree = read_node( &mut reader )?;

        let lights = reader.counted_items( size_of::<u32>(), Reader::u32 )?;
        let surfaces = reader.counted_items( SURFACE_MIN_SIZE, |reader| read_surface( reader, &lights ) )?;
        reader.finish()?;

        check_leaf_triangles( &tree, &surfaces )?;
        Ok( Self { leaf_count, tree, lights, surfaces } )
    }

    /// Leaves in tree order, front before back.
    pub fn leaves( &self ) -> Vec<&BspLeaf>
    {
        let mut leaves = Vec::new();
        let mut pending = vec![ &self.tree ];
        while let Some( node ) = pending.pop()
        {
            match node
            {
                BspNode::Leaf( leaf ) => leaves.push( leaf ),
                BspNode::Split( split ) =>
                {
                    pending.extend( split.back.as_deref() );
                    pending.extend( split.front.as_deref() );
                }
            }
        }
        leaves
    }
}

#[ cfg( test ) ]
mod tests
{
    use super::*;

    fn push_u32( data: &mut Vec<u8>, value: u32 )
    {
        data.extend_from_slice( &value.to_le_bytes() );
    }

    fn push_floats( data: &mut Vec<u8>, values: &[f32] )
    {
        for value in values
        {
            data.extend_from_slice( &value.to_le_bytes() );
        }
    }

    fn push_leaf( data: &mut Vec<u8>, index: u32, triangle: Option<LeafTriangle> )
    {
        data.push( 0 );
        push_u32( data, index );
        push_u32( data, u32::from( triangle.is_some() ) );
        if let Some( LeafTriangle { surface, triangle } ) = triangle
        {
            push_u32( data, surface );
            push_u32( data, triangle );
        }
        push_floats( data, &[ 0.0; 3 ] );
        push_u32( data, 1 );
        data.push( 0b11 );
        push_u32( data, 1 );
        push_u32( data, 42 );
        push_u32( data, 1 );
        push_floats( data, &[ 0.0; 9 ] );
        data.push( 1 );
    }

    /// Two leaves split by y = 0, one unlit triangle in the back leaf.
    fn interior( triangle_in_leaf: u32 ) -> Vec<u8>
    {
        let mut data = Vec::new();
        push_u32( &mut data, 2 );
        data.push( FRONT_CHILD | BACK_CHILD );
        push_floats( &mut data, &[ 0.0, 1.0, 0.0, 0.0 ] );
        push_leaf( &mut data, 0, None );
        push_leaf( &mut data, 1, Some( LeafTriangle { surface: 0, triangle: triangle_in_leaf } ) );

        push_u32( &mut data, 1 );
        push_u32( &mut data, 7 );
        push_u32( &mut data, 1 );
        push_u32( &mut data, 1 );
        push_u32( &mut data, UNLIT );
        data.push( 8 );
        data.extend_from_slice( b"wall.tex" );
        push_u32( &mut data, 1 );
        data.push( 0 );
        for ( x, u ) in [ ( 0.0, 0.0 ), ( 1.0, 1.0 ), ( 2.0, 0.5 ) ]
        {
            push_floats( &mut data, &[ x, 0.0, -1.0, u, 0.0 ] );
        }
        data
    }

    #[ test ]
    fn parses_tree_and_unlit_surface()
    {
        let mesh = InteriorMesh::parse( &interior( 0 ) ).unwrap();
        assert_eq!( ( mesh.leaf_count, mesh.lights.as_slice() ), ( 2, [ 7 ].as_slice() ) );

        let BspNode::Split( split ) = &mesh.tree else { panic!( "expected a split" ) };
        assert_eq!( split.plane, [ 0.0, 1.0, 0.0, 0.0 ] );

        let leaves = mesh.leaves();
        assert_eq!( leaves.iter().map( |leaf| leaf.index ).collect::<Vec<_>>(), [ 0, 1 ] );
        assert_eq!( ( leaves[ 1 ].static_ids.as_slice(), leaves[ 1 ].visibility.as_slice() ), ( [ 42 ].as_slice(), [ 0b11 ].as_slice() ) );

        let [ surface ] = &mesh.surfaces[ .. ] else { panic!( "expected one surface" ) };
        assert_eq!( surface.material, Material::Unlit { texture: "wall.tex".into(), alpha_test: false } );
        assert_eq!( surface.triangles, [ [ 0, 1, 2 ] ] );
        assert_eq!( surface.positions[ 2 ], [ 2.0, 0.0, -1.0 ] );
        assert!( surface.normals.is_empty() && surface.lightmaps.is_empty() );
    }

    #[ test ]
    fn rejects_leaf_triangle_out_of_range()
    {
        let parsed = InteriorMesh::parse( &interior( 1 ) );
        assert!( matches!( parsed, Err( Error::IndexOutOfRange { what: "surface triangle", index: 1, count: 1 } ) ) );
    }
}
