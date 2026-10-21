//! Tests against the installed game. Skipped when `PATHOLOGIC_DATA` does not exist.

use std::collections::BTreeSet;
use std::path::{ Path, PathBuf };

use morft::Font;
use morvfs::Vfs;

fn fonts() -> Option<Vfs>
{
    let dir = PathBuf::from( std::env::var_os( "PATHOLOGIC_DATA" )? );
    if !dir.is_dir()
    {
        eprintln!( "skipped: {} not found", dir.display() );
        return None;
    }
    Some( Vfs::open( dir.join( "Fonts.vfs" ) ).unwrap() )
}

fn is_font( path: &str ) -> bool
{
    Path::new( path ).extension().is_some_and( |extension| extension == morft::EXTENSION )
}

#[ test ]
fn parses_console_font()
{
    let Some( vfs ) = fonts() else { return };
    let font = Font::parse( &vfs.read( "console.ft" ).unwrap() ).unwrap();
    assert_eq!( ( font.line_height(), font.glyph_height(), font.glyphs().len() ), ( 16, 16, 213 ) );
    assert_eq!( font.kerning_pairs().len(), 0 );

    let a = font.glyph( 'A' ).unwrap();
    assert_eq!( ( a.width, a.advance ), ( 11, 8 ) );
}

#[ test ]
fn parses_every_font()
{
    let Some( vfs ) = fonts() else { return };
    let mut parsed = BTreeSet::new();
    for entry in vfs.entries().filter( |entry| is_font( &entry.path ) )
    {
        Font::parse( &vfs.read_entry( entry ).unwrap() ).unwrap_or_else( |err| panic!( "{}: {err}", entry.path ) );
        parsed.insert( entry.path.clone() );
    }

    let mut expected: BTreeSet<String> = ( 8..=25 ).map( |size| format!( "fritz_quadrata_regular_{size}.ft" ) ).collect();
    expected.extend( [ "console.ft", "loading.ft" ].map( String::from ) );
    assert_eq!( parsed, expected );
}
