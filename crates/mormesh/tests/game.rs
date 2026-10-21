//! Tests against the installed game. Skipped when `PATHOLOGIC_DATA` does not exist.

use std::path::PathBuf;

use mormesh::{ LandMesh, Material, StaticMesh };
use morvfs::Vfs;

fn geometries() -> Option<Vfs>
{
    let dir = PathBuf::from( std::env::var_os( "PATHOLOGIC_DATA" )? );
    if !dir.is_dir()
    {
        eprintln!( "skipped: {} not found", dir.display() );
        return None;
    }
    Some( Vfs::open( dir.join( "Geometries.vfs" ) ).unwrap() )
}

#[ test ]
fn parses_poster()
{
    let Some( vfs ) = geometries() else { return };
    let mesh = StaticMesh::parse( &vfs.read( "afisha01.mesh" ).unwrap() ).unwrap();

    let [ surface ] = &mesh.surfaces[ .. ] else { panic!( "expected one surface" ) };
    assert_eq!( surface.material, Material::Diffuse { texture: "afisha_f_DXT1.tex".into(), alpha_test: false } );
    assert_eq!( ( surface.positions.len(), surface.triangles.len() ), ( 4, 2 ) );
    assert_eq!( surface.triangles, [ [ 2, 1, 0 ], [ 1, 2, 3 ] ] );
    assert_eq!( surface.normals, [ [ 0.0, 0.0, 1.0 ]; 4 ] );
    assert_eq!( surface.uvs[ 0 ], [ 1.0, 1.0 ] );

    let [ x0, y0, _ ] = surface.positions[ 0 ];
    let [ x3, y3, _ ] = surface.positions[ 3 ];
    let ( width, height ) = ( x3 - x0, y3 - y0 );
    assert!( ( width - 90.0 ).abs() < 0.01 && ( height - 180.0 ).abs() < 0.01 );
}

#[ test ]
fn parses_terrain_tiles()
{
    let Some( vfs ) = geometries() else { return };
    let land = LandMesh::parse( &vfs.read( "terra.mesh" ).unwrap() ).unwrap();
    assert_eq!( land.tiles.len(), 9 );

    for tile in &land.tiles
    {
        let [ center_x, center_z ] = tile.center;
        let [ half_x, half_z ] = tile.half_extents;
        let tolerance = 1.0;
        for [ x, _, z ] in tile.surfaces.iter().flat_map( |surface| &surface.positions )
        {
            assert!( ( x - center_x ).abs() <= half_x + tolerance && ( z - center_z ).abs() <= half_z + tolerance );
        }
    }
}
