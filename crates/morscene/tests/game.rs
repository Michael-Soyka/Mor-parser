//! Tests against the installed game. Skipped when `PATHOLOGIC_DATA` does not exist.

use std::path::{ Path, PathBuf };

use morscene::actor::{ ActorFile, LandActor };
use morscene::map::{ CityMap, NO_AREA };
use morscene::path::PathFinding;
use morscene::{ INTERIOR_EXTENSION, OUTDOOR_EXTENSION, Scene };
use morvfs::Vfs;

fn data_dir() -> Option<PathBuf>
{
    let dir = PathBuf::from( std::env::var_os( "PATHOLOGIC_DATA" )? );
    if !dir.is_dir()
    {
        eprintln!( "skipped: {} not found", dir.display() );
        return None;
    }
    Some( dir )
}

fn open( dir: &Path, archive: &str ) -> Vfs
{
    Vfs::open( dir.join( format!( "{archive}.vfs" ) ) ).unwrap()
}

fn is_scene( path: &str ) -> bool
{
    let extension = Path::new( path ).extension().and_then( |extension| extension.to_str() ).unwrap_or_default();
    extension.eq_ignore_ascii_case( INTERIOR_EXTENSION ) || extension.eq_ignore_ascii_case( OUTDOOR_EXTENSION )
}

#[ test ]
fn loads_every_scene_with_its_references()
{
    let Some( dir ) = data_dir() else { return };
    let scenes = open( &dir, "Scenes" );
    let mut loaded = 0;
    for entry in scenes.entries().filter( |entry| is_scene( &entry.path ) )
    {
        Scene::load( &entry.path, |file| scenes.read( file ).ok() ).unwrap_or_else( |err| panic!( "{}: {err}", entry.path ) );
        loaded += 1;
    }

    // 965 interiors, 5 outdoor variants of the city.
    assert_eq!( loaded, 965 + 5 );
}

#[ test ]
fn links_city_actors_to_meshes()
{
    let Some( dir ) = data_dir() else { return };
    let ( scenes, actors, geometries ) = ( open( &dir, "Scenes" ), open( &dir, "Actors" ), open( &dir, "Geometries" ) );
    let city = Scene::load( "city_base.osc", |file| scenes.read( file ).ok() ).unwrap();
    assert_eq!( city.land.as_deref(), Some( "zterra" ) );
    let land = LandActor::parse( &actors.read( "zterra" ).unwrap() ).unwrap();
    assert_eq!( land.mesh, "terra.mesh" );
    assert_eq!( city.path_finding.as_deref(), Some( "city_base.osc.pfd" ) );
    assert!( city.points.iter().any( |point| point.name == "birth" ) );

    let mut reader = actors.reader().unwrap();
    let ( mut with_mesh, mut other ) = ( 0, 0 );
    for name in city.actors.iter().filter_map( |actor| actor.name.as_deref() )
    {
        let description = ActorFile::parse( &reader.read( name ).unwrap() ).unwrap();
        match description.mesh()
        {
            Some( mesh ) =>
            {
                assert!( geometries.contains( mesh ), "{name}: {mesh}" );
                with_mesh += 1;
            }
            None => other += 1,
        }
    }

    // Lights, sounds and other non-geometry actors make up the rest.
    assert_eq!( ( with_mesh, other ), ( 1921, 151 ) );
}

#[ test ]
fn parses_every_path_finding_file()
{
    let Some( dir ) = data_dir() else { return };
    let scenes = open( &dir, "Scenes" );
    let mut parsed = 0;
    for entry in scenes.entries().filter( |entry| entry.path.ends_with( ".pfd" ) )
    {
        let path = PathFinding::parse( &scenes.read_entry( entry ).unwrap() ).unwrap_or_else( |err| panic!( "{}: {err}", entry.path ) );
        let grid_cells = u64::from( path.columns ) * u64::from( path.rows );
        assert_eq!( u64::try_from( path.grid.len() ).unwrap(), grid_cells );
        assert!( path.cells.iter().all( |cell| cell.vertices.len() >= 3 && cell.area > 0.0 ), "{}", entry.path );
        parsed += 1;
    }
    assert_eq!( parsed, 32 );

    let city = PathFinding::parse( &scenes.read( "city_base.osc.pfd" ).unwrap() ).unwrap();
    assert_eq!( ( city.vertices.len(), city.cells.len(), city.columns, city.rows ), ( 39233, 35640, 61, 46 ) );
}

#[ test ]
fn parses_city_map()
{
    let Some( dir ) = data_dir() else { return };
    let scenes = open( &dir, "Scenes" );
    let city = Scene::load( "city.osc", |file| scenes.read( file ).ok() ).unwrap();
    let map = CityMap::parse( &scenes.read( city.map.as_deref().unwrap() ).unwrap() ).unwrap();

    assert_eq!( map.districts.len(), 16 );
    assert_eq!( ( map.width, map.height, map.texture.as_str() ), ( 2048, 2048, "city.tex" ) );
    assert!( map.districts.iter().flat_map( |district| &district.polygons ).all( |polygon| polygon.points.len() >= 3 ) );
    assert_eq!( map.area( 0, 0 ), None );
    assert_eq!( map.rects.len(), 32 );
    assert!( map.areas.iter().all( |&rect| rect == NO_AREA || usize::from( rect ) < map.rects.len() ) );

    // Each rect's top edge touches pixels of its own area.
    for ( index, rect ) in map.rects.iter().enumerate()
    {
        let inside = |x: u32, y: u32| map.area( x, y ).is_some_and( |found| std::ptr::eq( found, rect ) );
        assert!( inside( rect.left, rect.top ) || ( rect.left..=rect.right ).any( |x| inside( x, rect.top ) ), "rect {index}" );
    }
}
