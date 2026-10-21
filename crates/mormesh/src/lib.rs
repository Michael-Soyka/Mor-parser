//! Parser for Pathologic (Mor. Utopia) `.mesh` geometry.
//!
//! The engine picks the layout from the kind of object it loads; [`Mesh::parse`] tells them
//! apart by the leading zero and by which layout consumes the file exactly.
//!
//! Layout, all little-endian:
//!
//! ```text
//! static   +-------+------------------------------+------+
//!          | u32 0 | surface list, with lightmaps | tail |
//!          +-------+------------------------------+------+
//! movable  +---------------------------------+-----+
//!          | surface list, without lightmaps | obb |
//!          +---------------------------------+-----+
//! land     u32 tiles, tiles * { f32x2 center, f32x2 half_extents, surface list, with lightmaps }
//!          center and half extents of the tile's footprint in the XZ plane
//! raw      u32 n, n * { f32x3 position, f32x2 uv }, u32 t, t * u16x3, texture,
//!          f32 radius, f32x2 y_range, f32x3 center, f32x3 half_extents
//! water    u32 n, n * f32x3 position, u32 t, t * u16x3, obb, u8 kind
//!
//! surface list: u32 count, count * header, then count * geometry in the same order
//!
//! header: u32 group, u32 type, then by type ("colors" is a u8 bool)
//!   0   Color            colors, f32x3, f32x3
//!   1   Diffuse          colors, texture, u8 alpha_test
//!   3   DiffuseBump      texture, u8 alpha_test, texture bump
//!   5   DiffuseSpecular  texture, u8 alpha_test, f32x3 specular, f32 power
//!   17  DiffuseOpacity   colors, texture
//!   33  Unlit            texture, u8 alpha_test
//!   64  Special          texture
//! then u32 vertex_count, u32 triangle_count, sphere, obb,
//! and for types 0, 1, 3, 5, 17 of static and land meshes the lightmaps:
//!   u32 count, count * { u32 id, obb, u32 pages, pages * page }
//!   page: u32 width, u32 height, 2 * { u32 n, n B image }, u32 vertex_count, u32 triangle_count,
//!         [triangle_count * u16x3, triangle_count * u16 source triangle: not for type 3], f32 param
//!
//! geometry
//!   u8 compressed, [f32x3 origin if compressed]
//!   vertex_count * vertex
//!     position   compressed: i16x3, origin + v / 10; else f32x3
//!     normal     u32 packed, not for types 3, 33
//!     uv         f32x2, not for type 0
//!     type 3:    f32x2 bump uv, f32x3 tangent, f32x3 binormal
//!     colors:    u32 0xAARRGGBB
//!   type 3: vertex_count * u32 packed normal
//!   triangle_count * u16x3
//!   per lightmap page: vertex_count * { u16 source vertex, [40 B copy: type 3], f32x2 uv, [u32 copy: colors] }
//!                      type 3: then vertex_count * u32 copy, triangle_count * u16x3
//!
//! tail: sphere, 3 * obb, u32 hulls, hulls * hull, u8 flag
//!   hull: u32 n, n * f32x3 point, u32 m, m * f32x4 plane, u32 k, k * { u32x3 points, u32 plane }, sphere, obb
//!
//! texture   u8 length, bit 7 set: second byte holds length bits 7..14; cp1251
//! sphere    f32x3 center, f32 radius
//! obb       f32x3 center, f32x3 * 3 axes, f32x3 half extents along them
//! packed    10-bit signed x | y << 10 | z << 20, each / 511
//! ```
//!
//! Lightmap vertices repeat every attribute of their source vertex except the lightmap uv;
//! only the source index and the uv are kept.

use encoding_rs::WINDOWS_1251;

pub const EXTENSION: &str = "mesh";

const STATIC_MARKER: u32 = 0;

const COLOR:            u32 = 0;
const DIFFUSE:          u32 = 1;
const DIFFUSE_BUMP:     u32 = 3;
const DIFFUSE_SPECULAR: u32 = 5;
const DIFFUSE_OPACITY:  u32 = 17;
const UNLIT:            u32 = 33;
const SPECIAL:          u32 = 64;

const SHORT_LENGTH_BITS: u32 = 7;
const LONG_LENGTH_FLAG:  u8  = 1 << SHORT_LENGTH_BITS;

const NORMAL_BITS: u32 = 10;
const NORMAL_MAX:  f32 = 511.0;

const COMPRESSED_POSITION_SCALE: f32 = 10.0;

const COUNT_SIZE:  usize = size_of::<u32>();
const INDEX_SIZE:  usize = size_of::<u16>();
const PACKED_SIZE: usize = size_of::<u32>();
const UV_SIZE:     usize = size_of::<[f32; 2]>();
const VEC3_SIZE:   usize = size_of::<[f32; 3]>();
const PLANE_SIZE:  usize = size_of::<[f32; 4]>();

const SPHERE_SIZE:              usize = VEC3_SIZE + size_of::<f32>();
const OBB_SIZE:                 usize = VEC3_SIZE + ( 3 * VEC3_SIZE ) + VEC3_SIZE;
const TRIANGLE_SIZE:            usize = size_of::<[u16; 3]>();
const COMPRESSED_POSITION_SIZE: usize = size_of::<[i16; 3]>();
const LIGHTMAP_VERTEX_SIZE:     usize = INDEX_SIZE + UV_SIZE;
const RAW_VERTEX_SIZE:          usize = VEC3_SIZE + UV_SIZE;
const HULL_FACE_SIZE:           usize = size_of::<[u32; 3]>() + size_of::<u32>();

/// Bump attributes repeated in each bump lightmap vertex: uv, bump uv, tangent, binormal.
const BUMP_COPY_SIZE: usize = UV_SIZE + UV_SIZE + VEC3_SIZE + VEC3_SIZE;

const HEADER_MIN_SIZE: usize = size_of::<[u32; 2]>() + ( 2 * COUNT_SIZE ) + SPHERE_SIZE + OBB_SIZE;
const PAGE_MIN_SIZE:   usize = size_of::<[u32; 2]>() + ( 4 * COUNT_SIZE ) + size_of::<f32>();
const HULL_MIN_SIZE:   usize = ( 3 * COUNT_SIZE ) + SPHERE_SIZE + OBB_SIZE;

const LAND_TILE_MIN_SIZE: usize = size_of::<[f32; 2]>() + size_of::<[f32; 2]>() + COUNT_SIZE;

#[ derive( Debug, thiserror::Error ) ]
pub enum Error
{
    #[ error( "mesh data is truncated" ) ]
    Truncated,

    #[ error( "{0} trailing bytes after mesh data" ) ]
    TrailingData( usize ),

    #[ error( "unsupported surface type {0}" ) ]
    UnsupportedSurface( u32 ),

    #[ error( "{what} index {index} out of range for {count} items" ) ]
    IndexOutOfRange { what: &'static str, index: u32, count: usize },

    #[ error( "not a static, movable, land, water or raw mesh" ) ]
    UnknownLayout,

    #[ error( "mesh is too large" ) ]
    TooLarge,
}

pub type Result<T> = std::result::Result<T, Error>;

#[ derive( Debug, Clone, Copy, PartialEq ) ]
pub struct Sphere
{
    pub center: [f32; 3],
    pub radius: f32,
}

/// Oriented box; `half_extents[i]` runs along `axes[i]`.
#[ derive( Debug, Clone, Copy, PartialEq ) ]
pub struct Obb
{
    pub center:       [f32; 3],
    pub axes:         [[f32; 3]; 3],
    pub half_extents: [f32; 3],
}

#[ derive( Debug, Clone, PartialEq ) ]
pub enum Material
{
    /// Untextured; the engine extends both colors with alpha 1.
    Color { colors: [[f32; 3]; 2] },

    /// `alpha_test` is set exactly for textures with `#` in the name.
    Diffuse { texture: String, alpha_test: bool },

    DiffuseBump { texture: String, alpha_test: bool, bump: String },
    DiffuseSpecular { texture: String, alpha_test: bool, specular: [f32; 3], power: f32 },
    DiffuseOpacity { texture: String },

    /// No normals.
    Unlit { texture: String, alpha_test: bool },

    /// Drawn together with the engine's `special1.tex`.
    Special { texture: String },
}

impl Material
{
    pub fn texture( &self ) -> Option<&str>
    {
        match self
        {
            Self::Color { .. } => None,
            Self::Diffuse { texture, .. }
            | Self::DiffuseBump { texture, .. }
            | Self::DiffuseSpecular { texture, .. }
            | Self::DiffuseOpacity { texture }
            | Self::Unlit { texture, .. }
            | Self::Special { texture } => Some( texture ),
        }
    }
}

/// Lightmap vertex: surface vertex `source` with its own lightmap uv.
#[ derive( Debug, Clone, Copy, PartialEq ) ]
pub struct LightmapVertex
{
    pub source: u16,
    pub uv:     [f32; 2],
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct LightmapPage
{
    pub width:  u32,
    pub height: u32,

    /// Two images in the engine's own compressed format.
    pub images: [Vec<u8>; 2],

    pub vertices: Vec<LightmapVertex>,

    /// Indices into `vertices`.
    pub triangles: Vec<[u16; 3]>,

    /// Surface triangle behind each of `triangles`; empty for [`Material::DiffuseBump`].
    pub source_triangles: Vec<u16>,

    pub param: f32,
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct Lightmap
{
    pub id:    u32,
    pub obb:   Obb,
    pub pages: Vec<LightmapPage>,
}

/// One material with its own vertex and index buffers. Attribute vectors are either empty
/// or as long as `positions`.
#[ derive( Debug, Clone, PartialEq ) ]
pub struct Surface
{
    pub group:     u32,
    pub material:  Material,
    pub sphere:    Sphere,
    pub obb:       Obb,
    pub positions: Vec<[f32; 3]>,

    /// Empty for [`Material::Unlit`].
    pub normals: Vec<[f32; 3]>,

    /// Empty for [`Material::Color`].
    pub uvs: Vec<[f32; 2]>,

    /// RGBA, on surfaces stored with per-vertex color.
    pub colors: Vec<[u8; 4]>,

    /// [`Material::DiffuseBump`] only.
    pub bump_uvs:  Vec<[f32; 2]>,
    pub tangents:  Vec<[f32; 3]>,
    pub binormals: Vec<[f32; 3]>,
    pub triangles: Vec<[u16; 3]>,
    pub lightmaps: Vec<Lightmap>,
}

#[ derive( Debug, Clone, Copy, PartialEq, Eq ) ]
pub struct HullFace
{
    pub points: [u32; 3],
    pub plane:  u32,
}

/// Convex hull stored after the surfaces of a static mesh.
#[ derive( Debug, Clone, PartialEq ) ]
pub struct Hull
{
    pub points: Vec<[f32; 3]>,

    /// Normal and distance: `dot(normal, p) + d = 0`.
    pub planes: Vec<[f32; 4]>,

    pub faces:  Vec<HullFace>,
    pub sphere: Sphere,
    pub obb:    Obb,
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct StaticMesh
{
    pub surfaces: Vec<Surface>,
    pub sphere:   Sphere,
    pub bounds:   [Obb; 3],
    pub hulls:    Vec<Hull>,
    pub flag:     bool,
}

/// Geometry of movable objects (the engine's `CGeometryTrans`).
#[ derive( Debug, Clone, PartialEq ) ]
pub struct MovableMesh
{
    pub surfaces: Vec<Surface>,
    pub obb:      Obb,
}

/// Square of the city terrain; footprint in the XZ plane.
#[ derive( Debug, Clone, PartialEq ) ]
pub struct LandTile
{
    pub center:       [f32; 2],
    pub half_extents: [f32; 2],
    pub surfaces:     Vec<Surface>,
}

/// Terrain split into tiles (the engine's `CGeometryLand`), `terra.mesh` in the game.
#[ derive( Debug, Clone, PartialEq ) ]
pub struct LandMesh
{
    pub tiles: Vec<LandTile>,
}

/// Single textured buffer without normals, used for leaves and water plants.
#[ derive( Debug, Clone, PartialEq ) ]
pub struct RawMesh
{
    pub texture:   String,
    pub positions: Vec<[f32; 3]>,
    pub uvs:       Vec<[f32; 2]>,
    pub triangles: Vec<[u16; 3]>,

    /// Largest distance from the origin in the XZ plane.
    pub radius: f32,

    pub y_range:      [f32; 2],
    pub center:       [f32; 3],
    pub half_extents: [f32; 3],
}

/// Water surface without uvs or a texture: the river and the pond.
#[ derive( Debug, Clone, PartialEq ) ]
pub struct WaterMesh
{
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u16; 3]>,
    pub obb:       Obb,

    /// 1 for the pond, 2 for the river; probably picks `CGeometryWater` or `CGeometryWater2`.
    pub kind: u8,
}

#[ derive( Debug, Clone, PartialEq ) ]
pub enum Mesh
{
    Static( StaticMesh ),
    Movable( MovableMesh ),
    Land( LandMesh ),
    Water( WaterMesh ),
    Raw( RawMesh ),
}

fn to_usize( value: u32 ) -> Result<usize>
{
    usize::try_from( value ).map_err( |_| Error::TooLarge )
}

fn check_index( what: &'static str, index: u32, count: usize ) -> Result<()>
{
    if to_usize( index )? < count
    {
        return Ok( () );
    }
    Err( Error::IndexOutOfRange { what, index, count } )
}

fn sign_extend( value: u32, bits: u32 ) -> i32
{
    let unused_bits = u32::BITS - bits;
    ( value << unused_bits ).cast_signed() >> unused_bits
}

fn unpack_normal( packed: u32 ) -> [f32; 3]
{
    let component = |index: u32|
    {
        let field = packed >> ( index * NORMAL_BITS );
        sign_extend( field, NORMAL_BITS ) as f32 / NORMAL_MAX
    };
    [ component( 0 ), component( 1 ), component( 2 ) ]
}

fn unpack_color( packed: u32 ) -> [u8; 4]
{
    let [ b, g, r, a ] = packed.to_le_bytes();
    [ r, g, b, a ]
}

struct Reader<'a>
{
    data: &'a [u8],
    pos:  usize,
}

impl<'a> Reader<'a>
{
    fn new( data: &'a [u8] ) -> Self
    {
        Self { data, pos: 0 }
    }

    fn bytes( &mut self, len: usize ) -> Result<&'a [u8]>
    {
        let end = self.pos.checked_add( len ).ok_or( Error::TooLarge )?;
        let bytes = self.data.get( self.pos..end ).ok_or( Error::Truncated )?;
        self.pos = end;
        Ok( bytes )
    }

    fn skip( &mut self, len: usize ) -> Result<()>
    {
        self.bytes( len )?;
        Ok( () )
    }

    fn array<const N: usize>( &mut self ) -> Result<[u8; N]>
    {
        let mut array = [ 0; N ];
        array.copy_from_slice( self.bytes( N )? );
        Ok( array )
    }

    fn u8( &mut self ) -> Result<u8>
    {
        Ok( self.bytes( 1 )?[ 0 ] )
    }

    fn bool( &mut self ) -> Result<bool>
    {
        Ok( self.u8()? != 0 )
    }

    fn u16( &mut self ) -> Result<u16>
    {
        Ok( u16::from_le_bytes( self.array()? ) )
    }

    fn i16( &mut self ) -> Result<i16>
    {
        Ok( i16::from_le_bytes( self.array()? ) )
    }

    fn u32( &mut self ) -> Result<u32>
    {
        Ok( u32::from_le_bytes( self.array()? ) )
    }

    fn f32( &mut self ) -> Result<f32>
    {
        Ok( f32::from_le_bytes( self.array()? ) )
    }

    fn floats<const N: usize>( &mut self ) -> Result<[f32; N]>
    {
        let mut floats = [ 0.0; N ];
        for value in &mut floats
        {
            *value = self.f32()?;
        }
        Ok( floats )
    }

    fn count( &mut self ) -> Result<usize>
    {
        to_usize( self.u32()? )
    }

    /// Capacity for `count` items of at least `item_size` bytes, bounded by the remaining data.
    fn capacity( &self, count: usize, item_size: usize ) -> usize
    {
        count.min( self.remaining() / item_size )
    }

    fn items<T>( &mut self, count: usize, item_size: usize, mut read_item: impl FnMut( &mut Self ) -> Result<T> ) -> Result<Vec<T>>
    {
        let mut items = Vec::with_capacity( self.capacity( count, item_size ) );
        for _ in 0..count
        {
            items.push( read_item( self )? );
        }
        Ok( items )
    }

    fn counted_items<T>( &mut self, item_size: usize, read_item: impl FnMut( &mut Self ) -> Result<T> ) -> Result<Vec<T>>
    {
        let count = self.count()?;
        self.items( count, item_size, read_item )
    }

    fn counted_bytes( &mut self ) -> Result<Vec<u8>>
    {
        let len = self.count()?;
        Ok( self.bytes( len )?.to_vec() )
    }

    fn string_length( &mut self ) -> Result<usize>
    {
        let first = self.u8()?;
        if first & LONG_LENGTH_FLAG == 0
        {
            return Ok( usize::from( first ) );
        }

        let low = usize::from( first & !LONG_LENGTH_FLAG );
        let high = usize::from( self.u8()? );
        Ok( low | ( high << SHORT_LENGTH_BITS ) )
    }

    fn string( &mut self ) -> Result<String>
    {
        let len = self.string_length()?;
        let ( text, _ ) = WINDOWS_1251.decode_without_bom_handling( self.bytes( len )? );
        Ok( text.into_owned() )
    }

    fn sphere( &mut self ) -> Result<Sphere>
    {
        Ok( Sphere { center: self.floats()?, radius: self.f32()? } )
    }

    fn obb( &mut self ) -> Result<Obb>
    {
        let obb = Obb
        {
            center:       self.floats()?,
            axes:         [ self.floats()?, self.floats()?, self.floats()? ],
            half_extents: self.floats()?,
        };
        Ok( obb )
    }

    fn index( &mut self, what: &'static str, count: usize ) -> Result<u16>
    {
        let index = self.u16()?;
        check_index( what, index.into(), count )?;
        Ok( index )
    }

    fn triangle( &mut self, vertex_count: usize, what: &'static str ) -> Result<[u16; 3]>
    {
        let triangle = [ self.u16()?, self.u16()?, self.u16()? ];
        for index in triangle
        {
            check_index( what, index.into(), vertex_count )?;
        }
        Ok( triangle )
    }

    fn triangles( &mut self, count: usize, vertex_count: usize, what: &'static str ) -> Result<Vec<[u16; 3]>>
    {
        self.items( count, TRIANGLE_SIZE, |reader| reader.triangle( vertex_count, what ) )
    }

    fn remaining( &self ) -> usize
    {
        self.data.len() - self.pos
    }

    fn finish( &self ) -> Result<()>
    {
        match self.remaining()
        {
            0 => Ok( () ),
            rest => Err( Error::TrailingData( rest ) ),
        }
    }
}

#[ derive( Clone, Copy, PartialEq, Eq ) ]
enum Lightmaps
{
    Included,
    Omitted,
}

#[ derive( Clone, Copy ) ]
struct VertexLayout
{
    has_inline_normal: bool,
    has_uv:            bool,
    is_bump:           bool,
    has_color:         bool,
}

struct PageCounts
{
    vertices:  usize,
    triangles: usize,
}

/// Header fields needed to read the geometry that follows all headers.
struct PendingSurface
{
    surface:        Surface,
    layout:         VertexLayout,
    vertex_count:   usize,
    triangle_count: usize,
    page_counts:    Vec<PageCounts>,
}

fn read_material( reader: &mut Reader, kind: u32 ) -> Result<Material>
{
    let material = match kind
    {
        COLOR => Material::Color { colors: [ reader.floats()?, reader.floats()? ] },
        DIFFUSE => Material::Diffuse { texture: reader.string()?, alpha_test: reader.bool()? },
        DIFFUSE_BUMP => Material::DiffuseBump { texture: reader.string()?, alpha_test: reader.bool()?, bump: reader.string()? },
        DIFFUSE_SPECULAR => Material::DiffuseSpecular
        {
            texture:    reader.string()?,
            alpha_test: reader.bool()?,
            specular:   reader.floats()?,
            power:      reader.f32()?,
        },
        DIFFUSE_OPACITY => Material::DiffuseOpacity { texture: reader.string()? },
        UNLIT => Material::Unlit { texture: reader.string()?, alpha_test: reader.bool()? },
        SPECIAL => Material::Special { texture: reader.string()? },
        other => return Err( Error::UnsupportedSurface( other ) ),
    };
    Ok( material )
}

fn read_lightmap_page( reader: &mut Reader, pending: &PendingSurface ) -> Result<(LightmapPage, PageCounts)>
{
    let width = reader.u32()?;
    let height = reader.u32()?;
    let images = [ reader.counted_bytes()?, reader.counted_bytes()? ];
    let counts = PageCounts { vertices: reader.count()?, triangles: reader.count()? };

    let mut triangles = Vec::new();
    let mut source_triangles = Vec::new();
    if !pending.layout.is_bump
    {
        triangles = reader.triangles( counts.triangles, counts.vertices, "lightmap vertex" )?;
        source_triangles = reader.items( counts.triangles, INDEX_SIZE, |reader| reader.index( "surface triangle", pending.triangle_count ) )?;
    }

    let page = LightmapPage
    {
        width,
        height,
        images,
        vertices: Vec::new(),
        triangles,
        source_triangles,
        param:    reader.f32()?,
    };
    Ok( ( page, counts ) )
}

fn read_lightmaps( reader: &mut Reader, pending: &mut PendingSurface ) -> Result<()>
{
    let lightmap_count = reader.count()?;
    for _ in 0..lightmap_count
    {
        let id = reader.u32()?;
        let obb = reader.obb()?;
        let page_count = reader.count()?;
        let mut pages = Vec::with_capacity( reader.capacity( page_count, PAGE_MIN_SIZE ) );
        for _ in 0..page_count
        {
            let ( page, counts ) = read_lightmap_page( reader, pending )?;
            pending.page_counts.push( counts );
            pages.push( page );
        }
        pending.surface.lightmaps.push( Lightmap { id, obb, pages } );
    }
    Ok( () )
}

fn read_header( reader: &mut Reader, lightmaps: Lightmaps ) -> Result<PendingSurface>
{
    let group = reader.u32()?;
    let kind = reader.u32()?;
    let has_color = match kind
    {
        COLOR | DIFFUSE | DIFFUSE_OPACITY => reader.bool()?,
        _ => false,
    };
    let material = read_material( reader, kind )?;
    let vertex_count = reader.count()?;
    let triangle_count = reader.count()?;
    let surface = Surface
    {
        group,
        material,
        sphere:    reader.sphere()?,
        obb:       reader.obb()?,
        positions: Vec::new(),
        normals:   Vec::new(),
        uvs:       Vec::new(),
        colors:    Vec::new(),
        bump_uvs:  Vec::new(),
        tangents:  Vec::new(),
        binormals: Vec::new(),
        triangles: Vec::new(),
        lightmaps: Vec::new(),
    };

    let layout = VertexLayout
    {
        has_inline_normal: !matches!( kind, DIFFUSE_BUMP | UNLIT ),
        has_uv:            kind != COLOR,
        is_bump:           kind == DIFFUSE_BUMP,
        has_color,
    };

    let mut pending = PendingSurface { surface, layout, vertex_count, triangle_count, page_counts: Vec::new() };
    if lightmaps == Lightmaps::Included && !matches!( kind, UNLIT | SPECIAL )
    {
        read_lightmaps( reader, &mut pending )?;
    }
    Ok( pending )
}

fn read_compressed_position( reader: &mut Reader, origin: [f32; 3] ) -> Result<[f32; 3]>
{
    let mut position = origin;
    for coordinate in &mut position
    {
        *coordinate += f32::from( reader.i16()? ) / COMPRESSED_POSITION_SCALE;
    }
    Ok( position )
}

fn read_vertex( reader: &mut Reader, surface: &mut Surface, layout: VertexLayout, origin: Option<[f32; 3]> ) -> Result<()>
{
    let position = match origin
    {
        Some( origin ) => read_compressed_position( reader, origin )?,
        None => reader.floats()?,
    };
    surface.positions.push( position );

    if layout.has_inline_normal
    {
        surface.normals.push( unpack_normal( reader.u32()? ) );
    }
    if layout.has_uv
    {
        surface.uvs.push( reader.floats()? );
    }
    if layout.is_bump
    {
        surface.bump_uvs.push( reader.floats()? );
        surface.tangents.push( reader.floats()? );
        surface.binormals.push( reader.floats()? );
    }
    if layout.has_color
    {
        surface.colors.push( unpack_color( reader.u32()? ) );
    }
    Ok( () )
}

fn read_lightmap_vertex( reader: &mut Reader, layout: VertexLayout, vertex_count: usize ) -> Result<LightmapVertex>
{
    let source = reader.index( "vertex", vertex_count )?;
    if layout.is_bump
    {
        reader.skip( BUMP_COPY_SIZE )?;
    }
    let uv = reader.floats()?;
    if layout.has_color
    {
        reader.skip( PACKED_SIZE )?;
    }
    Ok( LightmapVertex { source, uv } )
}

fn read_geometry( reader: &mut Reader, pending: PendingSurface ) -> Result<Surface>
{
    let PendingSurface { mut surface, layout, vertex_count, triangle_count, page_counts } = pending;

    let compressed = reader.bool()?;
    let origin = if compressed { Some( reader.floats()? ) } else { None };
    surface.positions.reserve( reader.capacity( vertex_count, COMPRESSED_POSITION_SIZE ) );
    for _ in 0..vertex_count
    {
        read_vertex( reader, &mut surface, layout, origin )?;
    }

    if layout.is_bump
    {
        surface.normals = reader.items( vertex_count, PACKED_SIZE, |reader| reader.u32().map( unpack_normal ) )?;
    }

    surface.triangles = reader.triangles( triangle_count, vertex_count, "vertex" )?;

    let pages = surface.lightmaps.iter_mut().flat_map( |lightmap| &mut lightmap.pages );
    for ( page, counts ) in pages.zip( page_counts )
    {
        page.vertices = reader.items( counts.vertices, LIGHTMAP_VERTEX_SIZE, |reader| read_lightmap_vertex( reader, layout, vertex_count ) )?;
        if layout.is_bump
        {
            let normal_copies_size = counts.vertices.checked_mul( PACKED_SIZE ).ok_or( Error::TooLarge )?;
            reader.skip( normal_copies_size )?;
            page.triangles = reader.triangles( counts.triangles, counts.vertices, "lightmap vertex" )?;
        }
    }

    Ok( surface )
}

fn read_surfaces( reader: &mut Reader, lightmaps: Lightmaps ) -> Result<Vec<Surface>>
{
    let pending = reader.counted_items( HEADER_MIN_SIZE, |reader| read_header( reader, lightmaps ) )?;
    pending.into_iter().map( |surface| read_geometry( reader, surface ) ).collect()
}

fn read_hull_face( reader: &mut Reader, point_count: usize, plane_count: usize ) -> Result<HullFace>
{
    let face = HullFace { points: [ reader.u32()?, reader.u32()?, reader.u32()? ], plane: reader.u32()? };
    for point in face.points
    {
        check_index( "hull point", point, point_count )?;
    }
    check_index( "hull plane", face.plane, plane_count )?;
    Ok( face )
}

fn read_hull( reader: &mut Reader ) -> Result<Hull>
{
    let points = reader.counted_items( VEC3_SIZE, Reader::floats )?;
    let planes = reader.counted_items( PLANE_SIZE, Reader::floats )?;
    let faces = reader.counted_items( HULL_FACE_SIZE, |reader| read_hull_face( reader, points.len(), planes.len() ) )?;
    Ok( Hull { points, planes, faces, sphere: reader.sphere()?, obb: reader.obb()? } )
}

impl StaticMesh
{
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        let mut reader = Reader::new( data );
        if reader.u32()? != STATIC_MARKER
        {
            return Err( Error::UnknownLayout );
        }

        let surfaces = read_surfaces( &mut reader, Lightmaps::Included )?;
        let sphere = reader.sphere()?;
        let bounds = [ reader.obb()?, reader.obb()?, reader.obb()? ];
        let hulls = reader.counted_items( HULL_MIN_SIZE, read_hull )?;
        let flag = reader.bool()?;

        reader.finish()?;
        Ok( Self { surfaces, sphere, bounds, hulls, flag } )
    }
}

impl MovableMesh
{
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        let mut reader = Reader::new( data );
        let surfaces = read_surfaces( &mut reader, Lightmaps::Omitted )?;
        let obb = reader.obb()?;
        reader.finish()?;
        Ok( Self { surfaces, obb } )
    }
}

fn read_land_tile( reader: &mut Reader ) -> Result<LandTile>
{
    let center = reader.floats()?;
    let half_extents = reader.floats()?;
    let surfaces = read_surfaces( reader, Lightmaps::Included )?;
    Ok( LandTile { center, half_extents, surfaces } )
}

impl LandMesh
{
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        let mut reader = Reader::new( data );
        let tiles = reader.counted_items( LAND_TILE_MIN_SIZE, read_land_tile )?;
        reader.finish()?;
        Ok( Self { tiles } )
    }
}

impl RawMesh
{
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        let mut reader = Reader::new( data );
        let vertex_count = reader.count()?;
        let mut positions = Vec::with_capacity( reader.capacity( vertex_count, RAW_VERTEX_SIZE ) );
        let mut uvs = Vec::with_capacity( positions.capacity() );
        for _ in 0..vertex_count
        {
            positions.push( reader.floats()? );
            uvs.push( reader.floats()? );
        }

        let triangle_count = reader.count()?;
        let triangles = reader.triangles( triangle_count, vertex_count, "vertex" )?;
        let mesh = Self
        {
            texture:      reader.string()?,
            positions,
            uvs,
            triangles,
            radius:       reader.f32()?,
            y_range:      reader.floats()?,
            center:       reader.floats()?,
            half_extents: reader.floats()?,
        };

        reader.finish()?;
        Ok( mesh )
    }
}

impl WaterMesh
{
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        let mut reader = Reader::new( data );
        let positions = reader.counted_items( VEC3_SIZE, Reader::floats )?;
        let triangle_count = reader.count()?;
        let triangles = reader.triangles( triangle_count, positions.len(), "vertex" )?;
        let mesh = Self { positions, triangles, obb: reader.obb()?, kind: reader.u8()? };

        reader.finish()?;
        Ok( mesh )
    }
}

impl Mesh
{
    /// The file itself does not name its kind, the engine knows it from where the mesh is used.
    /// Static when the file starts with a zero, otherwise movable, land, water or raw, whichever reads the whole file.
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        if data.starts_with( &STATIC_MARKER.to_le_bytes() )
        {
            return StaticMesh::parse( data ).map( Self::Static );
        }
        if let Ok( mesh ) = MovableMesh::parse( data )
        {
            return Ok( Self::Movable( mesh ) );
        }
        if let Ok( mesh ) = LandMesh::parse( data )
        {
            return Ok( Self::Land( mesh ) );
        }
        if let Ok( mesh ) = WaterMesh::parse( data )
        {
            return Ok( Self::Water( mesh ) );
        }
        RawMesh::parse( data ).map( Self::Raw ).map_err( |_| Error::UnknownLayout )
    }

    /// Every surface, across all land tiles; none for the other kinds, which have their own geometry types.
    pub fn surfaces( &self ) -> impl Iterator<Item = &Surface>
    {
        let ( surfaces, tiles ): ( &[Surface], &[LandTile] ) = match self
        {
            Self::Static( mesh ) => ( &mesh.surfaces, &[] ),
            Self::Movable( mesh ) => ( &mesh.surfaces, &[] ),
            Self::Land( mesh ) => ( &[], &mesh.tiles ),
            Self::Water( _ ) | Self::Raw( _ ) => ( &[], &[] ),
        };
        surfaces.iter().chain( tiles.iter().flat_map( |tile| &tile.surfaces ) )
    }
}

#[ cfg( test ) ]
mod tests
{
    use super::*;

    const NORMAL_MASK: u32 = ( 1 << NORMAL_BITS ) - 1;

    #[ derive( Default ) ]
    struct Writer( Vec<u8> );

    impl Writer
    {
        fn u8( &mut self, value: u8 ) -> &mut Self
        {
            self.0.push( value );
            self
        }

        fn bool( &mut self, value: bool ) -> &mut Self
        {
            self.u8( value.into() )
        }

        fn u16( &mut self, value: u16 ) -> &mut Self
        {
            self.0.extend_from_slice( &value.to_le_bytes() );
            self
        }

        fn i16( &mut self, value: i16 ) -> &mut Self
        {
            self.0.extend_from_slice( &value.to_le_bytes() );
            self
        }

        fn u32( &mut self, value: u32 ) -> &mut Self
        {
            self.0.extend_from_slice( &value.to_le_bytes() );
            self
        }

        fn floats( &mut self, values: &[f32] ) -> &mut Self
        {
            for value in values
            {
                self.0.extend_from_slice( &value.to_le_bytes() );
            }
            self
        }

        fn zeros( &mut self, len: usize ) -> &mut Self
        {
            self.0.resize( self.0.len() + len, 0 );
            self
        }

        fn counted_bytes( &mut self, bytes: &[u8] ) -> &mut Self
        {
            self.u32( u32::try_from( bytes.len() ).unwrap() );
            self.0.extend_from_slice( bytes );
            self
        }

        fn triangle( &mut self, triangle: [u16; 3] ) -> &mut Self
        {
            for index in triangle
            {
                self.u16( index );
            }
            self
        }

        fn string( &mut self, text: &[u8] ) -> &mut Self
        {
            let len = text.len();
            if len < usize::from( LONG_LENGTH_FLAG )
            {
                self.u8( len as u8 );
            }
            else
            {
                self.u8( ( len as u8 ) | LONG_LENGTH_FLAG ).u8( ( len >> SHORT_LENGTH_BITS ) as u8 );
            }
            self.0.extend_from_slice( text );
            self
        }

        fn bounds( &mut self ) -> &mut Self
        {
            self.floats( &[ 1.0, 2.0, 3.0 ] )
                .floats( &[ 4.0 ] )
                .floats( &[ 0.0; 3 ] )
                .floats( &[ 1.0, 0.0, 0.0 ] )
                .floats( &[ 0.0, 1.0, 0.0 ] )
                .floats( &[ 0.0, 0.0, 1.0 ] )
                .floats( &[ 5.0; 3 ] )
        }

        fn obb( &mut self ) -> &mut Self
        {
            self.zeros( OBB_SIZE )
        }

        fn tail( &mut self ) -> &mut Self
        {
            self.zeros( SPHERE_SIZE ).obb().obb().obb().u32( 0 ).bool( true )
        }
    }

    fn pack_normal( [ x, y, z ]: [f32; 3] ) -> u32
    {
        let field = |value: f32| ( ( value * NORMAL_MAX ) as i32 ).cast_unsigned() & NORMAL_MASK;
        field( x ) | ( field( y ) << NORMAL_BITS ) | ( field( z ) << ( 2 * NORMAL_BITS ) )
    }

    /// Static mesh with one compressed Diffuse surface (3 vertices, one `triangle`) and one lightmap page.
    fn diffuse_static_with( triangle: [u16; 3] ) -> Writer
    {
        let mut w = Writer::default();

        w.u32( STATIC_MARKER ).u32( 1 );
        w.u32( 2 ).u32( DIFFUSE ).bool( false ).string( b"wall#_DXT1.tex" ).bool( true ).u32( 3 ).u32( 1 ).bounds();
        w.u32( 1 ).u32( 7 ).obb().u32( 1 );
        w.u32( 64 ).u32( 16 ).counted_bytes( &[ 0xaa, 0xbb ] ).counted_bytes( &[ 0xcc ] ).u32( 2 ).u32( 1 );
        w.triangle( [ 1, 0, 0 ] ).u16( 0 ).floats( &[ 0.5 ] );

        w.bool( true ).floats( &[ 100.0, 200.0, 300.0 ] );
        for ( x, uv ) in [ ( 10i16, 0.0f32 ), ( -20, 0.5 ), ( 0, 1.0 ) ]
        {
            w.i16( x ).i16( 5 ).i16( -5 ).u32( pack_normal( [ 0.0, 1.0, -1.0 ] ) ).floats( &[ uv, 1.0 - uv ] );
        }

        w.triangle( triangle );
        w.u16( 2 ).floats( &[ 0.25, 0.75 ] ).u16( 0 ).floats( &[ 0.0, 1.0 ] );
        w.tail();
        w
    }

    fn diffuse_static() -> Writer
    {
        diffuse_static_with( [ 0, 1, 2 ] )
    }

    #[ test ]
    fn parses_static_diffuse_surface()
    {
        let file = diffuse_static();
        let Mesh::Static( mesh ) = Mesh::parse( &file.0 ).unwrap() else { panic!( "expected static mesh" ) };
        assert!( mesh.flag );
        assert!( mesh.hulls.is_empty() );

        let [ surface ] = &mesh.surfaces[ .. ] else { panic!( "expected one surface" ) };
        assert_eq!( surface.group, 2 );
        assert_eq!( surface.material, Material::Diffuse { texture: "wall#_DXT1.tex".into(), alpha_test: true } );
        assert_eq!( surface.sphere, Sphere { center: [ 1.0, 2.0, 3.0 ], radius: 4.0 } );
        assert_eq!( surface.obb.half_extents, [ 5.0; 3 ] );
        assert_eq!( surface.positions, [ [ 101.0, 200.5, 299.5 ], [ 98.0, 200.5, 299.5 ], [ 100.0, 200.5, 299.5 ] ] );
        assert_eq!( surface.normals[ 0 ], [ 0.0, 1.0, -1.0 ] );
        assert_eq!( surface.uvs[ 1 ], [ 0.5, 0.5 ] );
        assert!( surface.colors.is_empty() && surface.tangents.is_empty() );
        assert_eq!( surface.triangles, [ [ 0, 1, 2 ] ] );

        let [ lightmap ] = &surface.lightmaps[ .. ] else { panic!( "expected one lightmap" ) };
        assert_eq!( lightmap.id, 7 );

        let [ page ] = &lightmap.pages[ .. ] else { panic!( "expected one page" ) };
        assert_eq!( ( page.width, page.height, page.param ), ( 64, 16, 0.5 ) );
        assert_eq!( page.images, [ vec![ 0xaa, 0xbb ], vec![ 0xcc ] ] );
        assert_eq!( page.triangles, [ [ 1, 0, 0 ] ] );
        assert_eq!( page.source_triangles, [ 0 ] );
        assert_eq!(
            page.vertices,
            [ LightmapVertex { source: 2, uv: [ 0.25, 0.75 ] }, LightmapVertex { source: 0, uv: [ 0.0, 1.0 ] } ]
        );
    }

    #[ test ]
    fn parses_uncompressed_color_surface_with_vertex_colors()
    {
        let mut w = Writer::default();

        w.u32( STATIC_MARKER ).u32( 1 );
        w.u32( 0 ).u32( COLOR ).bool( true ).floats( &[ 0.1, 0.2, 0.3 ] ).floats( &[ 0.4, 0.5, 0.6 ] ).u32( 1 ).u32( 0 ).bounds().u32( 0 );
        w.bool( false ).floats( &[ 1.0, 2.0, 3.0 ] ).u32( pack_normal( [ -1.0, 0.0, 0.0 ] ) ).u32( 0x80ff2010 );
        w.tail();

        let mesh = StaticMesh::parse( &w.0 ).unwrap();
        let surface = &mesh.surfaces[ 0 ];
        assert_eq!( surface.material, Material::Color { colors: [ [ 0.1, 0.2, 0.3 ], [ 0.4, 0.5, 0.6 ] ] } );
        assert_eq!( surface.positions, [ [ 1.0, 2.0, 3.0 ] ] );
        assert_eq!( surface.normals, [ [ -1.0, 0.0, 0.0 ] ] );
        assert!( surface.uvs.is_empty() );
        assert_eq!( surface.colors, [ [ 0xff, 0x20, 0x10, 0x80 ] ] );
    }

    #[ test ]
    fn parses_movable_mesh_without_lightmaps()
    {
        let mut w = Writer::default();

        w.u32( 1 ).u32( 0 ).u32( UNLIT ).string( b"bullet.tex" ).bool( false ).u32( 1 ).u32( 0 ).bounds();
        w.bool( false ).floats( &[ 1.0, 2.0, 3.0 ] ).floats( &[ 0.5, 0.25 ] );
        w.obb();

        let Mesh::Movable( mesh ) = Mesh::parse( &w.0 ).unwrap() else { panic!( "expected movable mesh" ) };
        let surface = &mesh.surfaces[ 0 ];
        assert_eq!( surface.material.texture(), Some( "bullet.tex" ) );
        assert!( surface.normals.is_empty() );
        assert_eq!( surface.uvs, [ [ 0.5, 0.25 ] ] );
    }

    #[ test ]
    fn parses_land_tiles()
    {
        let ( no_lightmaps, packed_normal ) = ( 0, 0 );
        let mut w = Writer::default();

        w.u32( 2 );
        w.floats( &[ -100.0, 50.0 ] ).floats( &[ 10.0, 20.0 ] ).u32( 0 );
        w.floats( &[ 100.0, 50.0 ] ).floats( &[ 10.0, 20.0 ] ).u32( 1 );
        w.u32( 0 ).u32( DIFFUSE ).bool( false ).string( b"grass_DXT1.tex" ).bool( false ).u32( 1 ).u32( 0 ).bounds().u32( no_lightmaps );
        w.bool( false ).floats( &[ 95.0, 0.0, 40.0 ] ).u32( packed_normal ).floats( &[ 0.5, 0.5 ] );

        let mesh = Mesh::parse( &w.0 ).unwrap();
        let Mesh::Land( land ) = &mesh else { panic!( "expected land mesh" ) };
        let [ empty, grass ] = &land.tiles[ .. ] else { panic!( "expected two tiles" ) };
        assert!( empty.surfaces.is_empty() );
        assert_eq!( ( grass.center, grass.half_extents ), ( [ 100.0, 50.0 ], [ 10.0, 20.0 ] ) );
        assert_eq!( grass.surfaces[ 0 ].positions, [ [ 95.0, 0.0, 40.0 ] ] );

        let textures: Vec<_> = mesh.surfaces().filter_map( |surface| surface.material.texture() ).collect();
        assert_eq!( textures, [ "grass_DXT1.tex" ] );
    }

    #[ test ]
    fn parses_raw_mesh()
    {
        let mut w = Writer::default();

        w.u32( 3 );
        for ( x, z ) in [ ( 0.0, 0.0 ), ( 3.0, 0.0 ), ( 0.0, 4.0 ) ]
        {
            w.floats( &[ x, 1.0, z ] ).floats( &[ x, z ] );
        }

        w.u32( 1 ).triangle( [ 2, 1, 0 ] ).string( b"leaves#_DXT1.tex" );
        w.floats( &[ 4.0 ] ).floats( &[ 1.0, 1.0 ] ).floats( &[ 1.5, 1.0, 2.0 ] ).floats( &[ 1.5, 0.0, 2.0 ] );

        let Mesh::Raw( mesh ) = Mesh::parse( &w.0 ).unwrap() else { panic!( "expected raw mesh" ) };
        assert_eq!( mesh.texture, "leaves#_DXT1.tex" );
        assert_eq!( mesh.positions[ 2 ], [ 0.0, 1.0, 4.0 ] );
        assert_eq!( mesh.triangles, [ [ 2, 1, 0 ] ] );
        assert_eq!( ( mesh.radius, mesh.y_range ), ( 4.0, [ 1.0, 1.0 ] ) );
    }

    #[ test ]
    fn decodes_long_cp1251_texture_name()
    {
        let cyrillic_a = 0xc0;
        let mut name = vec![ b'a'; 200 ];
        name[ 0 ] = cyrillic_a;

        let mut w = Writer::default();
        w.u32( 1 ).u32( 0 ).u32( SPECIAL ).string( &name ).u32( 0 ).u32( 0 ).bounds().bool( false ).obb();

        let mesh = MovableMesh::parse( &w.0 ).unwrap();
        let texture = mesh.surfaces[ 0 ].material.texture().unwrap();
        assert_eq!( texture.chars().count(), 200 );
        assert!( texture.starts_with( 'А' ) );
    }

    #[ test ]
    fn rejects_out_of_range_triangle()
    {
        let file = diffuse_static_with( [ 3, 1, 2 ] );
        assert!( matches!( StaticMesh::parse( &file.0 ), Err( Error::IndexOutOfRange { what: "vertex", index: 3, count: 3 } ) ) );
    }

    #[ test ]
    fn rejects_unknown_surface_type()
    {
        let mut w = Writer::default();
        w.u32( STATIC_MARKER ).u32( 1 ).u32( 0 ).u32( 2 );
        assert!( matches!( StaticMesh::parse( &w.0 ), Err( Error::UnsupportedSurface( 2 ) ) ) );
    }

    #[ test ]
    fn rejects_trailing_data()
    {
        let mut file = diffuse_static();
        file.u8( 0 );
        assert!( matches!( StaticMesh::parse( &file.0 ), Err( Error::TrailingData( 1 ) ) ) );
    }

    #[ test ]
    fn rejects_truncated_geometry()
    {
        let file = diffuse_static();
        assert!( matches!( StaticMesh::parse( &file.0[ ..file.0.len() - 1 ] ), Err( Error::Truncated ) ) );
        assert!( matches!( Mesh::parse( &1u32.to_le_bytes() ), Err( Error::UnknownLayout ) ) );
    }
}
