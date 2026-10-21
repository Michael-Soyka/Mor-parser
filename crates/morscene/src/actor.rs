//! Binary actor descriptions from `Actors.vfs`, stored under the actor's name without extension.
//!
//! ```text
//! u32 kind                    0 for scene geometry; lights, sounds and others use other kinds
//! kind 0:
//!   f32x3 center, f32x3 half_extents   axis-aligned bounds of the mesh, padded by 0.1
//!   f32x3 anchor                       meaning not established
//!   u8 length, mesh file name, cp1251
//!   collision data                     two zlib blocks, not parsed
//!
//! terrain actor, named by `<Land>`, without a kind:
//!   u8 length, mesh file name, cp1251
//!   collision data                     zlib blocks, not parsed
//! ```
//!
//! The two layouts cannot be told apart reliably, so the terrain actor is read by [`LandActor::parse`].

use encoding_rs::WINDOWS_1251;

pub const GEOMETRY_KIND: u32 = 0;

#[ derive( Debug, thiserror::Error ) ]
pub enum Error
{
    #[ error( "actor data is truncated" ) ]
    Truncated,
}

pub type Result<T> = std::result::Result<T, Error>;

#[ derive( Debug, Clone, PartialEq ) ]
pub struct GeometryActor
{
    pub center:       [f32; 3],
    pub half_extents: [f32; 3],
    pub anchor:       [f32; 3],

    /// File name in `Geometries.vfs`, normally ending with `.mesh`.
    pub mesh: String,
}

/// The terrain actor, see [`crate::Scene::land`].
#[ derive( Debug, Clone, PartialEq ) ]
pub struct LandActor
{
    /// File name in `Geometries.vfs`, `terra.mesh` for the city.
    pub mesh: String,
}

impl LandActor
{
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        let mut reader = Reader { data, pos: 0 };
        Ok( Self { mesh: reader.string()? } )
    }
}

#[ derive( Debug, Clone, PartialEq ) ]
pub enum ActorFile
{
    Geometry( GeometryActor ),
    Other { kind: u32 },
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
        let bytes = self.data.get( self.pos..self.pos + len ).ok_or( Error::Truncated )?;
        self.pos += len;
        Ok( bytes )
    }

    fn u32( &mut self ) -> Result<u32>
    {
        let mut bytes = [ 0; 4 ];
        bytes.copy_from_slice( self.bytes( size_of::<u32>() )? );
        Ok( u32::from_le_bytes( bytes ) )
    }

    fn vector( &mut self ) -> Result<[f32; 3]>
    {
        let mut vector = [ 0.0; 3 ];
        for component in &mut vector
        {
            *component = f32::from_bits( self.u32()? );
        }
        Ok( vector )
    }

    fn string( &mut self ) -> Result<String>
    {
        let len = usize::from( self.bytes( 1 )?[ 0 ] );
        let ( text, _had_errors ) = WINDOWS_1251.decode_without_bom_handling( self.bytes( len )? );
        Ok( text.into_owned() )
    }
}

impl ActorFile
{
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        let mut reader = Reader { data, pos: 0 };
        let kind = reader.u32()?;
        if kind != GEOMETRY_KIND
        {
            return Ok( Self::Other { kind } );
        }

        let geometry = GeometryActor
        {
            center:       reader.vector()?,
            half_extents: reader.vector()?,
            anchor:       reader.vector()?,
            mesh:         reader.string()?,
        };
        Ok( Self::Geometry( geometry ) )
    }

    pub fn mesh( &self ) -> Option<&str>
    {
        match self
        {
            Self::Geometry( geometry ) => Some( &geometry.mesh ),
            Self::Other { .. } => None,
        }
    }
}

#[ cfg( test ) ]
mod tests
{
    use super::*;

    #[ test ]
    fn parses_geometry_actor()
    {
        let mut data = GEOMETRY_KIND.to_le_bytes().to_vec();
        for value in [ 1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 1.0, 0.0, 3.0 ]
        {
            data.extend_from_slice( &value.to_le_bytes() );
        }
        data.push( 12 );
        data.extend_from_slice( b"Bench01.mesh" );
        data.extend_from_slice( &[ 1, 0, 0, 0, 0 ] );

        let actor = ActorFile::parse( &data ).unwrap();
        assert_eq!( actor.mesh(), Some( "Bench01.mesh" ) );
        let ActorFile::Geometry( geometry ) = actor else { panic!( "expected geometry" ) };
        assert_eq!( ( geometry.center, geometry.half_extents ), ( [ 1.0, 2.0, 3.0 ], [ 4.0, 5.0, 6.0 ] ) );
    }

    #[ test ]
    fn keeps_other_kinds()
    {
        let light_kind = 2u32;
        assert_eq!( ActorFile::parse( &light_kind.to_le_bytes() ).unwrap(), ActorFile::Other { kind: light_kind } );
        assert!( matches!( ActorFile::parse( &[ 0, 0 ] ), Err( Error::Truncated ) ) );
    }
}
