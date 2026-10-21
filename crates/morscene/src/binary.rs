//! Reader shared by the binary scene files: path finding data and the city map.

use std::io::Read as _;

use encoding_rs::WINDOWS_1251;
use flate2::read::ZlibDecoder;

#[ derive( Debug, thiserror::Error ) ]
pub enum Error
{
    #[ error( "data is truncated" ) ]
    Truncated,

    #[ error( "{0} trailing bytes after the data" ) ]
    TrailingData( usize ),

    #[ error( "{what} index {index} out of range for {count} items" ) ]
    IndexOutOfRange { what: &'static str, index: u32, count: usize },

    #[ error( "zlib stream is corrupt" ) ]
    CorruptStream,

    #[ error( "invalid {0}" ) ]
    Invalid( &'static str ),

    #[ error( "data is too large" ) ]
    TooLarge,
}

pub type Result<T> = std::result::Result<T, Error>;

pub( crate ) const COUNT_SIZE: usize = size_of::<u32>();
pub( crate ) const VEC2_SIZE:  usize = size_of::<[f32; 2]>();
pub( crate ) const VEC3_SIZE:  usize = size_of::<[f32; 3]>();

pub( crate ) fn to_usize( value: u32 ) -> Result<usize>
{
    usize::try_from( value ).map_err( |_| Error::TooLarge )
}

pub( crate ) fn check_index( what: &'static str, index: u32, count: usize ) -> Result<u32>
{
    if to_usize( index )? < count
    {
        return Ok( index );
    }
    Err( Error::IndexOutOfRange { what, index, count } )
}

pub( crate ) struct Reader<'a>
{
    data: &'a [u8],
    pos:  usize,
}

impl<'a> Reader<'a>
{
    pub( crate ) fn new( data: &'a [u8] ) -> Self
    {
        Self { data, pos: 0 }
    }

    pub( crate ) fn bytes( &mut self, len: usize ) -> Result<&'a [u8]>
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

    pub( crate ) fn u8( &mut self ) -> Result<u8>
    {
        Ok( self.bytes( 1 )?[ 0 ] )
    }

    pub( crate ) fn bool( &mut self ) -> Result<bool>
    {
        Ok( self.u8()? != 0 )
    }

    pub( crate ) fn u16( &mut self ) -> Result<u16>
    {
        Ok( u16::from_le_bytes( self.array()? ) )
    }

    pub( crate ) fn u32( &mut self ) -> Result<u32>
    {
        Ok( u32::from_le_bytes( self.array()? ) )
    }

    pub( crate ) fn f32( &mut self ) -> Result<f32>
    {
        Ok( f32::from_le_bytes( self.array()? ) )
    }

    pub( crate ) fn floats<const N: usize>( &mut self ) -> Result<[f32; N]>
    {
        let mut floats = [ 0.0; N ];
        for value in &mut floats
        {
            *value = self.f32()?;
        }
        Ok( floats )
    }

    pub( crate ) fn count( &mut self ) -> Result<usize>
    {
        to_usize( self.u32()? )
    }

    /// Reads `count` items of at least `item_size` bytes; the capacity is bounded by the remaining data.
    pub( crate ) fn items<T>( &mut self, count: usize, item_size: usize, mut read_item: impl FnMut( &mut Self ) -> Result<T> ) -> Result<Vec<T>>
    {
        let mut items = Vec::with_capacity( count.min( self.remaining() / item_size ) );
        for _ in 0..count
        {
            items.push( read_item( self )? );
        }
        Ok( items )
    }

    pub( crate ) fn counted_items<T>( &mut self, item_size: usize, read_item: impl FnMut( &mut Self ) -> Result<T> ) -> Result<Vec<T>>
    {
        let count = self.count()?;
        self.items( count, item_size, read_item )
    }

    /// `u8` length, cp1251.
    pub( crate ) fn string( &mut self ) -> Result<String>
    {
        let len = usize::from( self.u8()? );
        let ( text, _had_errors ) = WINDOWS_1251.decode_without_bom_handling( self.bytes( len )? );
        Ok( text.into_owned() )
    }

    /// `u32 compressed_length, zlib stream`.
    pub( crate ) fn zlib( &mut self ) -> Result<Vec<u8>>
    {
        let compressed_length = self.count()?;
        let mut decoder = ZlibDecoder::new( self.bytes( compressed_length )? );
        let mut unpacked = Vec::new();
        decoder.read_to_end( &mut unpacked ).map_err( |_| Error::CorruptStream )?;
        if decoder.total_in() != u64::try_from( compressed_length ).map_err( |_| Error::TooLarge )?
        {
            return Err( Error::CorruptStream );
        }
        Ok( unpacked )
    }

    pub( crate ) fn remaining( &self ) -> usize
    {
        self.data.len() - self.pos
    }

    pub( crate ) fn finish( &self ) -> Result<()>
    {
        match self.remaining()
        {
            0 => Ok( () ),
            rest => Err( Error::TrailingData( rest ) ),
        }
    }
}
