//! Usage: mesh2gltf <data_dir> <name.mesh> <out.glb | out.gltf>
//! Reads `Geometries.vfs` and `Textures.vfs` from `data_dir`; textures are embedded as PNG.
//! A `.gltf` output gets its buffer next to it, with the extension changed to `.bin`.
//! Vegetation is exported with its prototype raw meshes, one node per instance.

use std::io::Cursor;
use std::path::Path;
use std::process::ExitCode;

use image::{ ExtendedColorType, ImageFormat };
use mormesh::{ EXTENSION, Mesh, RawMesh };
use mormesh::gltf::Gltf;
use morvfs::Vfs;
use mortex::Texture;

const USAGE: &str = "usage: mesh2gltf <data_dir> <name.mesh> <out.glb | out.gltf>";

fn texture_png( textures: &Vfs, name: &str ) -> Option<Vec<u8>>
{
    let data = textures.read( name ).ok()?;
    let image = Texture::parse( &data ).ok()?.decode( 0, 0 ).ok()?;

    let mut png = Vec::new();
    let mut writer = Cursor::new( &mut png );
    image::write_buffer_with_format( &mut writer, &image.rgba, image.width, image.height, ExtendedColorType::Rgba8, ImageFormat::Png ).ok()?;
    Some( png )
}

fn save( gltf: &Gltf, out: &Path ) -> Result<(), Box<dyn std::error::Error>>
{
    if out.extension().is_some_and( |extension| extension.eq_ignore_ascii_case( "gltf" ) )
    {
        let bin_path = out.with_extension( "bin" );
        let bin_uri = bin_path.file_name().and_then( |name| name.to_str() ).ok_or( "output name is not valid UTF-8" )?;
        std::fs::write( out, gltf.to_gltf( bin_uri ) )?;
        std::fs::write( &bin_path, gltf.bin() )?;
    }
    else
    {
        std::fs::write( out, gltf.to_glb()? )?;
    }
    Ok( () )
}

fn run( args: &[String] ) -> Result<String, Box<dyn std::error::Error>>
{
    let [ data_dir, name, out ] = args else
    {
        return Err( USAGE.into() );
    };
    let data_dir = Path::new( data_dir );
    let geometries = Vfs::open( data_dir.join( "Geometries.vfs" ) )?;
    let textures = Vfs::open( data_dir.join( "Textures.vfs" ) )?;

    let mesh = Mesh::parse( &geometries.read( name )? )?;
    let mut missing = Vec::new();
    let mesh_name = name.strip_suffix( ".mesh" ).unwrap_or( name );
    let texture = |texture: &str|
    {
        let png = texture_png( &textures, texture );
        if png.is_none()
        {
            missing.push( texture.to_owned() );
        }
        png
    };
    let gltf = match &mesh
    {
        Mesh::Detail( detail ) =>
        {
            let prototype = |prototype: &str| RawMesh::parse( &geometries.read( &format!( "{prototype}.{EXTENSION}" ) ).ok()? ).ok();
            Gltf::from_detail( detail, mesh_name, prototype, texture )
        }
        _ => Gltf::from_mesh( &mesh, mesh_name, texture ),
    };
    save( &gltf, Path::new( out ) )?;

    let mut summary = format!( "{name} -> {out}" );
    if !missing.is_empty()
    {
        summary += &format!( ", textures not found: {}", missing.join( ", " ) );
    }
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
