//! glTF export of game meshes, read back and validated by the `gltf` crate.
//! Skipped when `PATHOLOGIC_DATA` does not exist.

use std::path::PathBuf;

use mormesh::gltf::Gltf;
use mormesh::{ EXTENSION, Mesh, RawMesh, StaticMesh };
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

fn no_textures( _: &str ) -> Option<Vec<u8>>
{
    None
}

#[ test ]
fn exports_poster_mirrored_to_right_handed()
{
    let Some( vfs ) = geometries() else { return };
    let data = vfs.read( "afisha01.mesh" ).unwrap();
    let source = StaticMesh::parse( &data ).unwrap();
    let glb = Gltf::from_mesh( &Mesh::Static( source.clone() ), "afisha01", no_textures ).to_glb().unwrap();

    let ::gltf::Gltf { document, blob } = ::gltf::Gltf::from_slice( &glb ).unwrap();
    let blob = blob.unwrap();
    let primitive = document.meshes().next().unwrap().primitives().next().unwrap();
    let reader = primitive.reader( |_| Some( &blob ) );

    let positions: Vec<[f32; 3]> = reader.read_positions().unwrap().collect();
    let expected: Vec<[f32; 3]> = source.surfaces[ 0 ].positions.iter().map( |&[ x, y, z ]| [ x, y, -z ] ).collect();
    assert_eq!( positions, expected );

    let indices: Vec<u32> = reader.read_indices().unwrap().into_u32().collect();
    assert_eq!( indices, [ 2, 0, 1, 1, 3, 2 ] );
    assert_eq!( primitive.material().name(), Some( "afisha_f_DXT1.tex" ) );
}

#[ test ]
fn exports_every_mesh_as_valid_glb()
{
    let Some( vfs ) = geometries() else { return };
    let suffix = format!( ".{EXTENSION}" );
    let mut exported = 0;
    for entry in vfs.entries().filter( |entry| entry.path.ends_with( &suffix ) )
    {
        let Ok( mesh ) = Mesh::parse( &vfs.read_entry( entry ).unwrap() ) else { continue };
        let gltf = match &mesh
        {
            Mesh::Detail( detail ) =>
            {
                let prototype = |name: &str| RawMesh::parse( &vfs.read( &format!( "{name}.{EXTENSION}" ) ).ok()? ).ok();
                Gltf::from_detail( detail, &entry.path, prototype, no_textures )
            }
            _ => Gltf::from_mesh( &mesh, &entry.path, no_textures ),
        };
        let glb = gltf.to_glb().unwrap();
        ::gltf::Gltf::from_slice( &glb ).unwrap_or_else( |err| panic!( "{}: {err}", entry.path ) );
        exported += 1;
    }

    // Every mesh in the game, see tests/game.rs.
    assert_eq!( exported, 5998 + 85 + 1 + 74 + 82 + 161 + 2 + 47 );
}
