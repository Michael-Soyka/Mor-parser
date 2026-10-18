//! Tests against the installed game. Skipped when `PATHOLOGIC_DATA` does not exist.

use std::path::{ Path, PathBuf };

use morvfs::{ EXTENSION, Vfs, VfsWriter };

fn data_dir() -> Option<PathBuf>
{
    let dir = PathBuf::from( std::env::var_os( "PATHOLOGIC_DATA" )? );
    if dir.is_dir()
    {
        Some( dir )
    }
    else
    {
        eprintln!( "skipped: {} not found", dir.display() );
        None
    }
}

fn archive_path( dir: &Path, name: &str ) -> PathBuf
{
    dir.join( format!( "{name}.{EXTENSION}" ) )
}

#[ test ]
fn opens_all_archives()
{
    let Some( dir ) = data_dir() else { return };
    let expected_entry_counts = [
        ( "Actors", 7751 ),
        ( "Fonts", 20 ),
        ( "Geometries", 7897 ),
        ( "Scenes", 1018 ),
        ( "Scripts", 963 ),
        ( "Sounds", 1694 ),
        ( "Strings", 4 ),
        ( "Textures", 2931 ),
        ( "UI", 780 ),
        ( "World", 18 ),
    ];
    for ( name, entry_count ) in expected_entry_counts
    {
        let vfs = Vfs::open( archive_path( &dir, name ) ).unwrap();
        assert_eq!( vfs.len(), entry_count, "{name}" );
    }
}

#[ test ]
fn finds_known_entries()
{
    let Some( dir ) = data_dir() else { return };
    let scenes = Vfs::open( archive_path( &dir, "Scenes" ) ).unwrap();
    assert!( scenes.contains( "CITY.MAP" ) );
    assert!( scenes.read( "city_base.osc" ).unwrap().starts_with( b"<Scene>" ) );

    let textures = Vfs::open( archive_path( &dir, "Textures" ) ).unwrap();
    assert!( textures.entries().any( |entry| entry.path.starts_with( "UI/" ) ) );

    let geometries = Vfs::open( archive_path( &dir, "Geometries" ) ).unwrap();
    assert!( geometries.contains( "mono_anna11 д.mms" ) );
}

#[ test ]
fn rewrites_byte_identical()
{
    let Some( dir ) = data_dir() else { return };
    for name in [ "World", "Strings", "Fonts", "Scenes", "Scripts", "UI" ]
    {
        let path = archive_path( &dir, name );
        let vfs = Vfs::open( &path ).unwrap();
        let mut writer = VfsWriter::new();
        for entry in vfs.entries()
        {
            writer.add( &entry.path, vfs.read_entry( entry ).unwrap(), entry.modified ).unwrap();
        }

        let mut rebuilt = Vec::new();
        writer.write_to( &mut rebuilt ).unwrap();
        let original = std::fs::read( &path ).unwrap();
        assert!( rebuilt == original, "{} differs after rewrite", path.display() );
    }
}
