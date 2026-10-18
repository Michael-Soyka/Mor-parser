//! Usage: tex2png <Textures.vfs> <name.tex> <out.png> [mip]

use std::process::ExitCode;

use morvfs::Vfs;
use mortex::Texture;

const USAGE: &str = "usage: tex2png <Textures.vfs> <name.tex> <out.png> [mip]";

fn run( args: &[String] ) -> Result<String, Box<dyn std::error::Error>>
{
    let [ archive, name, out, rest @ .. ] = args else
    {
        return Err( USAGE.into() );
    };
    let mip = match rest.first()
    {
        Some( mip ) => mip.parse()?,
        None => 0,
    };

    let data = Vfs::open( archive )?.read( name )?;
    let tex = Texture::parse( &data )?;
    let first_face = 0;
    let image = tex.decode( first_face, mip )?;
    image::save_buffer( out, &image.rgba, image.width, image.height, image::ExtendedColorType::Rgba8 )?;

    let summary = format!( "{name}: {:?} {}x{}, {} mips -> {out}", tex.format(), image.width, image.height, tex.mip_count() );
    Ok( summary )
}

fn main() -> ExitCode
{
    let args: Vec<String> = std::env::args().skip( 1 ).collect();
    match run( &args )
    {
        Ok( summary ) =>
        {
            println!( "{summary}" );
            ExitCode::SUCCESS
        }
        Err( err ) =>
        {
            eprintln!( "{err}" );
            ExitCode::FAILURE
        }
    }
}
