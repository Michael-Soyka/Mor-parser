//! Lightmap image decoder, ported from the game's `D3D.dll`.
//!
//! An image is [`LEVELS`] mip levels of 8-bit luminance, one after another, each starting
//! on a byte boundary. A level is coded like near-lossless JPEG-LS (LOCO-I) with NEAR = 2,
//! without markers, with these differences from the standard:
//!
//! ```text
//! bits       taken from the low bit of each byte up; a k-bit field is read high bit first
//! gradients  D1 = d - b, D2 = b - c, D3 = c - b
//! predictor  the context bias C alone, negated unless MED + sign is zero; MED is not added
//! runs       no run interruption sample: the pixel after a run is coded in regular mode
//! ```
//!
//! Neighbours of x: `a` left, `b` above, `c` above-left, `d` above-right. On the first row
//! all but `a` are 0; in the first column `a = b` and `c` is `b` of the previous row's start.

use crate::{ Error, Result };

pub const LEVELS: usize = 4;

const NEAR:          i32 = 2;
const MAX_VALUE:     i32 = 255;
const QUANT_STEP:    i32 = ( 2 * NEAR ) + 1;
const RANGE:         i32 = ( ( MAX_VALUE + ( 2 * NEAR ) ) / QUANT_STEP ) + 1;
const QBPP:          u32 = i32::BITS - RANGE.leading_zeros();
const LIMIT:         u32 = 32;
const ESCAPE_LENGTH: u32 = LIMIT - QBPP - 1;

const T1: i32 = ( 3 * NEAR ) + 3;
const T2: i32 = ( 5 * NEAR ) + 7;
const T3: i32 = ( 7 * NEAR ) + 21;

const CONTEXTS: usize = 365;
const RESET:    i32   = 64;
const MIN_BIAS: i32   = -128;
const MAX_BIAS: i32   = 127;

const RUN_ORDERS: [u32; 32] = [ 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 9, 10, 11, 12, 13, 14, 15 ];
const MAX_RUN_INDEX: usize = RUN_ORDERS.len() - 1;

/// One mip level, rows top to bottom.
#[ derive( Debug, Clone, PartialEq, Eq ) ]
pub struct LightmapLevel
{
    pub width:     u32,
    pub height:    u32,
    pub luminance: Vec<u8>,
}

/// Decodes the [`LEVELS`] levels of one image; the data must end with the last level.
pub fn decode( data: &[u8], width: u32, height: u32 ) -> Result<Vec<LightmapLevel>>
{
    let mut pos = 0;
    let mut levels = Vec::with_capacity( LEVELS );
    for level in 0..LEVELS
    {
        let shift = u32::try_from( level ).map_err( |_| Error::TooLarge )?;
        let level_width = width.checked_shr( shift ).unwrap_or( 0 ).max( 1 );
        let level_height = height.checked_shr( shift ).unwrap_or( 0 ).max( 1 );

        let mut decoder = LevelDecoder::new( data, pos, level_width, level_height )?;
        decoder.decode()?;
        pos = decoder.bits.pos;

        let level = LightmapLevel { width: level_width, height: level_height, luminance: decoder.image };
        levels.push( level );
    }

    match data.len() - pos
    {
        0 => Ok( levels ),
        rest => Err( Error::TrailingData( rest ) ),
    }
}

struct Bits<'a>
{
    data: &'a [u8],
    pos:  usize,
    byte: u8,
    left: u32,
}

impl Bits<'_>
{
    fn bit( &mut self ) -> Result<u32>
    {
        if self.left == 0
        {
            self.byte = *self.data.get( self.pos ).ok_or( Error::Truncated )?;
            self.pos += 1;
            self.left = u8::BITS;
        }

        let bit = u32::from( self.byte & 1 );
        self.byte >>= 1;
        self.left -= 1;
        Ok( bit )
    }

    fn field( &mut self, length: u32 ) -> Result<u64>
    {
        let mut value = 0;
        for _ in 0..length
        {
            value = ( value << 1 ) | u64::from( self.bit()? );
        }
        Ok( value )
    }

    /// Golomb code of order `k` limited to [`LIMIT`] bits.
    fn golomb( &mut self, k: u32 ) -> Result<u64>
    {
        let mut zeros = 0;
        while self.bit()? == 0
        {
            zeros += 1;
        }

        if zeros < ESCAPE_LENGTH
        {
            let value = ( u64::from( zeros ) << k ) | self.field( k )?;
            return Ok( value );
        }
        Ok( self.field( QBPP )? + 1 )
    }
}

#[ derive( Debug, Clone, Copy ) ]
struct Context
{
    error_sum:  i32,
    bias_sum:   i32,
    correction: i32,
    count:      i32,
}

impl Context
{
    const INITIAL: Self = Self { error_sum: 4, bias_sum: 0, correction: 0, count: 1 };

    fn golomb_order( &self ) -> u32
    {
        let mut order = 0;
        while ( i64::from( self.count ) << order ) < i64::from( self.error_sum )
        {
            order += 1;
        }
        order
    }

    fn update( &mut self, error: i32 )
    {
        self.error_sum += error.abs();
        self.bias_sum += error * QUANT_STEP;
        if self.count == RESET
        {
            self.error_sum >>= 1;
            self.bias_sum >>= 1;
            self.count = RESET / 2;
        }
        self.count += 1;

        if self.bias_sum <= -self.count
        {
            self.bias_sum += self.count;
            self.correction = ( self.correction - 1 ).max( MIN_BIAS );
            if self.bias_sum <= -self.count
            {
                self.bias_sum = 1 - self.count;
            }
        }
        else if self.bias_sum > 0
        {
            self.bias_sum -= self.count;
            self.correction = ( self.correction + 1 ).min( MAX_BIAS );
            if self.bias_sum > 0
            {
                self.bias_sum = 0;
            }
        }
    }
}

#[ derive( Debug, Clone, Copy ) ]
struct Neighbours
{
    a: i32,
    b: i32,
    c: i32,
    d: i32,
}

impl Neighbours
{
    fn is_flat( &self ) -> bool
    {
        let gradients = [ self.d - self.b, self.b - self.c, self.c - self.b ];
        gradients.iter().all( |gradient| gradient.abs() <= NEAR )
    }

    fn quantized_gradients( &self ) -> [i32; 3]
    {
        [ quantize( self.d - self.b ), quantize( self.b - self.c ), quantize( self.c - self.b ) ]
    }

    fn median( &self ) -> i32
    {
        let ( low, high ) = if self.a > self.b { ( self.b, self.a ) } else { ( self.a, self.b ) };
        if self.c >= high
        {
            return low;
        }
        if self.c <= low
        {
            return high;
        }
        self.a - self.c + self.b
    }
}

fn quantize( gradient: i32 ) -> i32
{
    match gradient
    {
        _ if gradient <= -T3 => -4,
        _ if gradient <= -T2 => -3,
        _ if gradient <= -T1 => -2,
        _ if gradient < -NEAR => -1,
        _ if gradient <= NEAR => 0,
        _ if gradient < T1 => 1,
        _ if gradient < T2 => 2,
        _ if gradient < T3 => 3,
        _ => 4,
    }
}

/// Context number for gradients with a non-negative leading sign.
fn context_index( [ q1, q2, q3 ]: [i32; 3] ) -> Result<usize>
{
    let index = match ( q1, q2 )
    {
        ( 0, 0 ) => 360 + q3,
        ( 0, _ ) => 319 + ( 9 * q2 ) + q3,
        _ => 40 + ( 81 * ( q1 - 1 ) ) + ( 9 * q2 ) + q3,
    };
    usize::try_from( index ).map_err( |_| Error::CorruptLightmap )
}

struct LevelDecoder<'a>
{
    bits:      Bits<'a>,
    contexts:  [Context; CONTEXTS],
    run_index: usize,
    width:     usize,
    height:    usize,
    image:     Vec<u8>,

    /// `b` at the start of the previous row, the next row's first `c`.
    row_start_above: i32,
}

impl<'a> LevelDecoder<'a>
{
    fn new( data: &'a [u8], pos: usize, width: u32, height: u32 ) -> Result<Self>
    {
        let width = usize::try_from( width ).map_err( |_| Error::TooLarge )?;
        let height = usize::try_from( height ).map_err( |_| Error::TooLarge )?;
        let pixels = width.checked_mul( height ).ok_or( Error::TooLarge )?;

        let decoder = Self
        {
            bits:            Bits { data, pos, byte: 0, left: 0 },
            contexts:        [ Context::INITIAL; CONTEXTS ],
            run_index:       0,
            width,
            height,
            image:           vec![ 0; pixels ],
            row_start_above: 0,
        };
        Ok( decoder )
    }

    fn pixel( &self, row: usize, col: usize ) -> i32
    {
        i32::from( self.image[ ( row * self.width ) + col ] )
    }

    fn neighbours( &mut self, row: usize, col: usize ) -> Neighbours
    {
        if row == 0
        {
            let a = if col == 0 { 0 } else { self.pixel( row, col - 1 ) };
            return Neighbours { a, b: 0, c: 0, d: 0 };
        }

        let b = self.pixel( row - 1, col );
        let d = if col + 1 < self.width { self.pixel( row - 1, col + 1 ) } else { b };
        if col == 0
        {
            let c = self.row_start_above;
            self.row_start_above = b;
            return Neighbours { a: b, b, c, d };
        }
        Neighbours { a: self.pixel( row, col - 1 ), b, c: self.pixel( row - 1, col - 1 ), d }
    }

    fn decode( &mut self ) -> Result<()>
    {
        for row in 0..self.height
        {
            let mut col = 0;
            while col < self.width
            {
                let mut around = self.neighbours( row, col );
                let is_first_pixel = row == 0 && col == 0;
                if around.is_flat() && !is_first_pixel
                {
                    col = self.decode_run( row, col, around.a )?;
                    if col == self.width
                    {
                        break;
                    }
                    around = self.neighbours( row, col );
                }

                self.decode_regular( row, col, around )?;
                col += 1;
            }
        }
        Ok( () )
    }

    /// Fills a run of `value`, returning the column after it.
    fn decode_run( &mut self, row: usize, mut col: usize, value: i32 ) -> Result<usize>
    {
        let value = u8::try_from( value ).map_err( |_| Error::CorruptLightmap )?;
        let row_start = row * self.width;
        loop
        {
            let order = RUN_ORDERS[ self.run_index ];
            if self.bits.bit()? == 1
            {
                let run = 1usize << order;
                let end = ( col + run ).min( self.width );
                self.image[ row_start + col..row_start + end ].fill( value );
                let is_full_run = end - col == run;
                col = end;

                if is_full_run && self.run_index < MAX_RUN_INDEX
                {
                    self.run_index += 1;
                }
                if col == self.width
                {
                    return Ok( col );
                }
            }
            else
            {
                let run = usize::try_from( self.bits.field( order )? ).map_err( |_| Error::CorruptLightmap )?;
                let end = col.checked_add( run ).filter( |&end| end <= self.width ).ok_or( Error::CorruptLightmap )?;
                self.image[ row_start + col..row_start + end ].fill( value );
                self.run_index = self.run_index.saturating_sub( 1 );
                return Ok( end );
            }
        }
    }

    fn decode_regular( &mut self, row: usize, col: usize, around: Neighbours ) -> Result<()>
    {
        let mut gradients = around.quantized_gradients();
        let is_negative = match gradients
        {
            [ q1, _, _ ] if q1 != 0 => q1 < 0,
            [ _, q2, _ ] if q2 != 0 => q2 < 0,
            [ _, _, q3 ] => q3 < 0,
        };
        if is_negative
        {
            gradients = gradients.map( |gradient| -gradient );
        }
        let index = context_index( gradients )?;
        let context = self.contexts[ index ];

        let sign = i32::from( is_negative );
        let correction = if around.median() + sign == 0 { context.correction } else { -context.correction };
        let prediction = correction.clamp( 0, MAX_VALUE );

        let order = context.golomb_order();
        let mapped = self.bits.golomb( order )?;
        let error = unmap_error( mapped )?;

        let scaled = if is_negative { -( error * QUANT_STEP ) } else { error * QUANT_STEP };
        let mut value = prediction + scaled;
        if value < -NEAR
        {
            value += RANGE * QUANT_STEP;
        }
        else if value > MAX_VALUE + NEAR
        {
            value -= RANGE * QUANT_STEP;
        }

        let stored = u8::try_from( value.clamp( 0, MAX_VALUE ) ).map_err( |_| Error::CorruptLightmap )?;
        self.image[ ( row * self.width ) + col ] = stored;
        self.contexts[ index ].update( error );
        Ok( () )
    }
}

/// Mapped error back to signed: 0, -1, 1, -2, 2, ... for 0, 1, 2, 3, 4, ...
fn unmap_error( mapped: u64 ) -> Result<i32>
{
    let half = i64::try_from( mapped / 2 ).map_err( |_| Error::CorruptLightmap )?;
    let error = if mapped % 2 == 0 { half } else { -( half + 1 ) };
    let error = i16::try_from( error ).map_err( |_| Error::CorruptLightmap )?;
    Ok( i32::from( error ) )
}
