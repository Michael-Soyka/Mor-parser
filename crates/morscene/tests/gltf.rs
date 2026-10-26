//! glTF export of game scenes, read back and validated by the `gltf` crate.
//! Skipped when `PATHOLOGIC_DATA` does not exist.

use std::path::{ Path, PathBuf };

use morscene::Scene;
use morscene::actor::ActorFile;
use morscene::gltf::{ SceneGltf, export };
use morvfs::Vfs;

struct Data
{
    scenes:     Vfs,
    actors:     Vfs,
    geometries: Vfs,
}

fn data() -> Option<Data>
{
    let dir = PathBuf::from( std::env::var_os( "PATHOLOGIC_DATA" )? );
    if !dir.is_dir()
    {
        eprintln!( "skipped: {} not found", dir.display() );
        return None;
    }

    let open = |archive: &str| Vfs::open( dir.join( format!( "{archive}.vfs" ) ) ).unwrap();
    Some( Data { scenes: open( "Scenes" ), actors: open( "Actors" ), geometries: open( "Geometries" ) } )
}

fn export_scene( data: &Data, file: &str ) -> ( Scene, SceneGltf )
{
    let scene = Scene::load( file, |file| data.scenes.read( file ).ok() ).unwrap();
    let name = Path::new( file ).file_stem().unwrap().to_str().unwrap();
    let exported = export( &scene, name, |actor| data.actors.read( actor ).ok(), |file| data.geometries.read( file ).ok(), |_| None );
    ( scene, exported )
}

fn root_names( document: &::gltf::Document ) -> Vec<String>
{
    let scene = document.default_scene().unwrap();
    scene.nodes().map( |node| node.name().unwrap_or_default().to_owned() ).collect()
}

/// Lights of the scene's light actors, groups counted by their lights.
fn light_count( data: &Data, scene: &Scene ) -> usize
{
    let lights = scene.actors.iter().filter_map( |actor| ActorFile::parse( &data.actors.read( actor.name.as_deref()? ).ok()? ).ok() );
    lights
        .map( |actor| match actor
        {
            ActorFile::Light( light ) => light.group.map_or( 1, |group| group.len() ),
            _ => 0,
        } )
        .sum()
}

#[ test ]
fn exports_interior_with_level_and_actors()
{
    let Some( data ) = data() else { return };
    let ( scene, exported ) = export_scene( &data, "house1_base.isc" );
    assert_eq!( exported.missing, Vec::<String>::new() );

    let document = ::gltf::Gltf::from_slice( &exported.gltf.to_glb().unwrap() ).unwrap().document;
    let roots = root_names( &document );
    assert_eq!( roots.first().map( String::as_str ), Some( "ihouse1" ) );
    assert!( roots.iter().any( |name| name == "ihouse1_stairs" ) );
    assert_eq!( roots[ roots.len() - 2.. ], [ "lights", "points" ] );
    assert_eq!( ( scene.level.as_deref(), scene.ambient ), ( Some( "ihouse1" ), Some( [ 20.0; 3 ] ) ) );

    // Level, 85 geometry actors, lights, points.
    assert_eq!( roots.len(), 1 + 85 + 1 + 1 );
    assert_eq!( document.lights().unwrap().len(), light_count( &data, &scene ) );
}

#[ test ]
fn exports_city_with_terrain_water_and_vegetation()
{
    let Some( data ) = data() else { return };
    let ( scene, exported ) = export_scene( &data, "city_base.osc" );
    assert_eq!( exported.missing, Vec::<String>::new() );

    let document = ::gltf::Gltf::from_slice( &exported.gltf.to_glb().unwrap() ).unwrap().document;
    assert_eq!( document.lights().unwrap().len(), light_count( &data, &scene ) );
    let roots = root_names( &document );
    for name in [ "zterra", "river", "arena_weed", "afisha01", "lights" ]
    {
        assert!( roots.iter().any( |root| root == name ), "{name}" );
    }
}
