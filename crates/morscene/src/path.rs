//! Path finding data (`.pfd`, named by `<PathFinding DataFile>`): a navigation mesh of convex
//! cells on floor planes, a graph over their edges, and a grid for looking cells up by position.
//!
//! ```text
//! u32 compressed_length, zlib stream, unpacked:
//!
//! u32 v, v * f32x3 vertex
//! u32 e, e * { u32 from, u32 to, u32 cell, u32 k, k * { u32 edge, f32 distance } } half-edge
//! u32 p, p * f32x3 point
//! u32 c, c * { u32 plane, u32 n, n * u32 vertex, f32 area, n * f32x3 edge line, u32 m, m * u32x2 }
//! c * u16 cell value
//! u32 f, f * { f32x4 plane, f32x3, f32x3 }
//! u32 columns, u32 rows, f32x2 origin, f32x2 grid cell size
//! rows * columns * { u32 n, n * u32 cell, u32 r, r * u32 run length }
//! ```
//!
//! An edge line is `[a, b, c]` with `a * x + b * z + c` zero on the edge, in the XZ plane.
//! The graph links each half-edge to the edges reachable from it, the distance between their midpoints.

use crate::binary::{ COUNT_SIZE, Error, Reader, Result, VEC3_SIZE, check_index };

const HALF_EDGE_MIN_SIZE: usize = 4 * COUNT_SIZE;
const LINK_SIZE:          usize = COUNT_SIZE + size_of::<f32>();
const CELL_MIN_SIZE:      usize = ( 3 * COUNT_SIZE ) + size_of::<f32>();
const PAIR_SIZE:          usize = 2 * COUNT_SIZE;
const PLANE_SIZE:         usize = size_of::<[f32; 4]>() + ( 2 * VEC3_SIZE );
const GRID_CELL_MIN_SIZE: usize = 2 * COUNT_SIZE;

#[ derive( Debug, Clone, Copy, PartialEq ) ]
pub struct EdgeLink
{
    pub edge:     u32,
    pub distance: f32,
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct HalfEdge
{
    pub from:  u32,
    pub to:    u32,
    pub cell:  u32,
    pub links: Vec<EdgeLink>,
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct Cell
{
    /// Index into [`PathFinding::planes`].
    pub plane: u32,

    pub vertices: Vec<u32>,
    pub area:     f32,

    /// One per side, see the module docs.
    pub lines: Vec<[f32; 3]>,

    /// Pairs of half-edge indices; meaning not established.
    pub pairs: Vec<[u32; 2]>,

    /// Up to a few dozen; meaning not established.
    pub value: u16,
}

#[ derive( Debug, Clone, Copy, PartialEq ) ]
pub struct FloorPlane
{
    /// `[a, b, c, d]` with `a * x + b * y + c * z + d` zero on the plane.
    pub plane: [f32; 4],

    /// Unit vectors; meaning not established.
    pub axes: [[f32; 3]; 2],
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct GridCell
{
    /// Cells under this grid cell, one list per floor plane.
    pub layers: Vec<Vec<u32>>,
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct PathFinding
{
    pub vertices: Vec<[f32; 3]>,
    pub edges:    Vec<HalfEdge>,

    /// Meaning not established.
    pub points: Vec<[f32; 3]>,

    pub cells:  Vec<Cell>,
    pub planes: Vec<FloorPlane>,

    pub columns:   u32,
    pub rows:      u32,
    pub origin:    [f32; 2],
    pub cell_size: [f32; 2],

    /// `rows * columns`, row by row.
    pub grid: Vec<GridCell>,
}

impl PathFinding
{
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        let mut packed = Reader::new( data );
        let unpacked = packed.zlib()?;
        packed.finish()?;

        let mut reader = Reader::new( &unpacked );
        let vertices = reader.counted_items( VEC3_SIZE, Reader::floats )?;
        let edges = reader.counted_items( HALF_EDGE_MIN_SIZE, |reader| read_half_edge( reader, vertices.len() ) )?;
        let points = reader.counted_items( VEC3_SIZE, Reader::floats )?;
        let mut cells = reader.counted_items( CELL_MIN_SIZE, |reader| read_cell( reader, vertices.len(), edges.len() ) )?;
        for cell in &mut cells
        {
            cell.value = reader.u16()?;
        }

        let planes = reader.counted_items( PLANE_SIZE, read_plane )?;
        let columns = reader.u32()?;
        let rows = reader.u32()?;
        let origin = reader.floats()?;
        let cell_size = reader.floats()?;
        let grid_count = usize::try_from( u64::from( rows ) * u64::from( columns ) ).map_err( |_| Error::TooLarge )?;
        let grid = reader.items( grid_count, GRID_CELL_MIN_SIZE, |reader| read_grid_cell( reader, cells.len() ) )?;
        reader.finish()?;

        check_links( &edges, cells.len() )?;
        for cell in &cells
        {
            check_index( "plane", cell.plane, planes.len() )?;
        }

        let path_finding = Self { vertices, edges, points, cells, planes, columns, rows, origin, cell_size, grid };
        Ok( path_finding )
    }
}

fn read_half_edge( reader: &mut Reader, vertex_count: usize ) -> Result<HalfEdge>
{
    let from = check_index( "vertex", reader.u32()?, vertex_count )?;
    let to = check_index( "vertex", reader.u32()?, vertex_count )?;
    let cell = reader.u32()?;
    let links = reader.counted_items( LINK_SIZE, |reader| Ok( EdgeLink { edge: reader.u32()?, distance: reader.f32()? } ) )?;
    Ok( HalfEdge { from, to, cell, links } )
}

/// Cells and links point forward, so they are checked once everything is read.
fn check_links( edges: &[HalfEdge], cell_count: usize ) -> Result<()>
{
    let edge_count = edges.len();
    for edge in edges
    {
        check_index( "cell", edge.cell, cell_count )?;
        for link in &edge.links
        {
            check_index( "edge", link.edge, edge_count )?;
        }
    }
    Ok( () )
}

fn read_cell( reader: &mut Reader, vertex_count: usize, edge_count: usize ) -> Result<Cell>
{
    let plane = reader.u32()?;
    let vertices = reader.counted_items( COUNT_SIZE, |reader| check_index( "vertex", reader.u32()?, vertex_count ) )?;
    let area = reader.f32()?;
    let lines = reader.items( vertices.len(), VEC3_SIZE, Reader::floats )?;
    let pairs = reader.counted_items( PAIR_SIZE, |reader|
    {
        let pair = [ check_index( "edge", reader.u32()?, edge_count )?, check_index( "edge", reader.u32()?, edge_count )? ];
        Ok( pair )
    } )?;

    let cell = Cell { plane, vertices, area, lines, pairs, value: 0 };
    Ok( cell )
}

fn read_plane( reader: &mut Reader ) -> Result<FloorPlane>
{
    Ok( FloorPlane { plane: reader.floats()?, axes: [ reader.floats()?, reader.floats()? ] } )
}

fn read_grid_cell( reader: &mut Reader, cell_count: usize ) -> Result<GridCell>
{
    let cells = reader.counted_items( COUNT_SIZE, |reader| check_index( "cell", reader.u32()?, cell_count ) )?;
    let runs = reader.counted_items( COUNT_SIZE, Reader::count )?;

    let mut layers = Vec::with_capacity( runs.len() );
    let mut rest = cells.as_slice();
    for run in runs
    {
        let ( layer, after ) = rest.split_at_checked( run ).ok_or( Error::Truncated )?;
        layers.push( layer.to_vec() );
        rest = after;
    }
    if !rest.is_empty()
    {
        return Err( Error::TrailingData( rest.len() ) );
    }
    Ok( GridCell { layers } )
}
