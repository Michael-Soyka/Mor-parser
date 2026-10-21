//! Parser for Pathologic (Mor. Utopia) `.ft` bitmap fonts.
//!
//! Layout, all little-endian:
//!
//! ```text
//! +-----------+-------------------+-----------------------+----------------------+
//! | header    | glyph table       | kerning               | bitmaps              |
//! | 12 B      | 18 B * N          | 4 B + 8 B * K         | in glyph table order |
//! +-----------+-------------------+-----------------------+----------------------+
//!
//! header
//!   +0     u32  line_height     distance between lines
//!   +4     u32  glyph_height    cell height, shared by all glyphs
//!   +8     u32  glyph_count     N
//!
//! glyph record
//!   +0     u16  code            UTF-16 code unit
//!   +2     u32  width
//!   +6     i32  offset_x        from the pen
//!   +10    i32  offset_y        from the line top, y down
//!   +14    u32  advance
//!
//! kerning
//!   +0     u32  pair_count      K
//!   +4     pair records:
//!            +0  u16  left
//!            +2  u16  right
//!            +4  i32  adjustment, added to the pen between left and right
//!
//! bitmap: width * glyph_height bytes of 8-bit coverage, rows bottom to top
//!
//! glyph placement
//!
//!   pen
//!    |
//!    v
//!    +-------------------------------------------- line top
//!    |        ^
//!    |        | offset_y
//!    |        v
//!    |<------>+-------- width --------+
//!    offset_x |                       |
//!             |                       | glyph_height
//!             |                       |
//!             +-----------------------+
//!    |<------------- advance ------------->| next pen
//! ```

use std::collections::HashMap;

use indexmap::IndexMap;

pub const EXTENSION: &str = "ft";

#[ derive( Debug, thiserror::Error ) ]
pub enum Error
{
    #[ error( "font data is truncated" ) ]
    Truncated,

    #[ error( "{0} trailing bytes after glyph bitmaps" ) ]
    TrailingData( usize ),

    #[ error( "invalid character code {0:#06x}" ) ]
    InvalidChar( u16 ),

    #[ error( "duplicate glyph for {0:?}" ) ]
    DuplicateGlyph( char ),

    #[ error( "font is too large" ) ]
    TooLarge,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Glyph bitmap with 8-bit coverage, rows top to bottom.
#[ derive( Debug, Clone, PartialEq, Eq ) ]
pub struct Glyph
{
    pub width:  u32,
    pub height: u32,

    /// Bitmap position relative to the pen at the top of the line, y pointing down.
    pub offset_x: i32,
    pub offset_y: i32,
    pub advance:  u32,
    pub coverage: Vec<u8>,
}

#[ derive( Debug, Clone ) ]
pub struct Font
{
    line_height:  u32,
    glyph_height: u32,
    glyphs:       IndexMap<char, Glyph>,
    kerning:      HashMap<(char, char), i32>,
}

const GLYPH_RECORD_SIZE:   usize = size_of::<u16>() + size_of::<u32>() + size_of::<i32>() + size_of::<i32>() + size_of::<u32>();
const KERNING_RECORD_SIZE: usize = size_of::<u16>() + size_of::<u16>() + size_of::<i32>();

fn to_usize( value: u32 ) -> Result<usize>
{
    usize::try_from( value ).map_err( |_| Error::TooLarge )
}

fn flip_rows( bitmap: &[u8], width: usize ) -> Vec<u8>
{
    if width == 0
    {
        return Vec::new();
    }

    bitmap.chunks_exact( width ).rev().flatten().copied().collect()
}

struct Reader<'a>
{
    data: &'a [u8],
    pos:  usize,
}

impl<'a> Reader<'a>
{
    fn bytes( &mut self, len: usize ) -> Result<&'a [u8]>
    {
        let end = self.pos.checked_add( len ).ok_or( Error::TooLarge )?;
        let bytes = self.data.get( self.pos..end ).ok_or( Error::Truncated )?;
        self.pos = end;
        Ok( bytes )
    }

    fn array<const N: usize>( &mut self ) -> Result<[u8; N]>
    {
        let mut array = [ 0; N ];
        array.copy_from_slice( self.bytes( N )? );
        Ok( array )
    }

    fn u16( &mut self ) -> Result<u16>
    {
        Ok( u16::from_le_bytes( self.array()? ) )
    }

    fn u32( &mut self ) -> Result<u32>
    {
        Ok( u32::from_le_bytes( self.array()? ) )
    }

    fn i32( &mut self ) -> Result<i32>
    {
        Ok( i32::from_le_bytes( self.array()? ) )
    }

    fn char( &mut self ) -> Result<char>
    {
        let code = self.u16()?;
        char::from_u32( code.into() ).ok_or( Error::InvalidChar( code ) )
    }

    fn count( &mut self ) -> Result<usize>
    {
        to_usize( self.u32()? )
    }

    fn remaining( &self ) -> usize
    {
        self.data.len() - self.pos
    }

    fn max_records( &self, record_size: usize ) -> usize
    {
        self.remaining() / record_size
    }
}

impl Reader<'_>
{
    fn glyph_table( &mut self, count: usize, glyph_height: u32 ) -> Result<IndexMap<char, Glyph>>
    {
        let mut glyphs = IndexMap::with_capacity( count.min( self.max_records( GLYPH_RECORD_SIZE ) ) );
        for _ in 0..count
        {
            let ch = self.char()?;
            let glyph = Glyph
            {
                width:    self.u32()?,
                height:   glyph_height,
                offset_x: self.i32()?,
                offset_y: self.i32()?,
                advance:  self.u32()?,
                coverage: Vec::new(),
            };
            if glyphs.insert( ch, glyph ).is_some()
            {
                return Err( Error::DuplicateGlyph( ch ) );
            }
        }
        Ok( glyphs )
    }

    fn kerning_table( &mut self ) -> Result<HashMap<(char, char), i32>>
    {
        let pair_count = self.count()?;
        let mut kerning = HashMap::with_capacity( pair_count.min( self.max_records( KERNING_RECORD_SIZE ) ) );
        for _ in 0..pair_count
        {
            let left = self.char()?;
            let right = self.char()?;
            let adjustment = self.i32()?;
            kerning.insert( ( left, right ), adjustment );
        }
        Ok( kerning )
    }

    fn bitmap( &mut self, width: u32, height: u32 ) -> Result<Vec<u8>>
    {
        let width = to_usize( width )?;
        let size = width.checked_mul( to_usize( height )? ).ok_or( Error::TooLarge )?;
        let bottom_up = self.bytes( size )?;
        Ok( flip_rows( bottom_up, width ) )
    }
}

impl Font
{
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        let mut reader = Reader { data, pos: 0 };
        let line_height = reader.u32()?;
        let glyph_height = reader.u32()?;
        let glyph_count = reader.count()?;

        let mut glyphs = reader.glyph_table( glyph_count, glyph_height )?;
        let kerning = reader.kerning_table()?;
        for glyph in glyphs.values_mut()
        {
            glyph.coverage = reader.bitmap( glyph.width, glyph_height )?;
        }

        if reader.remaining() > 0
        {
            return Err( Error::TrailingData( reader.remaining() ) );
        }

        Ok( Self { line_height, glyph_height, glyphs, kerning } )
    }

    /// Vertical distance between lines; covers descenders below the glyph cell.
    pub fn line_height( &self ) -> u32
    {
        self.line_height
    }

    pub fn glyph_height( &self ) -> u32
    {
        self.glyph_height
    }

    pub fn glyph( &self, ch: char ) -> Option<&Glyph>
    {
        self.glyphs.get( &ch )
    }

    /// Glyphs in file order.
    pub fn glyphs( &self ) -> impl ExactSizeIterator<Item = (char, &Glyph)>
    {
        self.glyphs.iter().map( |( ch, glyph )| ( *ch, glyph ) )
    }

    /// Advance adjustment between two neighbouring characters, 0 when not listed.
    pub fn kerning( &self, left: char, right: char ) -> i32
    {
        self.kerning.get( &( left, right ) ).copied().unwrap_or( 0 )
    }

    pub fn kerning_pairs( &self ) -> impl ExactSizeIterator<Item = ((char, char), i32)>
    {
        self.kerning.iter().map( |( pair, value )| ( *pair, *value ) )
    }
}

#[ cfg( test ) ]
mod tests
{
    use super::*;

    const LINE_HEIGHT:  u32 = 4;
    const GLYPH_HEIGHT: u32 = 2;

    #[ derive( Default ) ]
    struct GlyphRecord
    {
        code:     u16,
        width:    u32,
        offset_x: i32,
        offset_y: i32,
        advance:  u32,
        bitmap:   Vec<u8>,
    }

    fn code_unit( ch: char ) -> u16
    {
        u16::try_from( u32::from( ch ) ).unwrap()
    }

    fn filled_glyph( ch: char, width: u32 ) -> GlyphRecord
    {
        let bitmap = vec![ u8::MAX; to_usize( width * GLYPH_HEIGHT ).unwrap() ];
        GlyphRecord { code: code_unit( ch ), width, advance: width, bitmap, ..Default::default() }
    }

    fn font_file( glyphs: &[GlyphRecord], kerning: &[(char, char, i32)] ) -> Vec<u8>
    {
        let glyph_count = u32::try_from( glyphs.len() ).unwrap();
        let pair_count = u32::try_from( kerning.len() ).unwrap();

        let mut file = Vec::new();
        for value in [ LINE_HEIGHT, GLYPH_HEIGHT, glyph_count ]
        {
            file.extend_from_slice( &value.to_le_bytes() );
        }

        for glyph in glyphs
        {
            file.extend_from_slice( &glyph.code.to_le_bytes() );
            file.extend_from_slice( &glyph.width.to_le_bytes() );
            file.extend_from_slice( &glyph.offset_x.to_le_bytes() );
            file.extend_from_slice( &glyph.offset_y.to_le_bytes() );
            file.extend_from_slice( &glyph.advance.to_le_bytes() );
        }

        file.extend_from_slice( &pair_count.to_le_bytes() );
        for &( left, right, adjustment ) in kerning
        {
            file.extend_from_slice( &code_unit( left ).to_le_bytes() );
            file.extend_from_slice( &code_unit( right ).to_le_bytes() );
            file.extend_from_slice( &adjustment.to_le_bytes() );
        }

        for glyph in glyphs
        {
            file.extend_from_slice( &glyph.bitmap );
        }
        file
    }

    #[ test ]
    fn parses_glyphs_and_flips_rows()
    {
        let ( a_bottom, a_top ) = ( [ 1, 2 ], [ 3, 4 ] );
        let ( zhe_bottom, zhe_top ) = ( [ 5 ], [ 6 ] );
        let a = GlyphRecord
        {
            code:     code_unit( 'A' ),
            width:    2,
            offset_x: -1,
            offset_y: 3,
            advance:  4,
            bitmap:   [ a_bottom, a_top ].concat(),
        };
        let zhe = GlyphRecord
        {
            code:    code_unit( 'Ж' ),
            width:   1,
            advance: 2,
            bitmap:  [ zhe_bottom, zhe_top ].concat(),
            ..Default::default()
        };
        let file = font_file( &[ a, zhe ], &[ ( 'A', 'Ж', -1 ) ] );

        let font = Font::parse( &file ).unwrap();
        assert_eq!( ( font.line_height(), font.glyph_height() ), ( LINE_HEIGHT, GLYPH_HEIGHT ) );

        let a = font.glyph( 'A' ).unwrap();
        assert_eq!( ( a.width, a.height, a.offset_x, a.offset_y, a.advance ), ( 2, GLYPH_HEIGHT, -1, 3, 4 ) );
        assert_eq!( a.coverage, [ a_top, a_bottom ].concat() );
        assert_eq!( font.glyph( 'Ж' ).unwrap().coverage, [ zhe_top, zhe_bottom ].concat() );

        assert_eq!( font.kerning( 'A', 'Ж' ), -1 );
        assert_eq!( font.kerning( 'Ж', 'A' ), 0 );
        assert_eq!( font.glyphs().map( |( ch, _ )| ch ).collect::<String>(), "AЖ" );
    }

    #[ test ]
    fn accepts_zero_width_glyph()
    {
        let space = GlyphRecord { code: code_unit( ' ' ), width: 0, advance: 3, ..Default::default() };
        let font = Font::parse( &font_file( &[ space ], &[] ) ).unwrap();
        assert!( font.glyph( ' ' ).unwrap().coverage.is_empty() );
    }

    #[ test ]
    fn rejects_truncated_bitmaps()
    {
        let mut file = font_file( &[ filled_glyph( 'A', 2 ) ], &[] );
        file.pop();
        assert!( matches!( Font::parse( &file ), Err( Error::Truncated ) ) );
    }

    #[ test ]
    fn rejects_trailing_data()
    {
        let mut file = font_file( &[ filled_glyph( 'A', 1 ) ], &[] );
        file.push( 0 );
        assert!( matches!( Font::parse( &file ), Err( Error::TrailingData( 1 ) ) ) );
    }

    #[ test ]
    fn rejects_surrogate_code()
    {
        const HIGH_SURROGATE: u16 = 0xd800;
        let glyph = GlyphRecord { code: HIGH_SURROGATE, ..Default::default() };
        let file = font_file( &[ glyph ], &[] );
        assert!( matches!( Font::parse( &file ), Err( Error::InvalidChar( HIGH_SURROGATE ) ) ) );
    }
}
