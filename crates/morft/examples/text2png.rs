//! Usage: text2png <Fonts.vfs> <name.ft> <text> <out.png>

use std::process::ExitCode;

use image::{ Rgba, RgbaImage };
use morft::{ Font, Glyph };
use morvfs::Vfs;

const INK: [u8; 3] = [ 0, 0, 0 ];

struct PlacedGlyph<'a>
{
    left:  i64,
    top:   i64,
    glyph: &'a Glyph,
}

impl PlacedGlyph<'_>
{
    fn right( &self ) -> i64
    {
        self.left + i64::from( self.glyph.width )
    }
}

struct Line<'a>
{
    glyphs:  Vec<PlacedGlyph<'a>>,
    advance: i64,
}

impl Line<'_>
{
    fn width( &self ) -> i64
    {
        self.glyphs.iter().map( PlacedGlyph::right ).fold( self.advance, i64::max )
    }
}

fn layout<'a>( font: &'a Font, text: &str ) -> Line<'a>
{
    let mut glyphs = Vec::new();
    let mut pen = 0;
    let mut previous_ch = None;
    for ch in text.chars()
    {
        let Some( glyph ) = font.glyph( ch ) else { continue };
        if let Some( previous ) = previous_ch
        {
            pen += i64::from( font.kerning( previous, ch ) );
        }

        let placed = PlacedGlyph { left: pen + i64::from( glyph.offset_x ), top: i64::from( glyph.offset_y ), glyph };
        glyphs.push( placed );
        pen += i64::from( glyph.advance );
        previous_ch = Some( ch );
    }
    Line { glyphs, advance: pen }
}

fn paint( image: &mut RgbaImage, x: i64, y: i64, coverage: u8 )
{
    let ( Ok( x ), Ok( y ) ) = ( u32::try_from( x ), u32::try_from( y ) ) else { return };
    let Some( pixel ) = image.get_pixel_mut_checked( x, y ) else { return };

    let Rgba( [ .., alpha ] ) = *pixel;
    let [ red, green, blue ] = INK;
    *pixel = Rgba( [ red, green, blue, alpha.max( coverage ) ] );
}

fn draw_glyph( image: &mut RgbaImage, placed: &PlacedGlyph )
{
    let glyph = placed.glyph;
    let Ok( width ) = usize::try_from( glyph.width ) else { return };
    if width == 0
    {
        return;
    }

    for ( y, row ) in ( placed.top.. ).zip( glyph.coverage.chunks_exact( width ) )
    {
        for ( x, &coverage ) in ( placed.left.. ).zip( row )
        {
            paint( image, x, y, coverage );
        }
    }
}

fn rasterize( line: &Line, width: u32, height: u32 ) -> RgbaImage
{
    let mut image = RgbaImage::new( width, height );
    for placed in &line.glyphs
    {
        draw_glyph( &mut image, placed );
    }
    image
}

fn run( args: &[String] ) -> Result<String, Box<dyn std::error::Error>>
{
    let [ archive, name, text, out ] = args else
    {
        return Err( "usage: text2png <Fonts.vfs> <name.ft> <text> <out.png>".into() );
    };

    let font = Font::parse( &Vfs::open( archive )?.read( name )? )?;
    let line = layout( &font, text );
    let width = u32::try_from( line.width().max( 1 ) )?;
    let height = font.line_height().max( 1 );
    rasterize( &line, width, height ).save( out )?;

    let summary = format!( "{name}: {} glyphs, {} kerning pairs, {width}x{height} -> {out}", font.glyphs().len(), font.kerning_pairs().len() );
    Ok( summary )
}

fn main() -> ExitCode
{
    let args: Vec<String> = std::env::args().skip( 1 ).collect();
    match run( &args )
    {
        Ok( summary ) =>
        {
            println!( "{summary}" );
            ExitCode::SUCCESS
        }
        Err( err ) =>
        {
            eprintln!( "{err}" );
            ExitCode::FAILURE
        }
    }
}
