//! Binary actor descriptions from `Actors.vfs`, stored under the actor's name without extension.
//!
//! ```text
//! u32 kind                    0 for scene geometry; lights, sounds and others use other kinds
//! kind 0:
//!   f32x3 center, f32x3 half_extents   axis-aligned bounds of the mesh, padded by 0.1
//!   f32x3 anchor                       meaning not established
//!   u8 length, mesh file name, cp1251
//!   collision data                     two zlib blocks, not parsed
//! kind 1, light, and kind 2, group of lights:
//!   u32 color                          D3DCOLOR, bytes BGRA
//!   light                              for a group apparently its center
//!   u32                                meaning not established, looks like an id
//!   u32 coronas, coronas * { u32 0, u8 length, texture name, f32 size, f32x3 position }
//!   kind 2: u32 n, n * light           sharing the color
//! kind 7, water:
//!   u8 length, mesh file name, cp1251
//!   footprint
//! kind 15, vegetation:
//!   u8                                 meaning not established, 0
//!   u8 length, mesh file name, cp1251
//!   footprint
//!
//! light: f32x3 position, f32x2 params, f32 radius
//!
//! footprint: oriented bounds of the mesh in the XZ plane
//!   f32x2 center, f32x2 axes[2], f32x2 half_extents
//!
//! terrain actor, named by `<Land>`, and interior actor, named by `<Level>`, without a kind:
//!   u8 length, mesh file name, cp1251
//!   collision data                     zlib blocks, not parsed
//! ```
//!
//! The layouts cannot be told apart reliably, so terrain and interior actors are read by [`LandActor::parse`].

use encoding_rs::WINDOWS_1251;

pub const GEOMETRY_KIND:    u32 = 0;
pub const LIGHT_KIND:       u32 = 1;
pub const LIGHT_GROUP_KIND: u32 = 2;
pub const WATER_KIND:       u32 = 7;
pub const VEGETATION_KIND:  u32 = 15;

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

/// A light, or one light of a group.
#[ derive( Debug, Clone, Copy, PartialEq ) ]
pub struct PointLight
{
    pub position: [f32; 3],

    /// Meaning not established: the first is 2e-6 to 5e-5, maybe quadratic attenuation;
    /// the second is almost always 40/255.
    pub params: [f32; 2],

    pub radius: f32,
}

/// Glow sprite drawn at a light.
#[ derive( Debug, Clone, PartialEq ) ]
pub struct Corona
{
    /// File name in `Textures.vfs`, `corona.tex` in the game.
    pub texture:  String,
    pub size:     f32,
    pub position: [f32; 3],
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct LightActor
{
    /// RGBA.
    pub color: [u8; 4],

    /// The light itself; for a group apparently its center and extent.
    pub light: PointLight,

    /// Meaning not established, looks like an identifier.
    pub id: u32,

    pub coronas: Vec<Corona>,

    /// Lights of a group, sharing `color`; `None` for a single light.
    pub group: Option<Vec<PointLight>>,
}

/// Water or vegetation; the mesh is a water or detail mesh.
#[ derive( Debug, Clone, PartialEq ) ]
pub struct AreaActor
{
    /// File name in `Geometries.vfs`.
    pub mesh: String,

    /// Footprint in the XZ plane; `half_extents[i]` runs along `axes[i]`.
    pub center:       [f32; 2],
    pub axes:         [[f32; 2]; 2],
    pub half_extents: [f32; 2],
}

/// The terrain actor, see [`crate::Scene::land`], or the interior one, see [`crate::Scene::level`].
#[ derive( Debug, Clone, PartialEq ) ]
pub struct LandActor
{
    /// File name in `Geometries.vfs`: `terra.mesh` for the city, an interior mesh for a level.
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
    Light( LightActor ),
    Water( AreaActor ),
    Vegetation( AreaActor ),
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

    fn vector<const N: usize>( &mut self ) -> Result<[f32; N]>
    {
        let mut vector = [ 0.0; N ];
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

    fn point_light( &mut self ) -> Result<PointLight>
    {
        let light = PointLight
        {
            position: self.vector()?,
            params:   self.vector()?,
            radius:   self.vector::<1>()?[ 0 ],
        };
        Ok( light )
    }

    fn corona( &mut self ) -> Result<Corona>
    {
        self.u32()?;
        let corona = Corona
        {
            texture:  self.string()?,
            size:     self.vector::<1>()?[ 0 ],
            position: self.vector()?,
        };
        Ok( corona )
    }

    fn light( &mut self, is_group: bool ) -> Result<LightActor>
    {
        let [ blue, green, red, alpha ] = self.u32()?.to_le_bytes();
        let light = self.point_light()?;
        let id = self.u32()?;
        let coronas = ( 0..self.u32()? ).map( |_| self.corona() ).collect::<Result<_>>()?;
        let group = is_group.then( || ( 0..self.u32()? ).map( |_| self.point_light() ).collect::<Result<_>>() ).transpose()?;

        let actor = LightActor { color: [ red, green, blue, alpha ], light, id, coronas, group };
        Ok( actor )
    }

    fn area( &mut self ) -> Result<AreaActor>
    {
        let area = AreaActor
        {
            mesh:         self.string()?,
            center:       self.vector()?,
            axes:         [ self.vector()?, self.vector()? ],
            half_extents: self.vector()?,
        };
        Ok( area )
    }
}

impl ActorFile
{
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        let mut reader = Reader { data, pos: 0 };
        let actor = match reader.u32()?
        {
            GEOMETRY_KIND =>
            {
                let geometry = GeometryActor
                {
                    center:       reader.vector()?,
                    half_extents: reader.vector()?,
                    anchor:       reader.vector()?,
                    mesh:         reader.string()?,
                };
                Self::Geometry( geometry )
            }
            LIGHT_KIND => Self::Light( reader.light( false )? ),
            LIGHT_GROUP_KIND => Self::Light( reader.light( true )? ),
            WATER_KIND => Self::Water( reader.area()? ),
            VEGETATION_KIND =>
            {
                reader.bytes( 1 )?;
                Self::Vegetation( reader.area()? )
            }
            kind => Self::Other { kind },
        };
        Ok( actor )
    }

    pub fn mesh( &self ) -> Option<&str>
    {
        match self
        {
            Self::Geometry( GeometryActor { mesh, .. } )
            | Self::Water( AreaActor { mesh, .. } )
            | Self::Vegetation( AreaActor { mesh, .. } ) => Some( mesh ),
            Self::Light( _ ) | Self::Other { .. } => None,
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
    fn parses_vegetation_actor()
    {
        let mut data = VEGETATION_KIND.to_le_bytes().to_vec();
        data.extend_from_slice( &[ 0, 15 ] );
        data.extend_from_slice( b"arena_weed.mesh" );
        for value in [ 28223.5, 19227.5, 1.0, 0.0, 0.0, 1.0, 1207.0, 1195.25f32 ]
        {
            data.extend_from_slice( &value.to_le_bytes() );
        }

        let actor = ActorFile::parse( &data ).unwrap();
        assert_eq!( actor.mesh(), Some( "arena_weed.mesh" ) );
        let ActorFile::Vegetation( area ) = actor else { panic!( "expected vegetation" ) };
        assert_eq!( ( area.center, area.axes, area.half_extents ), ( [ 28223.5, 19227.5 ], [ [ 1.0, 0.0 ], [ 0.0, 1.0 ] ], [ 1207.0, 1195.25 ] ) );
    }

    fn floats( data: &mut Vec<u8>, values: &[f32] )
    {
        for value in values
        {
            data.extend_from_slice( &value.to_le_bytes() );
        }
    }

    #[ test ]
    fn parses_light_with_corona()
    {
        let mut data = LIGHT_KIND.to_le_bytes().to_vec();
        data.extend_from_slice( &[ 0x94, 0xdd, 0xf5, 0xff ] );
        floats( &mut data, &[ 1.0, 2.0, 3.0, 2e-5, 0.15625, 733.0 ] );
        data.extend_from_slice( &[ 7, 0, 0, 0, 1, 0, 0, 0 ] );
        data.extend_from_slice( &[ 0, 0, 0, 0, 10 ] );
        data.extend_from_slice( b"corona.tex" );
        floats( &mut data, &[ 75.0, 1.0, 2.5, 3.0 ] );

        let ActorFile::Light( light ) = ActorFile::parse( &data ).unwrap() else { panic!( "expected light" ) };
        assert_eq!( light.color, [ 0xf5, 0xdd, 0x94, 0xff ] );
        assert_eq!( light.light, PointLight { position: [ 1.0, 2.0, 3.0 ], params: [ 2e-5, 0.15625 ], radius: 733.0 } );
        assert_eq!( light.id, 7 );
        assert_eq!( light.coronas, [ Corona { texture: "corona.tex".into(), size: 75.0, position: [ 1.0, 2.5, 3.0 ] } ] );
        assert_eq!( light.group, None );
    }

    #[ test ]
    fn parses_light_group()
    {
        let mut data = LIGHT_GROUP_KIND.to_le_bytes().to_vec();
        data.extend_from_slice( &[ 0xff; 4 ] );
        floats( &mut data, &[ 0.0, 0.0, 0.0, 1e-6, 0.15625, 1939.0 ] );
        data.extend_from_slice( &[ 0; 8 ] );
        data.extend_from_slice( &2u32.to_le_bytes() );
        floats( &mut data, &[ -1.0, 0.0, 0.0, 1e-6, 0.15625, 898.0, 1.0, 0.0, 0.0, 1e-6, 0.15625, 898.0 ] );

        let ActorFile::Light( light ) = ActorFile::parse( &data ).unwrap() else { panic!( "expected light" ) };
        let positions: Vec<[f32; 3]> = light.group.unwrap().iter().map( |light| light.position ).collect();
        assert_eq!( positions, [ [ -1.0, 0.0, 0.0 ], [ 1.0, 0.0, 0.0 ] ] );
        assert!( matches!( ActorFile::parse( &data[ ..data.len() - 1 ] ), Err( Error::Truncated ) ) );
    }

    #[ test ]
    fn keeps_other_kinds()
    {
        let kind = 6u32;
        assert_eq!( ActorFile::parse( &kind.to_le_bytes() ).unwrap(), ActorFile::Other { kind } );
        assert!( matches!( ActorFile::parse( &[ 0, 0 ] ), Err( Error::Truncated ) ) );
    }
}
