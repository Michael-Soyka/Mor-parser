//! Vegetation (the engine's `DetailedObject`): instances of raw meshes scattered over a grid of cells.
//!
//! ```text
//! u32 compressed_length, zlib stream of compressed_length bytes, unpacked:
//!
//! u32 n, n * { u8 length, cp1251 raw mesh name }
//! f32x2 origin, u32 columns, u32 rows, f32x2 cell_size, f32 y_center, f32 y_half_range
//! u8 k, k * u32 light id
//! u32 lights_total, u32 instances_total, u32 colors_total, u32 max_cell_instances
//! rows * columns cells, row by row:
//!   u16 count, count * instance, u8 has_colors, [count * u32 0xAARRGGBB if has_colors]
//! instance: u8 x, f32 y, u8 z, u8 prototype, u8 m, m * { u8 light index, u8 value },
//!           [f32x4 tilt quaternion: version 2]
//! ```
//!
//! Instance x and z are offsets in world units from the cell corner, `( cell * cell_size ) + origin`.
//! The yaw is not stored: the engine draws it at load time, see [`DetailInstance::yaw`].

use encoding_rs::WINDOWS_1251;

use crate::{ COUNT_SIZE, Error, Reader, Result, check_index, unpack_zlib };

const INSTANCE_MIN_SIZE: usize = ( 3 * size_of::<u8>() ) + size_of::<f32>() + ( 2 * size_of::<u8>() );
const LIGHT_SIZE:        usize = 2 * size_of::<u8>();
const CELL_MIN_SIZE:     usize = size_of::<u16>() + size_of::<u8>();
const NAME_MIN_SIZE:     usize = size_of::<u8>();

const YAW_SEED: u32 = 0x6ec7e;

/// Maps a generator output to `[0, 1]`, as boost's `uniform_01` does.
const RANDOM_SCALE: f32 = 1.0 / 4_294_967_296.0;

#[ derive( Clone, Copy, PartialEq, Eq ) ]
enum Version
{
    Flat,
    Tilted,
}

#[ derive( Debug, Clone, Copy, PartialEq, Eq ) ]
pub struct InstanceLight
{
    /// Index into [`DetailMesh::lights`].
    pub light: u8,
    pub value: u8,
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct DetailInstance
{
    pub position: [f32; 3],

    /// Index into [`DetailMesh::prototypes`].
    pub prototype: u8,

    pub lights: Vec<InstanceLight>,

    /// Rotation about +Y in radians, as the engine draws it from MT19937 seeded with 0x6ec7e,
    /// one value per instance in file order.
    pub yaw: f32,

    /// Unit quaternion `[x, y, z, w]` applied before the yaw; version 2 files only.
    pub tilt: Option<[f32; 4]>,

    /// 0xAARRGGBB.
    pub color: Option<u32>,
}

impl DetailInstance
{
    /// Quaternion `[x, y, z, w]` of the yaw combined with the tilt, `yaw * tilt`.
    pub fn rotation( &self ) -> [f32; 4]
    {
        let half_yaw = self.yaw / 2.0;
        let [ x, y, z, w ] = self.tilt.unwrap_or( [ 0.0, 0.0, 0.0, 1.0 ] );
        let ( sin, cos ) = half_yaw.sin_cos();
        let rotation = [ ( cos * x ) + ( sin * z ), ( cos * y ) + ( sin * w ), ( cos * z ) - ( sin * x ), ( cos * w ) - ( sin * y ) ];
        rotation
    }
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct DetailCell
{
    pub instances: Vec<DetailInstance>,
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct DetailMesh
{
    /// Raw meshes the instances refer to, without the `.mesh` extension.
    pub prototypes: Vec<String>,

    /// Corner of cell 0 in the XZ plane.
    pub origin:    [f32; 2],
    pub columns:   u32,
    pub rows:      u32,
    pub cell_size: [f32; 2],

    pub y_center:     f32,
    pub y_half_range: f32,

    /// Ids of the scene lights the instances refer to.
    pub lights: Vec<u32>,

    /// Stored by the game, not always the actual maximum.
    pub max_cell_instances: u32,

    /// `rows * columns` cells, row by row.
    pub cells: Vec<DetailCell>,
}

impl DetailMesh
{
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        let ( unpacked, tail ) = unpack_zlib( data )?;
        if !tail.is_empty()
        {
            return Err( Error::TrailingData( tail.len() ) );
        }

        Self::parse_unpacked( &unpacked, Version::Flat ).or_else( |_| Self::parse_unpacked( &unpacked, Version::Tilted ) )
    }

    fn parse_unpacked( data: &[u8], version: Version ) -> Result<Self>
    {
        let mut reader = Reader::new( data );
        let prototypes = reader.counted_items( NAME_MIN_SIZE, read_name )?;
        let origin = reader.floats()?;
        let columns = reader.u32()?;
        let rows = reader.u32()?;
        let [ cell_width, cell_depth, y_center, y_half_range ] = reader.floats()?;
        let light_count = usize::from( reader.u8()? );
        let lights = reader.items( light_count, COUNT_SIZE, Reader::u32 )?;
        let [ lights_total, instances_total, colors_total, max_cell_instances ] = [ reader.u32()?, reader.u32()?, reader.u32()?, reader.u32()? ];

        let mut grid = Grid
        {
            reader:     &mut reader,
            version,
            prototypes: prototypes.len(),
            lights:     lights.len(),
            random:     Mt19937::new( YAW_SEED ),
            totals:     Totals::default(),
        };
        let cell_count = usize::try_from( u64::from( rows ) * u64::from( columns ) ).map_err( |_| Error::TooLarge )?;
        let mut cells = Vec::with_capacity( grid.reader.capacity( cell_count, CELL_MIN_SIZE ) );
        let mut row = 0.0;
        for _ in 0..rows
        {
            let z = ( row * cell_depth ) + origin[ 1 ];
            let mut column = 0.0;
            for _ in 0..columns
            {
                let x = ( column * cell_width ) + origin[ 0 ];
                cells.push( grid.cell( x, z )? );
                column += 1.0;
            }
            row += 1.0;
        }

        let totals = grid.totals;
        reader.finish()?;
        if [ totals.lights, totals.instances, totals.colors ] != [ lights_total, instances_total, colors_total ].map( u64::from )
        {
            return Err( Error::UnknownLayout );
        }

        let mesh = Self
        {
            prototypes,
            origin,
            columns,
            rows,
            cell_size: [ cell_width, cell_depth ],
            y_center,
            y_half_range,
            lights,
            max_cell_instances,
            cells,
        };
        Ok( mesh )
    }

    pub fn instances( &self ) -> impl Iterator<Item = &DetailInstance>
    {
        self.cells.iter().flat_map( |cell| &cell.instances )
    }
}

fn read_name( reader: &mut Reader ) -> Result<String>
{
    let len = usize::from( reader.u8()? );
    let ( name, _ ) = WINDOWS_1251.decode_without_bom_handling( reader.bytes( len )? );
    Ok( name.into_owned() )
}

#[ derive( Default ) ]
struct Totals
{
    lights:    u64,
    instances: u64,
    colors:    u64,
}

struct Grid<'r, 'a>
{
    reader:     &'r mut Reader<'a>,
    version:    Version,
    prototypes: usize,
    lights:     usize,
    random:     Mt19937,
    totals:     Totals,
}

impl Grid<'_, '_>
{
    fn cell( &mut self, x: f32, z: f32 ) -> Result<DetailCell>
    {
        let count = self.reader.u16()?;
        let mut instances = Vec::with_capacity( self.reader.capacity( count.into(), INSTANCE_MIN_SIZE ) );
        for _ in 0..count
        {
            instances.push( self.instance( x, z )? );
        }

        if self.reader.bool()?
        {
            for instance in &mut instances
            {
                instance.color = Some( self.reader.u32()? );
            }
            self.totals.colors += u64::from( count );
        }
        Ok( DetailCell { instances } )
    }

    fn instance( &mut self, cell_x: f32, cell_z: f32 ) -> Result<DetailInstance>
    {
        let x = cell_x + f32::from( self.reader.u8()? );
        let y = self.reader.f32()?;
        let z = cell_z + f32::from( self.reader.u8()? );
        let prototype = self.reader.u8()?;
        check_index( "prototype", prototype.into(), self.prototypes )?;

        let light_count = self.reader.u8()?;
        let lights = self.reader.items( light_count.into(), LIGHT_SIZE, |reader| read_light( reader, self.lights ) )?;
        let tilt = match self.version
        {
            Version::Flat => None,
            Version::Tilted => Some( self.reader.floats()? ),
        };

        self.totals.lights += u64::from( light_count );
        self.totals.instances += 1;
        let instance = DetailInstance
        {
            position:  [ x, y, z ],
            prototype,
            lights,
            yaw:       self.random.unit() * std::f32::consts::TAU,
            tilt,
            color:     None,
        };
        Ok( instance )
    }
}

fn read_light( reader: &mut Reader, light_count: usize ) -> Result<InstanceLight>
{
    let light = reader.u8()?;
    check_index( "light", light.into(), light_count )?;
    Ok( InstanceLight { light, value: reader.u8()? } )
}

const MT_SIZE:       usize = 624;
const MT_SHIFT:      usize = 397;
const MT_MATRIX:     u32   = 0x9908_b0df;
const MT_UPPER_MASK: u32   = 0x8000_0000;
const MT_LOWER_MASK: u32   = 0x7fff_ffff;
const MT_INIT:       u32   = 1_812_433_253;

/// The 32-bit Mersenne Twister, as in the C++ standard library.
struct Mt19937
{
    state: [u32; MT_SIZE],
    index: usize,
}

impl Mt19937
{
    fn new( seed: u32 ) -> Self
    {
        let mut state = [ 0; MT_SIZE ];
        state[ 0 ] = seed;
        for ( index, step ) in ( 1..MT_SIZE ).zip( 1u32.. )
        {
            let previous = state[ index - 1 ];
            state[ index ] = MT_INIT.wrapping_mul( previous ^ ( previous >> 30 ) ).wrapping_add( step );
        }
        Self { state, index: MT_SIZE }
    }

    fn twist( &mut self )
    {
        for index in 0..MT_SIZE
        {
            let bits = ( self.state[ index ] & MT_UPPER_MASK ) | ( self.state[ ( index + 1 ) % MT_SIZE ] & MT_LOWER_MASK );
            let mut next = self.state[ ( index + MT_SHIFT ) % MT_SIZE ] ^ ( bits >> 1 );
            if bits & 1 != 0
            {
                next ^= MT_MATRIX;
            }
            self.state[ index ] = next;
        }
        self.index = 0;
    }

    fn next( &mut self ) -> u32
    {
        if self.index >= MT_SIZE
        {
            self.twist();
        }

        let mut value = self.state[ self.index ];
        self.index += 1;
        value ^= value >> 11;
        value ^= ( value << 7 ) & 0x9d2c_5680;
        value ^= ( value << 15 ) & 0xefc6_0000;
        value ^= value >> 18;
        value
    }

    /// Rounds to the nearest f32 first, like the game, so the result can reach 1.
    fn unit( &mut self ) -> f32
    {
        ( self.next() as f32 ) * RANDOM_SCALE
    }
}

#[ cfg( test ) ]
mod tests
{
    use super::*;

    #[ test ]
    fn draws_the_games_random_sequence()
    {
        let mut random = Mt19937::new( YAW_SEED );
        let first = [ random.next(), random.next(), random.next() ];
        assert_eq!( first, [ 1_308_584_950, 2_559_359_795, 2_450_611_914 ] );
    }

    #[ test ]
    fn combines_yaw_with_tilt()
    {
        let tilt = [ 0.1, 0.7, -0.3, 0.6 ];
        let instance = DetailInstance { position: [ 0.0; 3 ], prototype: 0, lights: Vec::new(), yaw: 1.0, tilt: Some( tilt ), color: None };
        let ( sin, cos ) = 0.5f32.sin_cos();
        let yaw = [ 0.0, sin, 0.0, cos ];

        let [ x1, y1, z1, w1 ] = yaw;
        let [ x2, y2, z2, w2 ] = tilt;
        let hamilton =
        [
            ( w1 * x2 ) + ( x1 * w2 ) + ( y1 * z2 ) - ( z1 * y2 ),
            ( w1 * y2 ) - ( x1 * z2 ) + ( y1 * w2 ) + ( z1 * x2 ),
            ( w1 * z2 ) + ( x1 * y2 ) - ( y1 * x2 ) + ( z1 * w2 ),
            ( w1 * w2 ) - ( x1 * x2 ) - ( y1 * y2 ) - ( z1 * z2 ),
        ];
        for ( actual, expected ) in instance.rotation().into_iter().zip( hamilton )
        {
            assert!( ( actual - expected ).abs() < 1e-6, "{actual} {expected}" );
        }
    }
}
