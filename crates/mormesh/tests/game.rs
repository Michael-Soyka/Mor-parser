//! Tests against the installed game. Skipped when `PATHOLOGIC_DATA` does not exist.

use std::path::PathBuf;

use mormesh::layout::character::{ CharacterMesh, Layout };
use mormesh::layout::detail::DetailMesh;
use mormesh::{ EXTENSION, LandMesh, Material, Mesh, RawMesh, StaticMesh };
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
fn parses_every_mesh()
{
    let Some( vfs ) = geometries() else { return };
    let suffix = format!( ".{EXTENSION}" );
    let ( mut statics, mut movables, mut interiors, mut details, mut characters, mut raws ) = ( 0, 0, 0, 0, 0, 0 );
    let ( mut lands, mut waters ) = ( Vec::new(), Vec::new() );
    for entry in vfs.entries().filter( |e| e.path.ends_with( &suffix ) )
    {
        match Mesh::parse( &vfs.read_entry( entry ).unwrap() )
        {
            Ok( Mesh::Static( _ ) ) => statics += 1,
            Ok( Mesh::Movable( _ ) ) => movables += 1,
            Ok( Mesh::Land( _ ) ) => lands.push( entry.path.as_str() ),
            Ok( Mesh::Interior( _ ) ) => interiors += 1,
            Ok( Mesh::Detail( _ ) ) => details += 1,
            Ok( Mesh::Character( _ ) ) => characters += 1,
            Ok( Mesh::Water( _ ) ) => waters.push( entry.path.as_str() ),
            Ok( Mesh::Raw( _ ) ) => raws += 1,
            Err( err ) => panic!( "{}: {err}", entry.path ),
        }
    }

    assert_eq!( ( statics, movables, interiors, details, characters, raws ), ( 5998, 85, 74, 82, 161, 47 ) );
    assert_eq!( lands, [ "terra.mesh" ] );
    waters.sort_unstable();
    assert_eq!( waters, [ "pond_water.mesh", "river.mesh" ] );
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

#[ test ]
fn places_vegetation_on_raw_prototypes()
{
    let Some( vfs ) = geometries() else { return };
    let mut reader = vfs.reader().unwrap();
    let ( mut meshes, mut tilted, mut instances ) = ( 0, 0, 0 );
    for entry in vfs.entries()
    {
        let Ok( mesh ) = DetailMesh::parse( &reader.read_entry( entry ).unwrap() ) else { continue };
        for prototype in &mesh.prototypes
        {
            RawMesh::parse( &reader.read( &format!( "{prototype}.{EXTENSION}" ) ).unwrap() ).unwrap_or_else( |err| panic!( "{prototype}: {err}" ) );
        }

        let [ x0, z0 ] = mesh.origin.map( f64::from );
        let [ width, depth ] = mesh.cell_size.map( f64::from );
        let max_offset = f64::from( u8::MAX );
        let x_end = x0 + ( f64::from( mesh.columns ) * width ) + max_offset;
        let z_end = z0 + ( f64::from( mesh.rows ) * depth ) + max_offset;
        for instance in mesh.instances()
        {
            let [ x, _, z ] = instance.position.map( f64::from );
            assert!( ( x0..=x_end ).contains( &x ) && ( z0..=z_end ).contains( &z ), "{}", entry.path );
            assert!( ( 0.0..=std::f32::consts::TAU ).contains( &instance.yaw ) );

            let length: f32 = instance.rotation().iter().map( |value| value * value ).sum();
            assert!( ( length - 1.0 ).abs() < 1e-3, "{}", entry.path );
        }

        meshes += 1;
        tilted += usize::from( mesh.instances().any( |instance| instance.tilt.is_some() ) );
        instances += mesh.instances().count();
    }
    assert_eq!( ( meshes, tilted ), ( 82, 5 ) );
    assert!( instances > 0 );
}

#[ test ]
fn parses_characters_with_animations()
{
    let Some( vfs ) = geometries() else { return };
    let mut reader = vfs.reader().unwrap();
    let ( mut full, mut simple, mut animations ) = ( 0, 0, 0 );
    for entry in vfs.entries()
    {
        let Ok( mesh ) = CharacterMesh::parse( &reader.read_entry( entry ).unwrap() ) else { continue };
        for influences in &mesh.weights
        {
            let total: f32 = influences.iter().map( |influence| influence.weight ).sum();
            assert!( ( total - 1.0 ).abs() < 1e-3, "{}", entry.path );
        }

        let roots = mesh.bones.iter().filter( |bone| bone.parent.is_none() ).count();
        assert!( ( 1..=2 ).contains( &roots ), "{}", entry.path );
        for group in &mesh.groups
        {
            for animation in &group.animations
            {
                assert_eq!( animation.tracks.len(), group.bones.len() );
                assert!( animation.duration() >= 0.0 );
                animations += 1;
            }
        }

        match mesh.layout
        {
            Layout::Full => full += 1,
            Layout::Simple => simple += 1,
        }
    }
    assert_eq!( ( full, simple, animations ), ( 72, 89, 1294 ) );

    let burah = CharacterMesh::parse( &reader.read( "NPC_Burah.mesh" ).unwrap() ).unwrap();
    assert_eq!( ( burah.bones.len(), burah.positions.len(), burah.triangles.len() ), ( 38, 3257, 3746 ) );
    assert_eq!( burah.texture, "burah_2_DXT1.tex" );
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

#[ test ]
fn parses_every_interior()
{
    let Some( vfs ) = geometries() else { return };
    let mut reader = vfs.reader().unwrap();
    let ( mut interiors, mut pages ) = ( 0, 0 );
    for entry in vfs.entries()
    {
        let Ok( Mesh::Interior( interior ) ) = Mesh::parse( &reader.read_entry( entry ).unwrap() ) else { continue };
        let leaves = interior.leaves();
        assert_eq!( leaves.len(), interior.leaf_count as usize, "{}", entry.path );


        for page in interior.surfaces.iter().flat_map( |surface| &surface.lightmaps ).flat_map( |lightmap| &lightmap.pages )
        {
            page.decode().unwrap_or_else( |err| panic!( "{}: {err}", entry.path ) );
            pages += 1;
        }
        interiors += 1;
    }

    assert_eq!( ( interiors, pages ), ( 74, 2379 ) );
}
