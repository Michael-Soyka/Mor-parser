//! glTF 2.0 export of a loaded scene, see [`mormesh::gltf`].
//!
//! The game keeps scene geometry in world coordinates, so every mesh is placed as stored:
//! the terrain or the interior first, then a root node per actor with a mesh, named after the actor,
//! light actors as point lights under `lights`, a group's lights named `<actor>_<index>`,
//! and the scene points as empty nodes under `points`. Coronas, the ambient light, sounds
//! and other actors are left out.

use mormesh::gltf::{ Gltf, Light, SceneBuilder };
use mormesh::{ EXTENSION, Mesh, RawMesh };

use crate::Scene;
use crate::actor::{ ActorFile, LandActor, LightActor, PointLight };

const LIGHTS_NODE: &str = "lights";
const POINTS_NODE: &str = "points";

/// Exported scene with the files that could not be used.
#[ derive( Debug, Clone ) ]
pub struct SceneGltf
{
    pub gltf: Gltf,

    /// Actor descriptions and meshes that were not found or did not parse.
    pub missing: Vec<String>,
}

/// `actor` returns a description from `Actors.vfs`, `geometry` a file from `Geometries.vfs`,
/// `texture_png` PNG bytes for a texture name; `None` leaves the material untextured.
pub fn export(
    scene: &Scene,
    name: &str,
    mut actor: impl FnMut( &str ) -> Option<Vec<u8>>,
    mut geometry: impl FnMut( &str ) -> Option<Vec<u8>>,
    texture_png: impl FnMut( &str ) -> Option<Vec<u8>>,
) -> SceneGltf
{
    let mut builder = SceneBuilder::new( texture_png );
    let mut missing = Vec::new();

    for actor_name in [ &scene.land, &scene.level ].into_iter().flatten()
    {
        let Some( land ) = actor( actor_name ).and_then( |data| LandActor::parse( &data ).ok() ) else
        {
            missing.push( actor_name.clone() );
            continue;
        };
        if let Some( mesh ) = read_mesh( &mut geometry, &land.mesh, &mut missing )
        {
            builder.add_mesh( &mesh, actor_name );
        }
    }

    let mut lights = Vec::new();
    for actor_name in scene.actors.iter().filter_map( |scene_actor| scene_actor.name.as_deref() )
    {
        let Some( description ) = actor( actor_name ).and_then( |data| ActorFile::parse( &data ).ok() ) else
        {
            missing.push( actor_name.to_owned() );
            continue;
        };
        if let ActorFile::Light( light ) = &description
        {
            lights.extend( gltf_lights( actor_name, light ) );
            continue;
        }
        let Some( file ) = description.mesh() else { continue };
        let Some( mesh ) = read_mesh( &mut geometry, file, &mut missing ) else { continue };

        match &mesh
        {
            Mesh::Detail( detail ) =>
            {
                let prototype = |prototype: &str|
                {
                    let file = format!( "{prototype}.{EXTENSION}" );
                    let raw = geometry( &file ).and_then( |data| RawMesh::parse( &data ).ok() );
                    if raw.is_none()
                    {
                        missing.push( file );
                    }
                    raw
                };
                builder.add_detail( detail, actor_name, prototype );
            }
            _ => builder.add_mesh( &mesh, actor_name ),
        }
    }

    builder.add_lights( LIGHTS_NODE, lights );
    builder.add_points( POINTS_NODE, scene.points.iter().map( |point| ( point.name.as_str(), point.position ) ) );
    SceneGltf { gltf: builder.finish( name ), missing }
}

fn read_mesh( geometry: &mut impl FnMut( &str ) -> Option<Vec<u8>>, file: &str, missing: &mut Vec<String> ) -> Option<Mesh>
{
    let mesh = geometry( file ).and_then( |data| Mesh::parse( &data ).ok() );
    if mesh.is_none()
    {
        missing.push( file.to_owned() );
    }
    mesh
}

fn gltf_lights( name: &str, actor: &LightActor ) -> Vec<Light>
{
    let [ red, green, blue, _ ] = actor.color.map( srgb_to_linear );
    let light = |light_name: String, point: &PointLight| Light { name: light_name, position: point.position, color: [ red, green, blue ], range: point.radius };
    match &actor.group
    {
        Some( group ) => group.iter().enumerate().map( |( index, point )| light( format!( "{name}_{index}" ), point ) ).collect(),
        None => vec![ light( name.to_owned(), &actor.light ) ],
    }
}

fn srgb_to_linear( channel: u8 ) -> f32
{
    let value = f32::from( channel ) / 255.0;
    if value <= 0.04045
    {
        value / 12.92
    }
    else
    {
        ( ( value + 0.055 ) / 1.055 ).powf( 2.4 )
    }
}
