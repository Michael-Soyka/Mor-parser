//! Usage: mesh2obj <Geometries.vfs> <name.mesh> <out.obj>
//! Writes positions as stored, one OBJ group per surface; uv v is flipped to OBJ's bottom-up convention.

use std::fmt::Write as _;
use std::process::ExitCode;

use mormesh::layout::interior::InteriorSurface;
use mormesh::{ Mesh, Surface };
use morvfs::Vfs;

const OBJ_FIRST_INDEX: usize = 1;

struct Group<'a>
{
    name:      String,
    texture:   Option<&'a str>,
    positions: &'a [[f32; 3]],
    normals:   &'a [[f32; 3]],
    uvs:       &'a [[f32; 2]],
    triangles: &'a [[u16; 3]],
}

impl<'a> Group<'a>
{
    fn from_interior( index: usize, surface: &'a InteriorSurface ) -> Self
    {
        Self
        {
            name:      format!( "surface{index}" ),
            texture:   surface.material.texture(),
            positions: &surface.positions,
            normals:   &surface.normals,
            uvs:       &surface.uvs,
            triangles: &surface.triangles,
        }
    }

    fn from_surface( index: usize, surface: &'a Surface ) -> Self
    {
        Self
        {
            name:      format!( "surface{index}" ),
            texture:   surface.material.texture(),
            positions: &surface.positions,
            normals:   &surface.normals,
            uvs:       &surface.uvs,
            triangles: &surface.triangles,
        }
    }
}

fn groups( mesh: &Mesh ) -> Vec<Group<'_>>
{
    if let Mesh::Raw( raw ) = mesh
    {
        let group = Group
        {
            name:      "raw".into(),
            texture:   Some( &raw.texture ),
            positions: &raw.positions,
            normals:   &[],
            uvs:       &raw.uvs,
            triangles: &raw.triangles,
        };
        return vec![ group ];
    }
    if let Mesh::Character( character ) = mesh
    {
        let group = Group
        {
            name:      "character".into(),
            texture:   Some( &character.texture ),
            positions: &character.positions,
            normals:   &character.normals,
            uvs:       &character.uvs,
            triangles: &character.triangles,
        };
        return vec![ group ];
    }
    if let Mesh::Water( water ) = mesh
    {
        let group = Group
        {
            name:      "water".into(),
            texture:   None,
            positions: &water.positions,
            normals:   &[],
            uvs:       &[],
            triangles: &water.triangles,
        };
        return vec![ group ];
    }
    if let Mesh::Interior( interior ) = mesh
    {
        return interior.surfaces.iter().enumerate().map( |( index, surface )| Group::from_interior( index, surface ) ).collect();
    }
    mesh.surfaces().enumerate().map( |( index, surface )| Group::from_surface( index, surface ) ).collect()
}

fn to_obj( groups: &[Group] ) -> Result<String, std::fmt::Error>
{
    let mut obj = String::new();
    let ( mut base_v, mut base_vt, mut base_vn ) = ( OBJ_FIRST_INDEX, OBJ_FIRST_INDEX, OBJ_FIRST_INDEX );
    for group in groups
    {
        writeln!( obj, "g {}", group.name )?;
        if let Some( texture ) = group.texture
        {
            writeln!( obj, "usemtl {texture}" )?;
        }

        for [ x, y, z ] in group.positions
        {
            writeln!( obj, "v {x} {y} {z}" )?;
        }
        for [ u, v ] in group.uvs
        {
            let bottom_up_v = 1.0 - v;
            writeln!( obj, "vt {u} {bottom_up_v}" )?;
        }
        for [ x, y, z ] in group.normals
        {
            writeln!( obj, "vn {x} {y} {z}" )?;
        }

        let has_uvs = !group.uvs.is_empty();
        let has_normals = !group.normals.is_empty();
        for triangle in group.triangles
        {
            obj.push( 'f' );
            for index in triangle.map( usize::from )
            {
                write!( obj, " {}", base_v + index )?;
                match ( has_uvs, has_normals )
                {
                    ( true, true ) => write!( obj, "/{}/{}", base_vt + index, base_vn + index )?,
                    ( true, false ) => write!( obj, "/{}", base_vt + index )?,
                    ( false, true ) => write!( obj, "//{}", base_vn + index )?,
                    ( false, false ) => {}
                }
            }
            obj.push( '\n' );
        }

        base_v += group.positions.len();
        base_vt += group.uvs.len();
        base_vn += group.normals.len();
    }
    Ok( obj )
}

fn run( args: &[String] ) -> Result<String, Box<dyn std::error::Error>>
{
    let [ archive, name, out ] = args else
    {
        return Err( "usage: mesh2obj <Geometries.vfs> <name.mesh> <out.obj>".into() );
    };

    let mesh = Mesh::parse( &Vfs::open( archive )?.read( name )? )?;
    let groups = groups( &mesh );
    std::fs::write( out, to_obj( &groups )? )?;

    let vertices: usize = groups.iter().map( |group| group.positions.len() ).sum();
    let triangles: usize = groups.iter().map( |group| group.triangles.len() ).sum();
    let summary = format!( "{name}: {} groups, {vertices} vertices, {triangles} triangles -> {out}", groups.len() );
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
