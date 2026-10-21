//! Tests against the installed game. Skipped when `PATHOLOGIC_DATA` does not exist.

use std::path::PathBuf;

use mormesh::{ LandMesh, Material, Mesh, StaticMesh };
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

fn fnv1a( bytes: &[u8] ) -> u64
{
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME:        u64 = 0x0100_0000_01b3;
    bytes.iter().fold( OFFSET_BASIS, |hash, &byte| ( hash ^ u64::from( byte ) ).wrapping_mul( PRIME ) )
}

/// Fingerprints of the first level, taken from the game's own decoder run in an emulator.
#[ test ]
fn decodes_altar_lightmap_like_the_game()
{
    let Some( vfs ) = geometries() else { return };
    let mesh = StaticMesh::parse( &vfs.read( "altar_out.mesh" ).unwrap() ).unwrap();
    let page = &mesh.surfaces[ 0 ].lightmaps[ 0 ].pages[ 0 ];
    let [ first, second ] = page.decode().unwrap();

    let sizes: Vec<( u32, u32 )> = first.iter().map( |level| ( level.width, level.height ) ).collect();
    assert_eq!( sizes, [ ( 256, 256 ), ( 128, 128 ), ( 64, 64 ), ( 32, 32 ) ] );
    assert_eq!( fnv1a( &first[ 0 ].luminance ), 0x26e3_6ae1_3f59_085b );
    assert_eq!( fnv1a( &second[ 0 ].luminance ), 0xeb9b_6c9a_52af_dad2 );
}

#[ test ]
fn decodes_every_lightmap_page()
{
    let Some( vfs ) = geometries() else { return };
    let mut reader = vfs.reader().unwrap();
    let mut pages = 0;
    for entry in vfs.entries()
    {
        let Ok( mesh ) = Mesh::parse( &reader.read_entry( entry ).unwrap() ) else { continue };
        for page in mesh.surfaces().flat_map( |surface| &surface.lightmaps ).flat_map( |lightmap| &lightmap.pages )
        {
            page.decode().unwrap_or_else( |err| panic!( "{}: {err}", entry.path ) );
            pages += 1;
        }
    }

    // 17839 in static meshes, 100 in the terrain.
    assert_eq!( pages, 17839 + 100 );
}
