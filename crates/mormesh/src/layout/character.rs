//! Characters (the engine's `CGeometryAnimBone` and `CGeometryAnimBoneSimple`): one skinned
//! surface with its skeleton, and the animations stored after the zlib stream.
//!
//! ```text
//! u32 compressed_length, zlib stream of compressed_length bytes, then animations if any
//!
//! unpacked:
//!   u32 bones, u32 vertex_count, u32 n, n B weights, u32 reserved
//!   texture, u8 flag, u32 vertex_count, u8 compressed, [f32x3 origin if compressed],
//!   vertex_count * position, vertex_count * u32 packed normal, vertex_count * f32x2 uv,
//!   u32 triangle_count, triangle_count * u16x3
//!   full:   triangle_count * u8 triangle part
//!   bones * bone
//!   full:   u32 m, m * { name, f32x4x4 transform, u32 bone } attachment
//!
//! weights: vertex_count * { u32 k, k * { u32 bone, f32 weight } }
//! bone:    [f32x4x4 bind pose: full], u32 parent or 0xFFFFFFFF, u32 c, c * u32 child or 0xFFFFFFFF,
//!          [u32 t, t * u32 triangle: full], u8 has_obb, [obb if has_obb]
//!
//! animations: u32 groups, groups * { name, u32 b, b * u32 bone, u32 a, a * animation }
//! animation:  name, u8 has_motion, f32x3 displacement, b * { u32 k, k * keyframe },
//!             [u32 m, m * f32x3 motion key if has_motion]
//! keyframe:   f32 time, f32x4 rotation, f32x3 position
//! ```
//!
//! Positions are in the bind pose, matrices are row-major with the translation in the last row.
//! Names, positions and normals follow the static mesh conventions, see the crate docs.

use crate::{
    COMPRESSED_POSITION_SIZE, COUNT_SIZE, Error, Obb, PACKED_SIZE, Reader, Result, UV_SIZE, VEC3_SIZE, check_index,
    read_compressed_position, unpack_normal, unpack_zlib,
};

const NO_BONE: u32 = u32::MAX;

const MATRIX_SIZE:         usize = size_of::<[[f32; 4]; 4]>();
const WEIGHT_SIZE:         usize = size_of::<u32>() + size_of::<f32>();
const KEYFRAME_SIZE:       usize = size_of::<f32>() + size_of::<[f32; 4]>() + VEC3_SIZE;
const BONE_MIN_SIZE:       usize = ( 2 * COUNT_SIZE ) + size_of::<u8>();
const NAME_MIN_SIZE:       usize = size_of::<u8>();
const GROUP_MIN_SIZE:      usize = NAME_MIN_SIZE + ( 2 * COUNT_SIZE );
const MOTION_KEY_SIZE:     usize = VEC3_SIZE;
const ANIMATION_MIN_SIZE:  usize = NAME_MIN_SIZE + size_of::<u8>() + VEC3_SIZE;
const ATTACHMENT_MIN_SIZE: usize = NAME_MIN_SIZE + MATRIX_SIZE + COUNT_SIZE;

/// Which of the two engine classes the file is for.
#[ derive( Debug, Clone, Copy, PartialEq, Eq ) ]
pub enum Layout
{
    /// `CGeometryAnimBone`: bind poses, bone triangles, triangle parts and attachments.
    Full,

    /// `CGeometryAnimBoneSimple`, used for cutscene actors.
    Simple,
}

#[ derive( Debug, Clone, Copy, PartialEq ) ]
pub struct BoneWeight
{
    pub bone:   u32,
    pub weight: f32,
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct Bone
{
    /// Bone to world in the bind pose; full layout only.
    pub bind_pose: Option<[[f32; 4]; 4]>,

    pub parent:   Option<u32>,
    pub children: Vec<u32>,

    /// Triangles the bone moves; empty for the simple layout.
    pub triangles: Vec<u32>,

    pub obb: Option<Obb>,
}

/// Named point fixed to a bone, such as a muzzle.
#[ derive( Debug, Clone, PartialEq ) ]
pub struct Attachment
{
    pub name:      String,
    pub transform: [[f32; 4]; 4],
    pub bone:      u32,
}

#[ derive( Debug, Clone, Copy, PartialEq ) ]
pub struct Keyframe
{
    /// Seconds from the start of the animation.
    pub time: f32,

    /// Quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    pub position: [f32; 3],
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct Animation
{
    pub name: String,

    /// Root displacement over the whole animation.
    pub displacement: [f32; 3],

    /// One track per bone of the group, in the group's order.
    pub tracks: Vec<Vec<Keyframe>>,

    /// Time followed by two values; meaning not established.
    pub motion: Option<Vec<[f32; 3]>>,
}

impl Animation
{
    /// Time of the last key of the first track, as the engine takes it.
    pub fn duration( &self ) -> f32
    {
        self.tracks.first().and_then( |track| track.last() ).map_or( 0.0, |key| key.time )
    }
}

/// Bones animated together, such as `all`, `head` or `hit_react`.
#[ derive( Debug, Clone, PartialEq ) ]
pub struct BoneGroup
{
    pub name:       String,
    pub bones:      Vec<u32>,
    pub animations: Vec<Animation>,
}

#[ derive( Debug, Clone, PartialEq ) ]
pub struct CharacterMesh
{
    pub layout:  Layout,
    pub texture: String,
    pub flag:    bool,

    /// Always 9 in the game files.
    pub reserved: u32,

    pub positions: Vec<[f32; 3]>,
    pub normals:   Vec<[f32; 3]>,
    pub uvs:       Vec<[f32; 2]>,
    pub triangles: Vec<[u16; 3]>,

    /// Influences of each vertex, weights summing to 1.
    pub weights: Vec<Vec<BoneWeight>>,

    /// Small per-triangle numbers, up to 4; meaning not established. Empty for the simple layout.
    pub triangle_parts: Vec<u8>,

    pub bones:       Vec<Bone>,
    pub attachments: Vec<Attachment>,
    pub groups:      Vec<BoneGroup>,
}

impl CharacterMesh
{
    pub fn parse( data: &[u8] ) -> Result<Self>
    {
        let ( unpacked, tail ) = unpack_zlib( data )?;
        let mut mesh = Self::parse_unpacked( &unpacked, Layout::Full ).or_else( |_| Self::parse_unpacked( &unpacked, Layout::Simple ) )?;

        let mut reader = Reader::new( tail );
        if !tail.is_empty()
        {
            mesh.groups = reader.counted_items( GROUP_MIN_SIZE, |reader| read_group( reader, mesh.bones.len() ) )?;
        }
        reader.finish()?;
        Ok( mesh )
    }

    fn parse_unpacked( data: &[u8], layout: Layout ) -> Result<Self>
    {
        let mut reader = Reader::new( data );
        let bone_count = reader.count()?;
        let weight_count = reader.count()?;
        let weight_data = reader.counted_bytes()?;
        let reserved = reader.u32()?;

        let mut weight_reader = Reader::new( &weight_data );
        let weights = weight_reader.items( weight_count, COUNT_SIZE, |reader| read_weights( reader, bone_count ) )?;
        weight_reader.finish()?;

        let texture = reader.string()?;
        let flag = reader.bool()?;
        let vertex_count = reader.count()?;
        if vertex_count != weights.len()
        {
            return Err( Error::UnknownLayout );
        }

        let positions = if reader.bool()?
        {
            let origin = reader.floats()?;
            reader.items( vertex_count, COMPRESSED_POSITION_SIZE, |reader| read_compressed_position( reader, origin ) )?
        }
        else
        {
            reader.items( vertex_count, VEC3_SIZE, Reader::floats )?
        };
        let normals = reader.items( vertex_count, PACKED_SIZE, |reader| Ok( unpack_normal( reader.u32()? ) ) )?;
        let uvs = reader.items( vertex_count, UV_SIZE, Reader::floats )?;
        let triangle_count = reader.count()?;
        let triangles = reader.triangles( triangle_count, vertex_count, "vertex" )?;

        let triangle_parts = match layout
        {
            Layout::Full => reader.bytes( triangle_count )?.to_vec(),
            Layout::Simple => Vec::new(),
        };
        let shape = Shape { layout, bones: bone_count, triangles: triangle_count };
        let bones = reader.items( bone_count, BONE_MIN_SIZE, |reader| read_bone( reader, shape ) )?;
        let attachments = match layout
        {
            Layout::Full => reader.counted_items( ATTACHMENT_MIN_SIZE, |reader| read_attachment( reader, bone_count ) )?,
            Layout::Simple => Vec::new(),
        };
        reader.finish()?;

        let mesh = Self
        {
            layout,
            texture,
            flag,
            reserved,
            positions,
            normals,
            uvs,
            triangles,
            weights,
            triangle_parts,
            bones,
            attachments,
            groups: Vec::new(),
        };
        Ok( mesh )
    }
}

fn read_bone_index( reader: &mut Reader, bone_count: usize ) -> Result<u32>
{
    let bone = reader.u32()?;
    check_index( "bone", bone, bone_count )?;
    Ok( bone )
}

fn read_weights( reader: &mut Reader, bone_count: usize ) -> Result<Vec<BoneWeight>>
{
    reader.counted_items( WEIGHT_SIZE, |reader| Ok( BoneWeight { bone: read_bone_index( reader, bone_count )?, weight: reader.f32()? } ) )
}

fn read_matrix( reader: &mut Reader ) -> Result<[[f32; 4]; 4]>
{
    Ok( [ reader.floats()?, reader.floats()?, reader.floats()?, reader.floats()? ] )
}

#[ derive( Clone, Copy ) ]
struct Shape
{
    layout:    Layout,
    bones:     usize,
    triangles: usize,
}

fn read_bone( reader: &mut Reader, shape: Shape ) -> Result<Bone>
{
    let bind_pose = match shape.layout
    {
        Layout::Full => Some( read_matrix( reader )? ),
        Layout::Simple => None,
    };

    let parent = match reader.u32()?
    {
        NO_BONE => None,
        parent =>
        {
            check_index( "parent bone", parent, shape.bones )?;
            Some( parent )
        }
    };

    let mut children = reader.counted_items( COUNT_SIZE, Reader::u32 )?;
    children.retain( |&child| child != NO_BONE );
    for &child in &children
    {
        check_index( "child bone", child, shape.bones )?;
    }

    let triangles = match shape.layout
    {
        Layout::Full => reader.counted_items( COUNT_SIZE, |reader|
        {
            let triangle = reader.u32()?;
            check_index( "bone triangle", triangle, shape.triangles )?;
            Ok( triangle )
        } )?,
        Layout::Simple => Vec::new(),
    };

    let obb = if reader.bool()? { Some( reader.obb()? ) } else { None };
    Ok( Bone { bind_pose, parent, children, triangles, obb } )
}

fn read_attachment( reader: &mut Reader, bone_count: usize ) -> Result<Attachment>
{
    let attachment = Attachment
    {
        name:      reader.string()?,
        transform: read_matrix( reader )?,
        bone:      read_bone_index( reader, bone_count )?,
    };
    Ok( attachment )
}

fn read_keyframe( reader: &mut Reader ) -> Result<Keyframe>
{
    Ok( Keyframe { time: reader.f32()?, rotation: reader.floats()?, position: reader.floats()? } )
}

fn read_animation( reader: &mut Reader, track_count: usize ) -> Result<Animation>
{
    let name = reader.string()?;
    let has_motion = reader.bool()?;
    let displacement = reader.floats()?;
    let tracks = reader.items( track_count, COUNT_SIZE, |reader| reader.counted_items( KEYFRAME_SIZE, read_keyframe ) )?;
    let motion = if has_motion { Some( reader.counted_items( MOTION_KEY_SIZE, Reader::floats )? ) } else { None };
    Ok( Animation { name, displacement, tracks, motion } )
}

fn read_group( reader: &mut Reader, bone_count: usize ) -> Result<BoneGroup>
{
    let name = reader.string()?;
    let bones = reader.counted_items( COUNT_SIZE, |reader| read_bone_index( reader, bone_count ) )?;
    let animations = reader.counted_items( ANIMATION_MIN_SIZE, |reader| read_animation( reader, bones.len() ) )?;
    Ok( BoneGroup { name, bones, animations } )
}

#[ cfg( test ) ]
mod tests
{
    use super::*;

    #[ test ]
    fn takes_duration_from_first_track()
    {
        let key = |time| Keyframe { time, rotation: [ 0.0, 0.0, 0.0, 1.0 ], position: [ 0.0; 3 ] };
        let animation = Animation
        {
            name:         "idle".into(),
            displacement: [ 0.0; 3 ],
            tracks:       vec![ vec![ key( 0.0 ), key( 5.0 ) ], vec![ key( 0.0 ), key( 7.0 ) ] ],
            motion:       None,
        };
        assert_eq!( animation.duration(), 5.0 );
    }
}
