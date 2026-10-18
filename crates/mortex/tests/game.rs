//! Tests against the installed game. Skipped when `PATHOLOGIC_DATA` does not exist.

use std::path::PathBuf;

use morvfs::Vfs;
use mortex::{ Format, Texture };

const RGBA_BYTES_PER_PIXEL: usize = 4;

const ROOT_TEXTURES: usize = 2401;
const UI_TEXTURES:   usize = 180;

fn textures() -> Option<Vfs>
{
    let dir = PathBuf::from( std::env::var_os( "PATHOLOGIC_DATA" )? );
    if !dir.is_dir()
    {
        eprintln!( "skipped: {} not found", dir.display() );
        return None;
    }
    Some( Vfs::open( dir.join( "Textures.vfs" ) ).unwrap() )
}

#[ test ]
fn decodes_city_map()
{
    let Some( vfs ) = textures() else { return };
    let tex = Texture::parse( &vfs.read( "city.tex" ).unwrap() ).unwrap();
    assert_eq!( ( tex.width(), tex.height(), tex.format() ), ( 2048, 2048, Format::Dxt5 ) );

    let image = tex.decode( 0, 0 ).unwrap();
    assert_eq!( image.rgba.len(), ( 2048 * 2048 ) * RGBA_BYTES_PER_PIXEL );
}

#[ test ]
fn decodes_every_texture()
{
    let Some( vfs ) = textures() else { return };
    let mut decoded = 0;
    for entry in vfs.entries().filter( |entry| entry.path.ends_with( ".tex" ) )
    {
        let data = vfs.read_entry( entry ).unwrap();
        let tex = Texture::parse( &data ).unwrap_or_else( |err| panic!( "{}: {err}", entry.path ) );
        for face in 0..tex.face_count()
        {
            for mip in 0..tex.mip_count()
            {
                tex.decode( face, mip ).unwrap_or_else( |err| panic!( "{} face {face} mip {mip}: {err}", entry.path ) );
            }
        }
        decoded += 1;
    }

    assert_eq!( decoded, ROOT_TEXTURES + UI_TEXTURES );
}
