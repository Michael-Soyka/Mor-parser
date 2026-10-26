//! glTF 2.0 export, as `.glb` or `.gltf` with a separate `.bin`.
//!
//! The game is left-handed with clockwise front faces, glTF is right-handed with
//! counter-clockwise ones: z is negated and every triangle is wound the other way.
//! One primitive per surface, a child node per land tile; lightmaps and bump attributes are not exported.
//! Vegetation is exported by [`Gltf::from_detail`], a child node per instance; characters in the bind pose,
//! without the skeleton and animations. Several meshes and point lights go into one scene through [`SceneBuilder`].

use std::collections::HashMap;

use serde_json::{ Map, Value, json };

use crate::layout::character::CharacterMesh;
use crate::layout::detail::DetailMesh;
use crate::layout::interior::{ InteriorMesh, InteriorSurface };
use crate::{ Error, LandMesh, Material, Mesh, RawMesh, Result, Surface, WaterMesh };

const UNSIGNED_BYTE:  u32 = 5121;
const UNSIGNED_SHORT: u32 = 5123;
const FLOAT:          u32 = 5126;

const ARRAY_BUFFER:         u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

const GLB_MAGIC:       &[u8; 4] = b"glTF";
const GLB_VERSION:     u32      = 2;
const GLB_JSON_CHUNK:  &[u8; 4] = b"JSON";
const GLB_BIN_CHUNK:   &[u8; 4] = b"BIN\0";
const GLB_HEADER_SIZE: usize    = GLB_MAGIC.len() + size_of::<u32>() + size_of::<u32>();
const CHUNK_HEADER:    usize    = size_of::<u32>() + GLB_JSON_CHUNK.len();
const ALIGNMENT:       usize    = 4;

const ALPHA_CUTOFF: f32 = 0.5;
const WATER_COLOR:  [f32; 3] = [ 0.2, 0.3, 0.35 ];
const UNLIT:        &str = "KHR_materials_unlit";
const LIGHTS:       &str = "KHR_lights_punctual";

/// Exported scene: glTF JSON plus the binary buffer it points into.
#[ derive( Debug, Clone ) ]
pub struct Gltf
{
    json: Value,
    bin:  Vec<u8>,
}

impl Gltf
{
    /// `texture_png` returns PNG bytes for a texture name; `None` leaves the material untextured.
    /// Vegetation gives an empty scene, it needs its prototypes, see [`Gltf::from_detail`].
    pub fn from_mesh( mesh: &Mesh, name: &str, texture_png: impl FnMut( &str ) -> Option<Vec<u8>> ) -> Self
    {
        let mut builder = SceneBuilder::new( texture_png );
        builder.add_mesh( mesh, name );
        builder.finish( name )
    }

    /// See [`SceneBuilder::add_detail`].
    pub fn from_detail(
        mesh: &DetailMesh,
        name: &str,
        prototype: impl FnMut( &str ) -> Option<RawMesh>,
        texture_png: impl FnMut( &str ) -> Option<Vec<u8>>,
    ) -> Self
    {
        let mut builder = SceneBuilder::new( texture_png );
        builder.add_detail( mesh, name, prototype );
        builder.finish( name )
    }

    /// Binary buffer of a `.gltf` export, to be saved under the `bin_uri` given to [`Gltf::to_gltf`].
    pub fn bin( &self ) -> &[u8]
    {
        &self.bin
    }

    pub fn to_gltf( &self, bin_uri: &str ) -> String
    {
        let json = self.json_with_buffer( Some( bin_uri ) );
        format!( "{json:#}" )
    }

    pub fn to_glb( &self ) -> Result<Vec<u8>>
    {
        let json = self.json_with_buffer( None ).to_string().into_bytes();
        let json_chunk = padded( json, b' ' );
        let bin_chunk = padded( self.bin.clone(), 0 );

        let mut total_size = GLB_HEADER_SIZE + CHUNK_HEADER + json_chunk.len();
        if !bin_chunk.is_empty()
        {
            total_size += CHUNK_HEADER + bin_chunk.len();
        }

        let mut glb = Vec::with_capacity( total_size );
        glb.extend_from_slice( GLB_MAGIC );
        glb.extend_from_slice( &GLB_VERSION.to_le_bytes() );
        glb.extend_from_slice( &to_u32( total_size )?.to_le_bytes() );
        write_chunk( &mut glb, GLB_JSON_CHUNK, &json_chunk )?;
        if !bin_chunk.is_empty()
        {
            write_chunk( &mut glb, GLB_BIN_CHUNK, &bin_chunk )?;
        }
        Ok( glb )
    }

    fn json_with_buffer( &self, uri: Option<&str> ) -> Value
    {
        let mut json = self.json.clone();
        if self.bin.is_empty()
        {
            return json;
        }

        let mut buffer = json!( { "byteLength": self.bin.len() } );
        if let Some( uri ) = uri
        {
            buffer[ "uri" ] = uri.into();
        }
        json[ "buffers" ] = json!( [ buffer ] );
        json
    }
}

fn to_u32( value: usize ) -> Result<u32>
{
    u32::try_from( value ).map_err( |_| Error::TooLarge )
}

fn padded( mut bytes: Vec<u8>, filler: u8 ) -> Vec<u8>
{
    bytes.resize( bytes.len().next_multiple_of( ALIGNMENT ), filler );
    bytes
}

fn write_chunk( glb: &mut Vec<u8>, kind: &[u8; 4], data: &[u8] ) -> Result<()>
{
    glb.extend_from_slice( &to_u32( data.len() )?.to_le_bytes() );
    glb.extend_from_slice( kind );
    glb.extend_from_slice( data );
    Ok( () )
}

fn to_right_handed( [ x, y, z ]: [f32; 3] ) -> [f32; 3]
{
    [ x, y, -z ]
}

fn counter_clockwise( [ a, b, c ]: [u16; 3] ) -> [u16; 3]
{
    [ a, c, b ]
}

fn float_bytes<const N: usize>( items: &[[f32; N]] ) -> Vec<u8>
{
    items.iter().flatten().flat_map( |value| value.to_le_bytes() ).collect()
}

/// Mask for `#` textures, blend for opacity materials, opaque otherwise.
fn alpha_mode( material: &Material ) -> Option<&'static str>
{
    match material
    {
        Material::DiffuseOpacity { .. } => Some( "BLEND" ),
        Material::Diffuse { alpha_test: true, .. }
        | Material::DiffuseBump { alpha_test: true, .. }
        | Material::DiffuseSpecular { alpha_test: true, .. }
        | Material::Unlit { alpha_test: true, .. } => Some( "MASK" ),
        _ => None,
    }
}

/// What a primitive is built from, whatever the mesh kind.
struct Geometry<'a>
{
    material:     &'a Material,
    positions:    &'a [[f32; 3]],
    normals:      &'a [[f32; 3]],
    uvs:          &'a [[f32; 2]],
    colors:       &'a [[u8; 4]],
    triangles:    &'a [[u16; 3]],
    double_sided: bool,
}

impl<'a> Geometry<'a>
{
    fn from_surface( surface: &'a Surface ) -> Self
    {
        Self
        {
            material:     &surface.material,
            positions:    &surface.positions,
            normals:      &surface.normals,
            uvs:          &surface.uvs,
            colors:       &surface.colors,
            triangles:    &surface.triangles,
            double_sided: false,
        }
    }

    fn from_interior( surface: &'a InteriorSurface ) -> Self
    {
        Self
        {
            material:     &surface.material,
            positions:    &surface.positions,
            normals:      &surface.normals,
            uvs:          &surface.uvs,
            colors:       &[],
            triangles:    &surface.triangles,
            double_sided: false,
        }
    }
}

/// Point light for [`SceneBuilder::add_lights`].
#[ derive( Debug, Clone, PartialEq ) ]
pub struct Light
{
    pub name:     String,
    pub position: [f32; 3],

    /// Linear RGB, 0 to 1.
    pub color: [f32; 3],

    /// Distance where the light ends, positive.
    pub range: f32,
}

/// Meshes collected into one scene, a root node each, placed as stored: the game keeps
/// scene geometry in world coordinates. Textures and vegetation prototypes are shared by all of them.
pub struct SceneBuilder<F>
{
    texture_png: F,
    bin:         Vec<u8>,
    views:       Vec<Value>,
    accessors:   Vec<Value>,
    images:      Vec<Value>,
    textures:    Vec<Value>,
    materials:   Vec<Value>,
    primitives:  Vec<Value>,
    meshes:      Vec<Value>,
    nodes:       Vec<Value>,
    roots:       Vec<usize>,
    lights:      Vec<Value>,
    uses_unlit:  bool,

    /// Texture index by name; `None` when the callback had no image for it.
    texture_by_name: HashMap<String, Option<usize>>,

    /// Mesh index by vegetation prototype name; `None` when the callback had no mesh for it.
    prototype_by_name: HashMap<String, Option<usize>>,
}

impl<F: FnMut( &str ) -> Option<Vec<u8>>> SceneBuilder<F>
{
    /// `texture_png` returns PNG bytes for a texture name; `None` leaves the material untextured.
    pub fn new( texture_png: F ) -> Self
    {
        Self
        {
            texture_png,
            bin:               Vec::new(),
            views:             Vec::new(),
            accessors:         Vec::new(),
            images:            Vec::new(),
            textures:          Vec::new(),
            materials:         Vec::new(),
            primitives:        Vec::new(),
            meshes:            Vec::new(),
            nodes:             Vec::new(),
            roots:             Vec::new(),
            lights:            Vec::new(),
            uses_unlit:        false,
            texture_by_name:   HashMap::new(),
            prototype_by_name: HashMap::new(),
        }
    }

    /// Vegetation is left out, it needs its prototypes, see [`SceneBuilder::add_detail`].
    pub fn add_mesh( &mut self, mesh: &Mesh, name: &str )
    {
        match mesh
        {
            Mesh::Static( mesh ) => self.add_surfaces( name, &mesh.surfaces ),
            Mesh::Movable( mesh ) => self.add_surfaces( name, &mesh.surfaces ),
            Mesh::Land( land ) => self.add_land( name, land ),
            Mesh::Interior( mesh ) => self.add_interior( name, mesh ),
            Mesh::Detail( _ ) => {}
            Mesh::Character( mesh ) => self.add_character( name, mesh ),
            Mesh::Water( mesh ) => self.add_water( name, mesh ),
            Mesh::Raw( mesh ) => self.add_raw( name, mesh ),
        }
    }

    /// One node per instance, sharing a mesh per prototype. `prototype` returns the raw mesh
    /// for a prototype name, see [`DetailMesh::prototypes`]; instances of prototypes it has no mesh for are left out.
    pub fn add_detail( &mut self, mesh: &DetailMesh, name: &str, mut prototype: impl FnMut( &str ) -> Option<RawMesh> )
    {
        let meshes: Vec<Option<usize>> = mesh
            .prototypes
            .iter()
            .map( |prototype_name| self.prototype_mesh( prototype_name, &mut prototype ) )
            .collect();

        let mut instances = Vec::new();
        for instance in mesh.instances()
        {
            let Some( &Some( mesh ) ) = meshes.get( usize::from( instance.prototype ) ) else { continue };
            let [ x, y, z, w ] = instance.rotation();
            let rotation = [ -x, -y, z, w ];
            let node = json!( { "mesh": mesh, "translation": to_right_handed( instance.position ), "rotation": rotation } );
            instances.push( self.push_node( node ) );
        }
        self.add_group( name, instances );
    }

    /// Empty nodes at the given positions, children of one node named `name`.
    pub fn add_points<'a>( &mut self, name: &str, points: impl IntoIterator<Item = ( &'a str, [f32; 3] )> )
    {
        let children = points
            .into_iter()
            .map( |( point, position )| self.push_node( json!( { "name": point, "translation": to_right_handed( position ) } ) ) )
            .collect();
        self.add_group( name, children );
    }

    /// Point lights, children of one node named `name`. The game stores no intensity:
    /// it is set so that a light gives 1 lux at its range.
    pub fn add_lights( &mut self, name: &str, lights: impl IntoIterator<Item = Light> )
    {
        let children = lights
            .into_iter()
            .map( |Light { name: light_name, position, color, range }|
            {
                self.lights.push( json!( { "type": "point", "color": color, "intensity": range * range, "range": range } ) );
                let light = self.lights.len() - 1;
                self.push_node( json!( { "name": light_name, "translation": to_right_handed( position ), "extensions": { LIGHTS: { "light": light } } } ) )
            } )
            .collect();
        self.add_group( name, children );
    }

    fn add_view( &mut self, bytes: &[u8], target: Option<u32> ) -> usize
    {
        self.bin.resize( self.bin.len().next_multiple_of( ALIGNMENT ), 0 );
        let mut view = json!( { "buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len() } );
        if let Some( target ) = target
        {
            view[ "target" ] = target.into();
        }

        self.bin.extend_from_slice( bytes );
        self.views.push( view );
        self.views.len() - 1
    }

    fn add_accessor( &mut self, bytes: &[u8], target: u32, component_type: u32, count: usize, kind: &str ) -> Value
    {
        let view = self.add_view( bytes, Some( target ) );
        json!( { "bufferView": view, "componentType": component_type, "count": count, "type": kind } )
    }

    fn push_accessor( &mut self, accessor: Value ) -> usize
    {
        self.accessors.push( accessor );
        self.accessors.len() - 1
    }

    fn positions( &mut self, positions: &[[f32; 3]] ) -> usize
    {
        let positions: Vec<[f32; 3]> = positions.iter().copied().map( to_right_handed ).collect();
        let mut min = [ f32::INFINITY; 3 ];
        let mut max = [ f32::NEG_INFINITY; 3 ];
        for position in &positions
        {
            for axis in 0..3
            {
                min[ axis ] = min[ axis ].min( position[ axis ] );
                max[ axis ] = max[ axis ].max( position[ axis ] );
            }
        }

        let mut accessor = self.add_accessor( &float_bytes( &positions ), ARRAY_BUFFER, FLOAT, positions.len(), "VEC3" );
        accessor[ "min" ] = json!( min );
        accessor[ "max" ] = json!( max );
        self.push_accessor( accessor )
    }

    fn normals( &mut self, normals: &[[f32; 3]] ) -> usize
    {
        let normals: Vec<[f32; 3]> = normals.iter().copied().map( to_right_handed ).collect();
        let accessor = self.add_accessor( &float_bytes( &normals ), ARRAY_BUFFER, FLOAT, normals.len(), "VEC3" );
        self.push_accessor( accessor )
    }

    fn uvs( &mut self, uvs: &[[f32; 2]] ) -> usize
    {
        let accessor = self.add_accessor( &float_bytes( uvs ), ARRAY_BUFFER, FLOAT, uvs.len(), "VEC2" );
        self.push_accessor( accessor )
    }

    fn colors( &mut self, colors: &[[u8; 4]] ) -> usize
    {
        let mut accessor = self.add_accessor( colors.as_flattened(), ARRAY_BUFFER, UNSIGNED_BYTE, colors.len(), "VEC4" );
        accessor[ "normalized" ] = true.into();
        self.push_accessor( accessor )
    }

    fn indices( &mut self, triangles: &[[u16; 3]] ) -> usize
    {
        let bytes: Vec<u8> = triangles
            .iter()
            .copied()
            .flat_map( counter_clockwise )
            .flat_map( |index| index.to_le_bytes() )
            .collect();
        let index_count = triangles.as_flattened().len();
        let accessor = self.add_accessor( &bytes, ELEMENT_ARRAY_BUFFER, UNSIGNED_SHORT, index_count, "SCALAR" );
        self.push_accessor( accessor )
    }

    fn texture( &mut self, name: &str ) -> Option<usize>
    {
        if let Some( &cached ) = self.texture_by_name.get( name )
        {
            return cached;
        }

        let texture = ( self.texture_png )( name ).map( |png| self.add_texture( name, &png ) );
        self.texture_by_name.insert( name.to_owned(), texture );
        texture
    }

    fn add_texture( &mut self, name: &str, png: &[u8] ) -> usize
    {
        let view = self.add_view( png, None );
        self.images.push( json!( { "name": name, "bufferView": view, "mimeType": "image/png" } ) );
        self.textures.push( json!( { "source": self.images.len() - 1 } ) );
        self.textures.len() - 1
    }

    fn material( &mut self, material: &Material, double_sided: bool ) -> usize
    {
        let mut pbr = json!( { "metallicFactor": 0.0, "roughnessFactor": 1.0 } );
        if let Material::Color { colors: [ [ red, green, blue ], _ ] } = material
        {
            pbr[ "baseColorFactor" ] = json!( [ red, green, blue, 1.0 ] );
        }
        if let Some( texture ) = material.texture().and_then( |name| self.texture( name ) )
        {
            pbr[ "baseColorTexture" ] = json!( { "index": texture } );
        }

        let mut json = json!( { "pbrMetallicRoughness": pbr, "doubleSided": double_sided } );
        if let Some( name ) = material.texture()
        {
            json[ "name" ] = name.into();
        }
        if let Some( mode ) = alpha_mode( material )
        {
            json[ "alphaMode" ] = mode.into();
            if mode == "MASK"
            {
                json[ "alphaCutoff" ] = ALPHA_CUTOFF.into();
            }
        }
        if matches!( material, Material::Unlit { .. } )
        {
            json[ "extensions" ] = json!( { UNLIT: {} } );
            self.uses_unlit = true;
        }

        self.materials.push( json );
        self.materials.len() - 1
    }

    fn add_primitive( &mut self, attributes: Map<String, Value>, triangles: &[[u16; 3]], material: usize )
    {
        let indices = self.indices( triangles );
        self.primitives.push( json!( { "attributes": attributes, "indices": indices, "material": material } ) );
    }

    /// Turns the primitives added so far into a mesh.
    fn close_mesh( &mut self, name: &str ) -> Option<usize>
    {
        if self.primitives.is_empty()
        {
            return None;
        }

        let primitives = std::mem::take( &mut self.primitives );
        self.meshes.push( json!( { "name": name, "primitives": primitives } ) );
        Some( self.meshes.len() - 1 )
    }

    fn push_node( &mut self, node: Value ) -> usize
    {
        self.nodes.push( node );
        self.nodes.len() - 1
    }

    /// Turns the primitives added so far into a mesh with its own node.
    fn close_node( &mut self, name: &str ) -> Option<usize>
    {
        let mesh = self.close_mesh( name )?;
        Some( self.push_node( json!( { "name": name, "mesh": mesh } ) ) )
    }

    /// Turns the primitives added so far into a mesh with its own root node.
    fn close_root( &mut self, name: &str )
    {
        if let Some( node ) = self.close_node( name )
        {
            self.roots.push( node );
        }
    }

    /// Root node over `children`, left out when there are none.
    fn add_group( &mut self, name: &str, children: Vec<usize> )
    {
        if children.is_empty()
        {
            return;
        }

        let root = self.push_node( json!( { "name": name, "children": children } ) );
        self.roots.push( root );
    }

    fn prototype_mesh( &mut self, name: &str, prototype: &mut impl FnMut( &str ) -> Option<RawMesh> ) -> Option<usize>
    {
        if let Some( &cached ) = self.prototype_by_name.get( name )
        {
            return cached;
        }

        let mesh = prototype( name ).and_then( |raw|
        {
            self.add_raw_geometry( &raw );
            self.close_mesh( name )
        } );
        self.prototype_by_name.insert( name.to_owned(), mesh );
        mesh
    }

    fn add_geometry( &mut self, geometry: &Geometry )
    {
        if geometry.triangles.is_empty()
        {
            return;
        }

        let mut attributes = Map::new();
        attributes.insert( "POSITION".into(), self.positions( geometry.positions ).into() );
        if !geometry.normals.is_empty()
        {
            attributes.insert( "NORMAL".into(), self.normals( geometry.normals ).into() );
        }
        if !geometry.uvs.is_empty()
        {
            attributes.insert( "TEXCOORD_0".into(), self.uvs( geometry.uvs ).into() );
        }
        if !geometry.colors.is_empty()
        {
            attributes.insert( "COLOR_0".into(), self.colors( geometry.colors ).into() );
        }

        let material = self.material( geometry.material, geometry.double_sided );
        self.add_primitive( attributes, geometry.triangles, material );
    }

    fn add_surfaces( &mut self, name: &str, surfaces: &[Surface] )
    {
        for surface in surfaces
        {
            self.add_geometry( &Geometry::from_surface( surface ) );
        }
        self.close_root( name );
    }

    fn add_land( &mut self, name: &str, land: &LandMesh )
    {
        let mut tiles = Vec::new();
        for ( index, tile ) in land.tiles.iter().enumerate()
        {
            for surface in &tile.surfaces
            {
                self.add_geometry( &Geometry::from_surface( surface ) );
            }
            tiles.extend( self.close_node( &format!( "{name}_tile{index}" ) ) );
        }
        self.add_group( name, tiles );
    }

    fn add_interior( &mut self, name: &str, mesh: &InteriorMesh )
    {
        for surface in &mesh.surfaces
        {
            self.add_geometry( &Geometry::from_interior( surface ) );
        }
        self.close_root( name );
    }

    fn add_character( &mut self, name: &str, mesh: &CharacterMesh )
    {
        let material = Material::Diffuse { texture: mesh.texture.clone(), alpha_test: false };
        let geometry = Geometry
        {
            material:     &material,
            positions:    &mesh.positions,
            normals:      &mesh.normals,
            uvs:          &mesh.uvs,
            colors:       &[],
            triangles:    &mesh.triangles,
            double_sided: false,
        };
        self.add_geometry( &geometry );
        self.close_root( name );
    }

    /// Water has no texture of its own: plain colour, seen from both sides.
    fn add_water( &mut self, name: &str, mesh: &WaterMesh )
    {
        let material = Material::Color { colors: [ WATER_COLOR, WATER_COLOR ] };
        let geometry = Geometry
        {
            material:     &material,
            positions:    &mesh.positions,
            normals:      &[],
            uvs:          &[],
            colors:       &[],
            triangles:    &mesh.triangles,
            double_sided: true,
        };
        self.add_geometry( &geometry );
        self.close_root( name );
    }

    fn add_raw( &mut self, name: &str, mesh: &RawMesh )
    {
        self.add_raw_geometry( mesh );
        self.close_root( name );
    }

    /// Raw meshes are leaves and water plants: one-sided cards seen from both sides.
    fn add_raw_geometry( &mut self, mesh: &RawMesh )
    {
        let material = Material::Diffuse { texture: mesh.texture.clone(), alpha_test: mesh.texture.contains( '#' ) };
        let geometry = Geometry
        {
            material:     &material,
            positions:    &mesh.positions,
            normals:      &[],
            uvs:          &mesh.uvs,
            colors:       &[],
            triangles:    &mesh.triangles,
            double_sided: true,
        };
        self.add_geometry( &geometry );
    }

    pub fn finish( mut self, name: &str ) -> Gltf
    {
        if self.roots.is_empty()
        {
            let root = self.push_node( json!( { "name": name } ) );
            self.roots.push( root );
        }
        let mut json = json!( { "asset": { "version": "2.0", "generator": "mormesh" }, "scene": 0, "scenes": [ { "name": name, "nodes": self.roots } ] } );
        json[ "nodes" ] = self.nodes.into();

        let arrays =
        [
            ( "meshes", self.meshes ),
            ( "bufferViews", self.views ),
            ( "accessors", self.accessors ),
            ( "images", self.images ),
            ( "textures", self.textures ),
            ( "materials", self.materials ),
        ];
        for ( key, items ) in arrays.into_iter().filter( |( _, items )| !items.is_empty() )
        {
            json[ key ] = items.into();
        }
        let mut extensions = Vec::new();
        if self.uses_unlit
        {
            extensions.push( UNLIT );
        }
        if !self.lights.is_empty()
        {
            extensions.push( LIGHTS );
            json[ "extensions" ] = json!( { LIGHTS: { "lights": self.lights } } );
        }
        if !extensions.is_empty()
        {
            json[ "extensionsUsed" ] = extensions.into();
        }

        Gltf { json, bin: self.bin }
    }
}

#[ cfg( test ) ]
mod tests
{
    use super::*;

    fn leaf() -> Mesh
    {
        let raw = RawMesh
        {
            texture:      "leaves#_DXT1.tex".into(),
            positions:    vec![ [ 0.0, 0.0, 1.0 ], [ 1.0, 0.0, 1.0 ], [ 0.0, 1.0, 1.0 ] ],
            uvs:          vec![ [ 0.0, 0.0 ], [ 1.0, 0.0 ], [ 0.0, 1.0 ] ],
            triangles:    vec![ [ 0, 1, 2 ] ],
            radius:       1.0,
            y_range:      [ 0.0, 1.0 ],
            center:       [ 0.5, 0.5, 1.0 ],
            half_extents: [ 0.5, 0.5, 0.0 ],
        };
        Mesh::Raw( raw )
    }

    fn fake_png( _: &str ) -> Option<Vec<u8>>
    {
        Some( b"\x89PNG not really".to_vec() )
    }

    #[ test ]
    fn writes_aligned_glb()
    {
        let glb = Gltf::from_mesh( &leaf(), "leaf", fake_png ).to_glb().unwrap();
        assert_eq!( &glb[ ..4 ], GLB_MAGIC );
        assert_eq!( u32::from_le_bytes( glb[ 8..12 ].try_into().unwrap() ) as usize, glb.len() );
        assert_eq!( glb.len() % ALIGNMENT, 0 );

        let ::gltf::Gltf { document, blob } = ::gltf::Gltf::from_slice( &glb ).unwrap();
        let blob = blob.unwrap();
        let primitive = document.meshes().next().unwrap().primitives().next().unwrap();
        let reader = primitive.reader( |_| Some( &blob ) );

        let positions: Vec<[f32; 3]> = reader.read_positions().unwrap().collect();
        assert_eq!( positions[ 1 ], [ 1.0, 0.0, -1.0 ] );
        let indices: Vec<u32> = reader.read_indices().unwrap().into_u32().collect();
        assert_eq!( indices, [ 0, 2, 1 ] );

        let material = primitive.material();
        assert_eq!( material.alpha_mode(), ::gltf::material::AlphaMode::Mask );
        assert!( material.double_sided() );
        assert!( material.pbr_metallic_roughness().base_color_texture().is_some() );
    }

    #[ test ]
    fn writes_point_lights()
    {
        let mut builder = SceneBuilder::new( fake_png );
        let light = Light { name: "lamp".into(), position: [ 1.0, 2.0, 3.0 ], color: [ 1.0, 0.5, 0.0 ], range: 10.0 };
        builder.add_lights( "lights", [ light ] );
        let glb = builder.finish( "scene" ).to_glb().unwrap();

        let document = ::gltf::Gltf::from_slice( &glb ).unwrap().document;
        let lamp = document.nodes().find( |node| node.name() == Some( "lamp" ) ).unwrap();
        assert_eq!( lamp.transform().decomposed().0, [ 1.0, 2.0, -3.0 ] );
        let light = lamp.light().unwrap();
        assert_eq!( ( light.color(), light.range(), light.intensity() ), ( [ 1.0, 0.5, 0.0 ], Some( 10.0 ), 100.0 ) );
    }

    #[ test ]
    fn writes_gltf_with_external_buffer()
    {
        let gltf = Gltf::from_mesh( &leaf(), "leaf", |_| None );
        let json: Value = serde_json::from_str( &gltf.to_gltf( "leaf.bin" ) ).unwrap();
        assert_eq!( json[ "buffers" ][ 0 ][ "uri" ], "leaf.bin" );
        assert_eq!( json[ "buffers" ][ 0 ][ "byteLength" ], gltf.bin().len() );
        assert!( json.get( "images" ).is_none() );
        ::gltf::Gltf::from_slice( gltf.to_gltf( "leaf.bin" ).as_bytes() ).unwrap();
    }
}
