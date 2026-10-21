//! Scene model for Pathologic (Mor. Utopia): `.isc` interior and `.osc` outdoor scenes.
//!
//! A scene is XML read as leniently as the game reads it: end tags are not matched, anything
//! after the root element is ignored, attribute values are taken raw (names hold bare `&`).
//!
//! ```text
//! <Scene>
//!   <Name>house1_arc_01r</Name>
//!   <Land>zterra</Land>                                   outdoor: terrain actor
//!   <PathFinding DataFile = "city_base.osc.pfd"/>          see [`path`]
//!   <Map FileName="city.map"/>                            outdoor, see [`map`]
//!   <reference filename="house1_base.isc" node="Scene"/>  included in place
//!   <reference filename="music_nailed.xml" node="Music"/> kept, not followed
//!   <Point Name = "birth" Position = "x y z" Direction = "x y z"/>
//!   <Actor Name = "Bench01" StaticID = "12" Extras = "bench.xml" Location = "pt_x" Type = "scripted"/>
//! </Scene>
//! ```
//!
//! An actor name is also the name of its binary description in `Actors.vfs`, see [`actor`].

use encoding_rs::WINDOWS_1251;
use quick_xml::events::{ BytesStart, Event };

pub mod actor;
pub mod binary;
pub mod map;
pub mod path;

pub const INTERIOR_EXTENSION: &str = "isc";
pub const OUTDOOR_EXTENSION:  &str = "osc";

const SCENE_NODE: &str = "Scene";

#[ derive( Debug, thiserror::Error ) ]
pub enum Error
{
    #[ error( "malformed XML: {0}" ) ]
    Xml( #[ from ] quick_xml::Error ),

    #[ error( "no <Scene> element" ) ]
    NoScene,

    #[ error( "<{element}> is missing attribute {attribute}" ) ]
    MissingAttribute { element: &'static str, attribute: &'static str },

    #[ error( "invalid {what}: {value:?}" ) ]
    InvalidValue { what: &'static str, value: String },

    #[ error( "referenced file not found: {0}" ) ]
    MissingFile( String ),

    #[ error( "reference cycle through {0}" ) ]
    ReferenceCycle( String ),
}

pub type Result<T> = std::result::Result<T, Error>;

#[ derive( Debug, Clone, PartialEq ) ]
pub struct Point
{
    pub name:      String,
    pub position:  [f32; 3],
    pub direction: [f32; 3],
}

/// Attributes as written; an actor without a name is placed by `location`.
#[ derive( Debug, Clone, Default, PartialEq, Eq ) ]
pub struct Actor
{
    pub name:      Option<String>,
    pub kind:      Option<String>,
    pub static_id: Option<u32>,
    pub location:  Option<String>,
    pub extras:    Option<String>,
}

#[ derive( Debug, Clone, PartialEq, Eq ) ]
pub struct Reference
{
    pub file: String,
    pub node: String,
}

#[ derive( Debug, Clone, Default, PartialEq ) ]
pub struct Scene
{
    pub name:         Option<String>,
    pub land:         Option<String>,
    pub path_finding: Option<String>,
    pub map:          Option<String>,
    pub points:       Vec<Point>,
    pub actors:       Vec<Actor>,

    /// References to other nodes than `Scene`, and before [`Scene::load`] all of them.
    pub references: Vec<Reference>,
}

fn attribute( element: &BytesStart, name: &str ) -> Option<String>
{
    element
        .attributes()
        .with_checks( false )
        .flatten()
        .find( |attribute| attribute.key.as_ref().eq_ignore_ascii_case( name ) )
        .map( |attribute| attribute.value.trim().to_owned() )
}

fn required( element: &BytesStart, element_name: &'static str, name: &'static str ) -> Result<String>
{
    attribute( element, name ).ok_or( Error::MissingAttribute { element: element_name, attribute: name } )
}

fn vector( what: &'static str, text: &str ) -> Result<[f32; 3]>
{
    let invalid = || Error::InvalidValue { what, value: text.to_owned() };
    let components: Vec<f32> = text.split_whitespace().map( str::parse ).collect::<std::result::Result<_, _>>().map_err( |_| invalid() )?;
    components.try_into().map_err( |_| invalid() )
}

impl Point
{
    fn from_element( element: &BytesStart ) -> Result<Self>
    {
        let point = Self
        {
            name:      required( element, "Point", "Name" )?,
            position:  vector( "position", &required( element, "Point", "Position" )? )?,
            direction: vector( "direction", &required( element, "Point", "Direction" )? )?,
        };
        Ok( point )
    }
}

impl Actor
{
    fn from_element( element: &BytesStart ) -> Result<Self>
    {
        let static_id = match attribute( element, "StaticID" )
        {
            Some( id ) => Some( id.parse().map_err( |_| Error::InvalidValue { what: "static id", value: id } )? ),
            None => None,
        };

        let actor = Self
        {
            name:     attribute( element, "Name" ),
            kind:     attribute( element, "Type" ),
            static_id,
            location: attribute( element, "Location" ),
            extras:   attribute( element, "Extras" ),
        };
        Ok( actor )
    }
}

/// Element whose text is read: `<Name>` or `<Land>`.
#[ derive( Clone, Copy ) ]
enum TextField
{
    Name,
    Land,
}

impl Scene
{
    /// One file as written; references are not followed.
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        let ( text, _had_errors ) = WINDOWS_1251.decode_without_bom_handling( data );
        let mut reader = quick_xml::Reader::from_str( &text );
        reader.config_mut().check_end_names = false;

        let mut scene = None;
        let mut depth = 0;
        let mut text_field = None;
        loop
        {
            let event = reader.read_event()?;
            let is_scene_child = depth == 1 && scene.is_some();
            match event
            {
                Event::Start( element ) if depth == 0 && element.name().as_ref() == SCENE_NODE =>
                {
                    scene = Some( Self::default() );
                    depth += 1;
                }
                Event::Start( element ) if is_scene_child =>
                {
                    text_field = match element.name().as_ref()
                    {
                        "Name" => Some( TextField::Name ),
                        "Land" => Some( TextField::Land ),
                        _ => None,
                    };
                    depth += 1;
                }
                Event::Empty( element ) if is_scene_child =>
                {
                    if let Some( scene ) = scene.as_mut()
                    {
                        scene.add_element( &element )?;
                    }
                }
                Event::Text( text ) if depth == 2 =>
                {
                    if let ( Some( field ), Some( scene ) ) = ( text_field, scene.as_mut() )
                    {
                        let value = Some( text.trim().to_owned() );
                        match field
                        {
                            TextField::Name => scene.name = value,
                            TextField::Land => scene.land = value,
                        }
                    }
                }
                Event::Start( _ ) => depth += 1,
                Event::End( _ ) =>
                {
                    depth -= 1;
                    text_field = None;
                    if depth == 0 && scene.is_some()
                    {
                        break;
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        scene.ok_or( Error::NoScene )
    }

    fn add_element( &mut self, element: &BytesStart ) -> Result<()>
    {
        match element.name().as_ref()
        {
            "Point" => self.points.push( Point::from_element( element )? ),
            "Actor" => self.actors.push( Actor::from_element( element )? ),
            "PathFinding" => self.path_finding = attribute( element, "DataFile" ),
            "Map" => self.map = attribute( element, "FileName" ),
            "reference" =>
            {
                let reference = Reference { file: required( element, "reference", "filename" )?, node: required( element, "reference", "node" )? };
                self.references.push( reference );
            }
            _ => {}
        }
        Ok( () )
    }

    /// `file` with its `Scene` references included in place, depth first.
    /// The including file's name, land, path finding and map win over those it includes.
    pub fn load( file: &str, mut read: impl FnMut( &str ) -> Option<Vec<u8>> ) -> Result<Self>
    {
        let mut chain = Vec::new();
        Self::load_into( file, &mut read, &mut chain )
    }

    fn load_into( file: &str, read: &mut impl FnMut( &str ) -> Option<Vec<u8>>, chain: &mut Vec<String> ) -> Result<Self>
    {
        let key = file.to_lowercase();
        if chain.contains( &key )
        {
            return Err( Error::ReferenceCycle( file.to_owned() ) );
        }

        let data = read( file ).ok_or_else( || Error::MissingFile( file.to_owned() ) )?;
        let own = Self::parse( &data )?;
        chain.push( key );

        let mut merged = Self::default();
        for reference in &own.references
        {
            if reference.node == SCENE_NODE
            {
                let included = Self::load_into( &reference.file, read, chain )?;
                merged.absorb( included );
            }
            else
            {
                merged.references.push( reference.clone() );
            }
        }
        chain.pop();

        let Self { name, land, path_finding, map, points, actors, references: _ } = own;
        let own_body = Self { name, land, path_finding, map, points, actors, references: Vec::new() };
        merged.absorb( own_body );
        Ok( merged )
    }

    /// Appends `other`; its scalar fields replace ours when present.
    fn absorb( &mut self, other: Self )
    {
        self.name = other.name.or( self.name.take() );
        self.land = other.land.or( self.land.take() );
        self.path_finding = other.path_finding.or( self.path_finding.take() );
        self.map = other.map.or( self.map.take() );
        self.points.extend( other.points );
        self.actors.extend( other.actors );
        self.references.extend( other.references );
    }
}

#[ cfg( test ) ]
mod tests
{
    use super::*;

    const BASE: &str = r#"<Scene>
        <Name>base</Name>
        <Land>zterra</Land>
        <PathFinding DataFile = "base.pfd"/>
        <Point Name = "birth" Position = "1 2.5 -3" Direction = "0 0 1"/>
        <Actor Name = "kachely&pesochnitsa_01" StaticID = "7"/>
    </Scene>
    <Cache>
        <GeomAnimBone Name = "pers_boy.mesh"></GeomAnimBone>
    </Cache>"#;

    const HERO: &str = r#"<Scene>
        <reference filename="base.osc" node="Scene"/>
        <reference filename="music.xml" node="Music"/>
        <Name>hero</Name>
        <Actor Type="scripted" Location="birth" Extras="bonfire.xml"/>
    </Scene>"#;

    fn files( name: &str ) -> Option<Vec<u8>>
    {
        match name
        {
            "base.osc" => Some( BASE.as_bytes().to_vec() ),
            "hero.osc" => Some( HERO.as_bytes().to_vec() ),
            "loop.osc" => Some( br#"<Scene><reference filename="LOOP.osc" node="Scene"/></Scene>"#.to_vec() ),
            _ => None,
        }
    }

    #[ test ]
    fn parses_one_file_leniently()
    {
        let scene = Scene::parse( BASE.as_bytes() ).unwrap();
        assert_eq!( ( scene.name.as_deref(), scene.land.as_deref() ), ( Some( "base" ), Some( "zterra" ) ) );
        assert_eq!( scene.path_finding.as_deref(), Some( "base.pfd" ) );
        assert_eq!( scene.points, [ Point { name: "birth".into(), position: [ 1.0, 2.5, -3.0 ], direction: [ 0.0, 0.0, 1.0 ] } ] );
        assert_eq!( scene.actors[ 0 ].name.as_deref(), Some( "kachely&pesochnitsa_01" ) );
        assert_eq!( scene.actors[ 0 ].static_id, Some( 7 ) );
    }

    #[ test ]
    fn includes_scene_references_in_place()
    {
        let scene = Scene::load( "hero.osc", files ).unwrap();
        assert_eq!( ( scene.name.as_deref(), scene.land.as_deref() ), ( Some( "hero" ), Some( "zterra" ) ) );
        assert_eq!( scene.points.len(), 1 );

        let kinds: Vec<_> = scene.actors.iter().map( |actor| actor.kind.as_deref() ).collect();
        assert_eq!( kinds, [ None, Some( "scripted" ) ] );
        assert_eq!( scene.references, [ Reference { file: "music.xml".into(), node: "Music".into() } ] );
    }

    #[ test ]
    fn reports_missing_files_and_cycles()
    {
        assert!( matches!( Scene::load( "absent.osc", files ), Err( Error::MissingFile( name ) ) if name == "absent.osc" ) );
        assert!( matches!( Scene::load( "loop.osc", files ), Err( Error::ReferenceCycle( _ ) ) ) );
    }

    #[ test ]
    fn rejects_bad_point()
    {
        let xml = br#"<Scene><Point Name = "p" Position = "1 2" Direction = "0 0 1"/></Scene>"#;
        assert!( matches!( Scene::parse( xml ), Err( Error::InvalidValue { what: "position", .. } ) ) );
    }
}
