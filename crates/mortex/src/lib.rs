//! Decoder for Pathologic (Mor. Utopia) `.tex` textures.
//!
//! A `.tex` file is a plain DDS. The game uses DXT1/DXT3/DXT5 and a few
//! uncompressed 24/32-bit BGR(A) textures, including one cubemap.
//!
//! Layout, all little-endian:
//!
//! ```text
//! +--------+---------------------------+-------------------------------------+
//! | "DDS " | header, 124 B             | pixel data                          |
//! +--------+---------------------------+-------------------------------------+
//!
//! header fields read here, offsets from file start
//!   12     u32  height
//!   16     u32  width
//!   28     u32  mip_count
//!   76     pixel format: 80 u32 flags, 84 fourcc "DXT1"/"DXT3"/"DXT5",
//!                        88 u32 bit_count, 92 u32 * 4 RGBA masks
//!   112    u32  caps2, CUBEMAP bit set for cubemaps
//!
//! pixel data, face-major
//!   +--------+--------+-----+-------------+
//!   | mip 0  | mip 1  | ... | mip n-1     |  face 0
//!   +--------+--------+-----+-------------+
//!   | mip 0  | mip 1  | ... | mip n-1     |  face 1..5, cubemaps only: +X -X +Y -Y +Z -Z
//!   +--------+--------+-----+-------------+
//!
//! mip size: max(1, width >> mip) * max(1, height >> mip)
//!   DXT1          8 B per 4x4 block
//!   DXT3, DXT5   16 B per 4x4 block
//!   BGRA8, BGRX8  4 B per pixel
//!   BGR8          3 B per pixel
//! ```

use std::ops::Range;

use ddsfile::{ Caps2, D3DFormat, Dds };

pub const EXTENSION: &str = "tex";

const CUBEMAP_FACES:        u32   = 6;
const RGBA_BYTES_PER_PIXEL: usize = 4;
const OPAQUE:               u8    = u8::MAX;

#[ derive( Debug, thiserror::Error ) ]
pub enum Error
{
    #[ error( "invalid DDS: {0}" ) ]
    Dds( #[ from ] ddsfile::Error ),

    #[ error( "unsupported pixel format: {0}" ) ]
    UnsupportedFormat( String ),

    #[ error( "face {face} / mip {mip} out of range" ) ]
    OutOfRange { face: u32, mip: u32 },

    #[ error( "pixel data is truncated" ) ]
    Truncated,

    #[ error( "texture is too large" ) ]
    TooLarge,
}

pub type Result<T> = std::result::Result<T, Error>;

#[ derive( Debug, Clone, Copy, PartialEq, Eq ) ]
pub enum Format
{
    Dxt1,
    Dxt3,
    Dxt5,
    Bgra8,
    Bgrx8,
    Bgr8,
}

enum Encoding
{
    Compressed( texpresso::Format ),
    Uncompressed { bytes_per_pixel: usize, has_alpha: bool },
}

impl Format
{
    fn from_d3d( format: Option<D3DFormat> ) -> Result<Self>
    {
        match format
        {
            Some( D3DFormat::DXT1 ) => Ok( Self::Dxt1 ),
            Some( D3DFormat::DXT3 ) => Ok( Self::Dxt3 ),
            Some( D3DFormat::DXT5 ) => Ok( Self::Dxt5 ),
            Some( D3DFormat::A8R8G8B8 ) => Ok( Self::Bgra8 ),
            Some( D3DFormat::X8R8G8B8 ) => Ok( Self::Bgrx8 ),
            Some( D3DFormat::R8G8B8 ) => Ok( Self::Bgr8 ),
            other => Err( Error::UnsupportedFormat( format!( "{other:?}" ) ) ),
        }
    }

    fn encoding( self ) -> Encoding
    {
        match self
        {
            Self::Dxt1 => Encoding::Compressed( texpresso::Format::Bc1 ),
            Self::Dxt3 => Encoding::Compressed( texpresso::Format::Bc2 ),
            Self::Dxt5 => Encoding::Compressed( texpresso::Format::Bc3 ),
            Self::Bgra8 => Encoding::Uncompressed { bytes_per_pixel: 4, has_alpha: true },
            Self::Bgrx8 => Encoding::Uncompressed { bytes_per_pixel: 4, has_alpha: false },
            Self::Bgr8 => Encoding::Uncompressed { bytes_per_pixel: 3, has_alpha: false },
        }
    }

    fn level_size( self, width: usize, height: usize ) -> Result<usize>
    {
        let size = match self.encoding()
        {
            Encoding::Compressed( block ) => checked_area( texpresso::num_blocks( width ), texpresso::num_blocks( height ), block.block_size() ),
            Encoding::Uncompressed { bytes_per_pixel, .. } => checked_area( width, height, bytes_per_pixel ),
        };
        size.ok_or( Error::TooLarge )
    }
}

impl Format
{
    /// DXT1, DXT3 and DXT5 are BC1, BC2 and BC3 for GPU APIs, uploadable without decoding.
    pub fn is_block_compressed( self ) -> bool
    {
        matches!( self.encoding(), Encoding::Compressed( _ ) )
    }
}

/// One face/mip level as stored, rows top to bottom.
#[ derive( Debug, Clone, Copy, PartialEq, Eq ) ]
pub struct Level<'a>
{
    pub width:  u32,
    pub height: u32,

    /// 4x4 blocks for block-compressed formats, BGR(A) pixels otherwise.
    pub data: &'a [u8],
}

/// Decoded RGBA8 image, rows top to bottom.
#[ derive( Debug, Clone, PartialEq, Eq ) ]
pub struct Image
{
    pub width:  u32,
    pub height: u32,
    pub rgba:   Vec<u8>,
}

#[ derive( Debug ) ]
pub struct Texture
{
    dds:    Dds,
    format: Format,
}

fn to_usize( value: u32 ) -> Result<usize>
{
    usize::try_from( value ).map_err( |_| Error::TooLarge )
}

fn checked_area( columns: usize, rows: usize, bytes_per_cell: usize ) -> Option<usize>
{
    columns.checked_mul( rows )?.checked_mul( bytes_per_cell )
}

fn mip_extent( size: u32, mip: u32 ) -> u32
{
    size.checked_shr( mip ).unwrap_or( 0 ).max( 1 )
}

fn bgr_to_rgba( bgr: &[u8], bytes_per_pixel: usize, has_alpha: bool, rgba: &mut [u8] )
{
    for ( bgr_pixel, rgba_pixel ) in bgr.chunks_exact( bytes_per_pixel ).zip( rgba.chunks_exact_mut( RGBA_BYTES_PER_PIXEL ) )
    {
        let ( blue, green, red ) = ( bgr_pixel[ 0 ], bgr_pixel[ 1 ], bgr_pixel[ 2 ] );
        let alpha = if has_alpha { bgr_pixel[ 3 ] } else { OPAQUE };
        rgba_pixel.copy_from_slice( &[ red, green, blue, alpha ] );
    }
}

impl Texture
{
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        let dds = Dds::read( data )?;
        let format = Format::from_d3d( dds.get_d3d_format() )?;
        Ok( Self { dds, format } )
    }

    pub fn format( &self ) -> Format
    {
        self.format
    }

    pub fn width( &self ) -> u32
    {
        self.dds.get_width()
    }

    pub fn height( &self ) -> u32
    {
        self.dds.get_height()
    }

    pub fn mip_count( &self ) -> u32
    {
        self.dds.get_num_mipmap_levels().max( 1 )
    }

    /// 6 for cubemaps (+X, -X, +Y, -Y, +Z, -Z), otherwise 1.
    pub fn face_count( &self ) -> u32
    {
        if self.is_cubemap() { CUBEMAP_FACES } else { 1 }
    }

    pub fn is_cubemap( &self ) -> bool
    {
        self.dds.header.caps2.contains( Caps2::CUBEMAP )
    }

    fn mip_dimensions( &self, mip: u32 ) -> ( u32, u32 )
    {
        ( mip_extent( self.width(), mip ), mip_extent( self.height(), mip ) )
    }

    fn level_size( &self, mip: u32 ) -> Result<usize>
    {
        let ( width, height ) = self.mip_dimensions( mip );
        self.format.level_size( to_usize( width )?, to_usize( height )? )
    }

    fn levels_size( &self, mips: Range<u32> ) -> Result<usize>
    {
        mips.map( |mip| self.level_size( mip ) ).try_fold( 0usize, |total, size| total.checked_add( size? ).ok_or( Error::TooLarge ) )
    }

    fn level_data( &self, face: u32, mip: u32 ) -> Result<&[u8]>
    {
        if face >= self.face_count() || mip >= self.mip_count()
        {
            return Err( Error::OutOfRange { face, mip } );
        }

        let face_size = self.levels_size( 0..self.mip_count() )?;
        let mip_offset = self.levels_size( 0..mip )?;
        let face_offset = to_usize( face )?.checked_mul( face_size ).ok_or( Error::TooLarge )?;
        let start = face_offset.checked_add( mip_offset ).ok_or( Error::TooLarge )?;
        let end = start.checked_add( self.level_size( mip )? ).ok_or( Error::TooLarge )?;
        self.dds.data.get( start..end ).ok_or( Error::Truncated )
    }

    pub fn level( &self, face: u32, mip: u32 ) -> Result<Level<'_>>
    {
        let data = self.level_data( face, mip )?;
        let ( width, height ) = self.mip_dimensions( mip );
        Ok( Level { width, height, data } )
    }

    /// Decodes one face/mip level to RGBA8.
    pub fn decode( &self, face: u32, mip: u32 ) -> Result<Image>
    {
        let Level { width, height, data } = self.level( face, mip )?;
        let ( columns, rows ) = ( to_usize( width )?, to_usize( height )? );
        let rgba_size = checked_area( columns, rows, RGBA_BYTES_PER_PIXEL ).ok_or( Error::TooLarge )?;
        let mut rgba = vec![ 0; rgba_size ];

        match self.format.encoding()
        {
            Encoding::Compressed( block ) => block.decompress( data, columns, rows, &mut rgba ),
            Encoding::Uncompressed { bytes_per_pixel, has_alpha } => bgr_to_rgba( data, bytes_per_pixel, has_alpha, &mut rgba ),
        }

        Ok( Image { width, height, rgba } )
    }
}

#[ cfg( test ) ]
mod tests
{
    use super::*;

    const MAGIC:             &[u8; 4] = b"DDS ";
    const HEADER_SIZE:       u32      = 124;
    const PIXEL_FORMAT_SIZE: u32      = 32;

    const MAGIC_AT:              usize = 0;
    const HEADER_SIZE_AT:        usize = 4;
    const HEADER_FLAGS_AT:       usize = 8;
    const HEIGHT_AT:             usize = 12;
    const WIDTH_AT:              usize = 16;
    const PIXEL_FORMAT_SIZE_AT:  usize = 76;
    const PIXEL_FORMAT_FLAGS_AT: usize = 80;
    const FOURCC_AT:             usize = 84;
    const BIT_COUNT_AT:          usize = 88;
    const RED_MASK_AT:           usize = 92;
    const GREEN_MASK_AT:         usize = 96;
    const BLUE_MASK_AT:          usize = 100;
    const ALPHA_MASK_AT:         usize = 104;
    const CAPS_AT:               usize = 108;
    const PIXEL_DATA_AT:         usize = 128;

    const DDSD_CAPS:        u32 = 0x1;
    const DDSD_HEIGHT:      u32 = 0x2;
    const DDSD_WIDTH:       u32 = 0x4;
    const DDSD_PIXELFORMAT: u32 = 0x1000;

    const DDPF_ALPHAPIXELS: u32 = 0x1;
    const DDPF_FOURCC:      u32 = 0x4;
    const DDPF_RGB:         u32 = 0x40;

    const DDSCAPS_TEXTURE: u32 = 0x1000;

    fn put_bytes( file: &mut [u8], at: usize, bytes: &[u8] )
    {
        file[ at..at + bytes.len() ].copy_from_slice( bytes );
    }

    fn put_u32( file: &mut [u8], at: usize, value: u32 )
    {
        put_bytes( file, at, &value.to_le_bytes() );
    }

    fn dds_header( width: u32, height: u32 ) -> Vec<u8>
    {
        let mut file = vec![ 0; PIXEL_DATA_AT ];
        put_bytes( &mut file, MAGIC_AT, MAGIC );
        put_u32( &mut file, HEADER_SIZE_AT, HEADER_SIZE );
        put_u32( &mut file, HEADER_FLAGS_AT, DDSD_CAPS | DDSD_HEIGHT | DDSD_WIDTH | DDSD_PIXELFORMAT );
        put_u32( &mut file, HEIGHT_AT, height );
        put_u32( &mut file, WIDTH_AT, width );
        put_u32( &mut file, PIXEL_FORMAT_SIZE_AT, PIXEL_FORMAT_SIZE );
        put_u32( &mut file, CAPS_AT, DDSCAPS_TEXTURE );
        file
    }

    fn fourcc_header( width: u32, height: u32, fourcc: &[u8; 4] ) -> Vec<u8>
    {
        let mut file = dds_header( width, height );
        put_u32( &mut file, PIXEL_FORMAT_FLAGS_AT, DDPF_FOURCC );
        put_bytes( &mut file, FOURCC_AT, fourcc );
        file
    }

    fn bgra8_header( width: u32, height: u32 ) -> Vec<u8>
    {
        let mut file = dds_header( width, height );
        put_u32( &mut file, PIXEL_FORMAT_FLAGS_AT, DDPF_RGB | DDPF_ALPHAPIXELS );
        put_u32( &mut file, BIT_COUNT_AT, 32 );
        put_u32( &mut file, RED_MASK_AT, 0x00ff_0000 );
        put_u32( &mut file, GREEN_MASK_AT, 0x0000_ff00 );
        put_u32( &mut file, BLUE_MASK_AT, 0x0000_00ff );
        put_u32( &mut file, ALPHA_MASK_AT, 0xff00_0000 );
        file
    }

    #[ test ]
    fn decodes_bgra8()
    {
        let bgra = [ 1, 2, 3, 4, 5, 6, 7, 8 ];
        let rgba = [ 3, 2, 1, 4, 7, 6, 5, 8 ];

        let mut file = bgra8_header( 2, 1 );
        file.extend_from_slice( &bgra );

        let tex = Texture::parse( &file ).unwrap();
        assert_eq!( tex.format(), Format::Bgra8 );
        assert_eq!( tex.decode( 0, 0 ).unwrap().rgba, rgba );
    }

    #[ test ]
    fn decodes_dxt1_solid_block()
    {
        let red_rgb565: u16 = 0b11111_000000_00000;
        let color0 = red_rgb565;
        let color1 = red_rgb565;
        let indices: u32 = 0;
        let opaque_red: [u8; 4] = [ 255, 0, 0, 255 ];

        let mut file = fourcc_header( 4, 4, b"DXT1" );
        file.extend_from_slice( &color0.to_le_bytes() );
        file.extend_from_slice( &color1.to_le_bytes() );
        file.extend_from_slice( &indices.to_le_bytes() );

        let tex = Texture::parse( &file ).unwrap();
        let image = tex.decode( 0, 0 ).unwrap();
        assert_eq!( image.rgba, opaque_red.repeat( 4 * 4 ) );

        let block = &file[ PIXEL_DATA_AT.. ];
        assert!( tex.format().is_block_compressed() );
        assert_eq!( tex.level( 0, 0 ).unwrap(), Level { width: 4, height: 4, data: block } );
    }

    #[ test ]
    fn rejects_truncated_data()
    {
        let file = fourcc_header( 4, 4, b"DXT5" );
        assert!( matches!( Texture::parse( &file ).unwrap().decode( 0, 0 ), Err( Error::Truncated ) ) );
    }

    #[ test ]
    fn rejects_oversized_dimensions()
    {
        let bgra8 = Texture::parse( &bgra8_header( u32::MAX, u32::MAX ) ).unwrap();
        assert!( matches!( bgra8.decode( 0, 0 ), Err( Error::TooLarge ) ) );

        let dxt1 = Texture::parse( &fourcc_header( u32::MAX, u32::MAX, b"DXT1" ) ).unwrap();
        assert!( matches!( dxt1.decode( 0, 0 ), Err( Error::TooLarge | Error::Truncated ) ) );
    }
}
