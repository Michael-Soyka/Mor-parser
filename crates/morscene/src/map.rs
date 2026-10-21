//! City map (`city.map`, named by `<Map FileName>`): district outlines in world space and the
//! in-game map picture with the areas a cursor can point at.
//!
//! ```text
//! u32 d, d * district: u32 n, n * polygon
//! polygon: u32 k, k * f32x2 point, k * f32x3 edge line
//! u32 compressed_length, zlib stream, unpacked:
//!   u32 width, u32 height, u8 length, cp1251 texture, u32 tile
//!   (height / tile) * (width / tile) tiles, row by row:
//!     u8 uniform, uniform: u16 rect index; otherwise tile * tile u16 rect index, row by row
//!   f32x2 center, f32 scale, u32 r, r * { u32 area, u32 left, u32 top, u32 right, u32 bottom }
//! ```
//!
//! Points are world x and z; an edge line is `[a, b, c]` with `a * x + b * z + c` zero on the edge.
//! A picture pixel is `( width / 2 ) + ( ( x - center ) * scale )`; the direction of z is not verified.

use crate::binary::{ COUNT_SIZE, Error, Reader, Result, VEC2_SIZE, VEC3_SIZE, to_usize };

/// Mask value of pixels outside any area.
pub const NO_AREA: u16 = u16::MAX;

const POLYGON_MIN_SIZE: usize = COUNT_SIZE;
const RECT_SIZE:        usize = 5 * COUNT_SIZE;
const UNIFORM_TILE:     usize = size_of::<u8>() + size_of::<u16>();

#[ derive( Debug, Clone, PartialEq ) ]
pub struct Polygon
{
    pub points: Vec<[f32; 2]>,
    pub lines:  Vec<[f32; 3]>,
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct District
{
    /// Convex parts.
    pub polygons: Vec<Polygon>,
}

/// An area and its bounds in picture pixels.
#[ derive( Debug, Clone, Copy, PartialEq, Eq ) ]
pub struct AreaRect
{
    /// Area id, not the rect's index.
    pub area:   u32,
    pub left:   u32,
    pub top:    u32,
    pub right:  u32,
    pub bottom: u32,
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct CityMap
{
    pub districts: Vec<District>,

    pub width:   u32,
    pub height:  u32,
    pub texture: String,

    /// `width * height` indices into [`CityMap::rects`], row by row, [`NO_AREA`] outside.
    pub areas: Vec<u16>,

    /// World x and z at the middle of the picture.
    pub center: [f32; 2],

    /// Picture pixels per world unit.
    pub scale: f32,

    pub rects: Vec<AreaRect>,
}

impl CityMap
{
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        let mut reader = Reader::new( data );
        let districts = reader.counted_items( COUNT_SIZE, |reader| Ok( District { polygons: reader.counted_items( POLYGON_MIN_SIZE, read_polygon )? } ) )?;
        let unpacked = reader.zlib()?;
        reader.finish()?;

        let mut reader = Reader::new( &unpacked );
        let width = reader.u32()?;
        let height = reader.u32()?;
        let texture = reader.string()?;
        let tile = reader.u32()?;
        let areas = read_areas( &mut reader, [ width, height ], tile )?;
        let center = reader.floats()?;
        let scale = reader.f32()?;
        let rects = reader.counted_items( RECT_SIZE, read_rect )?;
        reader.finish()?;

        let map = Self { districts, width, height, texture, areas, center, scale, rects };
        Ok( map )
    }

    /// Area under picture pixel `x, y`, if any.
    pub fn area( &self, x: u32, y: u32 ) -> Option<&AreaRect>
    {
        if x >= self.width || y >= self.height
        {
            return None;
        }

        let index = ( u64::from( y ) * u64::from( self.width ) ) + u64::from( x );
        let rect = *self.areas.get( usize::try_from( index ).ok()? )?;
        self.rects.get( usize::from( rect ) )
    }
}

fn read_polygon( reader: &mut Reader ) -> Result<Polygon>
{
    let count = reader.count()?;
    let points = reader.items( count, VEC2_SIZE, Reader::floats )?;
    let lines = reader.items( count, VEC3_SIZE, Reader::floats )?;
    Ok( Polygon { points, lines } )
}

fn read_rect( reader: &mut Reader ) -> Result<AreaRect>
{
    let rect = AreaRect
    {
        area:   reader.u32()?,
        left:   reader.u32()?,
        top:    reader.u32()?,
        right:  reader.u32()?,
        bottom: reader.u32()?,
    };
    Ok( rect )
}

fn read_areas( reader: &mut Reader, [ width, height ]: [u32; 2], tile: u32 ) -> Result<Vec<u16>>
{
    let [ width, height, tile ] = [ to_usize( width )?, to_usize( height )?, to_usize( tile )? ];
    if tile == 0 || width % tile != 0 || height % tile != 0
    {
        return Err( Error::Invalid( "tile size" ) );
    }

    let pixel_count = width.checked_mul( height ).ok_or( Error::TooLarge )?;
    let tile_count = pixel_count / ( tile * tile );
    if tile_count > reader.remaining() / UNIFORM_TILE
    {
        return Err( Error::Truncated );
    }

    let mut areas = vec![ NO_AREA; pixel_count ];
    for tile_y in 0..( height / tile )
    {
        for tile_x in 0..( width / tile )
        {
            let uniform = reader.bool()?.then( || reader.u16() ).transpose()?;
            for y in 0..tile
            {
                let row_start = ( ( ( tile_y * tile ) + y ) * width ) + ( tile_x * tile );
                for pixel in &mut areas[ row_start..row_start + tile ]
                {
                    *pixel = match uniform
                    {
                        Some( area ) => area,
                        None => reader.u16()?,
                    };
                }
            }
        }
    }
    Ok( areas )
}

#[ cfg( test ) ]
mod tests
{
    use std::io::Write as _;

    use flate2::Compression;
    use flate2::write::ZlibEncoder;

    use super::*;

    fn packed( unpacked: &[u8] ) -> Vec<u8>
    {
        let mut encoder = ZlibEncoder::new( Vec::new(), Compression::default() );
        encoder.write_all( unpacked ).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut data = 0u32.to_le_bytes().to_vec();
        data.extend_from_slice( &u32::try_from( compressed.len() ).unwrap().to_le_bytes() );
        data.extend_from_slice( &compressed );
        data
    }

    #[ test ]
    fn fills_uniform_and_detailed_tiles()
    {
        let mut unpacked = Vec::new();
        for value in [ 4u32, 2 ]
        {
            unpacked.extend_from_slice( &value.to_le_bytes() );
        }
        unpacked.push( 3 );
        unpacked.extend_from_slice( b"map" );
        unpacked.extend_from_slice( &2u32.to_le_bytes() );

        unpacked.push( 1 );
        unpacked.extend_from_slice( &NO_AREA.to_le_bytes() );
        unpacked.push( 0 );
        for area in [ 1u16, 2, 3, 4 ]
        {
            unpacked.extend_from_slice( &area.to_le_bytes() );
        }

        for value in [ 0.0f32, 0.0, 1.0 ]
        {
            unpacked.extend_from_slice( &value.to_le_bytes() );
        }
        unpacked.extend_from_slice( &5u32.to_le_bytes() );
        for _ in 0..5
        {
            for value in [ 9u32, 2, 0, 4, 2 ]
            {
                unpacked.extend_from_slice( &value.to_le_bytes() );
            }
        }

        let map = CityMap::parse( &packed( &unpacked ) ).unwrap();
        assert_eq!( map.areas, [ NO_AREA, NO_AREA, 1, 2, NO_AREA, NO_AREA, 3, 4 ] );
        assert_eq!( map.area( 2, 1 ).map( |rect| rect.area ), Some( 9 ) );
        assert_eq!( ( map.area( 0, 0 ), map.area( 4, 0 ) ), ( None, None ) );
    }
}
