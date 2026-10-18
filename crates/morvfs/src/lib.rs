//! Reader and writer for Pathologic (Mor. Utopia) `.vfs` archives.
//!
//! Layout, all little-endian:
//!
//! ```text
//! +--------+--------------------------------+-------------------------------+
//! | "LP1C" | root node (directory tree)     | file data                     |
//! +--------+--------------------------------+-------------------------------+
//!
//! node
//!   +0     u32  dir_count
//!   +4     u32  file_count
//!   +8     file record * file_count
//!          dir record  * dir_count
//!
//! file record                               dir record
//!   +0     u8   name_len                      +0     u8   name_len
//!   +1     name, cp1251                       +1     name, cp1251
//!   +1+n   u32  size                          +1+n   node (recursive)
//!   +5+n   u32  offset, from file start
//!   +9+n   u64  modified, FILETIME
//!
//! file data: contiguous, in directory order, files of a node before its subdirectories
//!
//!   root/a  root/b  root/sub/c  root/sub/deep/d
//!   +-------+-------+-----------+----------------+
//!   |   a   |   b   |     c     |       d        |
//!   +-------+-------+-----------+----------------+
//! ```

use std::fs::{ self, File };
use std::io::{ self, BufReader, BufWriter, Read, Seek, SeekFrom, Write };
use std::path::{ Path, PathBuf };
use std::time::{ Duration, SystemTime, UNIX_EPOCH };

use encoding_rs::WINDOWS_1251;
use indexmap::IndexMap;

pub const SIGNATURE: [u8; 4] = *b"LP1C";
pub const EXTENSION: &str = "vfs";

const FILETIME_TICK_NANOS: u64 = 100;
const NANOS_PER_SECOND:    u64 = 1_000_000_000;
const TICKS_PER_SECOND:    u64 = NANOS_PER_SECOND / FILETIME_TICK_NANOS;

/// Separators, and `:` of drive prefixes like `C:x`.
const PATH_SYNTAX_CHARS: [char; 3] = [ '/', '\\', ':' ];

/// FILETIME ticks between 1601-01-01 and 1970-01-01.
const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;

/// Largest FILETIME Windows accepts.
const FILETIME_MAX: u64 = i64::MAX.cast_unsigned();

/// dir_count, file_count.
const NODE_HEADER_SIZE: usize = size_of::<u32>() + size_of::<u32>();

/// size, offset, modified: the part of a file record after the name.
const FILE_FIELDS_SIZE: usize = size_of::<u32>() + size_of::<u32>() + size_of::<u64>();

#[ derive( Debug, thiserror::Error ) ]
pub enum Error
{
    #[ error( transparent ) ]
    Io( #[ from ] io::Error ),

    #[ error( "not an LP1C archive" ) ]
    BadSignature,

    #[ error( "entry not found: {0}" ) ]
    NotFound( String ),

    #[ error( "invalid entry name: {0:?}" ) ]
    InvalidName( String ),

    #[ error( "archive exceeds the 4 GiB format limit" ) ]
    TooLarge,
}

pub type Result<T> = std::result::Result<T, Error>;

#[ derive( Debug, Clone, PartialEq, Eq ) ]
pub struct Entry
{
    /// Path inside the archive, `/`-separated, original case.
    pub path: String,

    pub offset: u32,
    pub size:   u32,

    /// Windows FILETIME: 100 ns ticks since 1601-01-01 UTC.
    pub modified: u64,
}

impl Entry
{
    pub fn modified_time( &self ) -> SystemTime
    {
        filetime_to_system_time( self.modified )
    }
}

/// Values past the largest FILETIME Windows accepts are clamped to it.
pub fn filetime_to_system_time( filetime: u64 ) -> SystemTime
{
    let filetime = filetime.min( FILETIME_MAX );
    match filetime.checked_sub( FILETIME_UNIX_EPOCH )
    {
        Some( ticks_after_epoch ) => UNIX_EPOCH + ticks_to_duration( ticks_after_epoch ),
        None => UNIX_EPOCH - ticks_to_duration( FILETIME_UNIX_EPOCH - filetime ),
    }
}

pub fn system_time_to_filetime( time: SystemTime ) -> u64
{
    match time.duration_since( UNIX_EPOCH )
    {
        Ok( after_epoch ) => FILETIME_UNIX_EPOCH.saturating_add( duration_to_ticks( after_epoch ) ),
        Err( before_epoch ) => FILETIME_UNIX_EPOCH.saturating_sub( duration_to_ticks( before_epoch.duration() ) ),
    }
}

fn ticks_to_duration( ticks: u64 ) -> Duration
{
    let seconds = ticks / TICKS_PER_SECOND;
    let nanos = ( ticks % TICKS_PER_SECOND ) * FILETIME_TICK_NANOS;
    Duration::from_secs( seconds ) + Duration::from_nanos( nanos )
}

fn duration_to_ticks( duration: Duration ) -> u64
{
    let ticks = duration.as_nanos() / u128::from( FILETIME_TICK_NANOS );
    u64::try_from( ticks ).unwrap_or( u64::MAX )
}

fn normalize_separators( path: &str ) -> String
{
    path.replace( '\\', "/" )
}

/// Case-insensitive, as the engine treats names.
fn lookup_key( path: &str ) -> String
{
    normalize_separators( path ).to_lowercase()
}

fn validate_name( name: &str ) -> Result<()>
{
    if name.is_empty() || name == "." || name == ".." || name.contains( PATH_SYNTAX_CHARS )
    {
        return Err( Error::InvalidName( name.to_owned() ) );
    }
    Ok( () )
}

fn encode_name( name: &str ) -> Result<Vec<u8>>
{
    validate_name( name )?;
    let ( encoded, _encoding_used, unmappable ) = WINDOWS_1251.encode( name );
    if unmappable || encoded.len() > usize::from( u8::MAX )
    {
        return Err( Error::InvalidName( name.to_owned() ) );
    }
    Ok( encoded.into_owned() )
}

fn decode_name( encoded: &[u8] ) -> String
{
    let ( name, _had_errors ) = WINDOWS_1251.decode_without_bom_handling( encoded );
    name.into_owned()
}

fn read_array<const N: usize>( reader: &mut impl Read ) -> io::Result<[u8; N]>
{
    let mut array = [ 0; N ];
    reader.read_exact( &mut array )?;
    Ok( array )
}

fn read_u8( reader: &mut impl Read ) -> io::Result<u8>
{
    Ok( u8::from_le_bytes( read_array( reader )? ) )
}

fn read_u32( reader: &mut impl Read ) -> io::Result<u32>
{
    Ok( u32::from_le_bytes( read_array( reader )? ) )
}

fn read_u64( reader: &mut impl Read ) -> io::Result<u64>
{
    Ok( u64::from_le_bytes( read_array( reader )? ) )
}

fn read_name( reader: &mut impl Read ) -> Result<String>
{
    let len = read_u8( reader )?;
    let mut encoded = vec![ 0; usize::from( len ) ];
    reader.read_exact( &mut encoded )?;

    let name = decode_name( &encoded );
    validate_name( &name )?;
    Ok( name )
}

fn read_file_record( reader: &mut impl Read, path_prefix: &str ) -> Result<Entry>
{
    let name = read_name( reader )?;
    let size = read_u32( reader )?;
    let offset = read_u32( reader )?;
    let modified = read_u64( reader )?;

    let path = format!( "{path_prefix}{name}" );
    Ok( Entry { path, offset, size, modified } )
}

fn read_node( reader: &mut impl Read, path_prefix: &str, entries: &mut IndexMap<String, Entry> ) -> Result<()>
{
    let dir_count = read_u32( reader )?;
    let file_count = read_u32( reader )?;

    for _ in 0..file_count
    {
        let entry = read_file_record( reader, path_prefix )?;
        entries.insert( lookup_key( &entry.path ), entry );
    }

    for _ in 0..dir_count
    {
        let dir_name = read_name( reader )?;
        read_node( reader, &format!( "{path_prefix}{dir_name}/" ), entries )?;
    }
    Ok( () )
}

/// An opened archive. Only the directory is kept in memory; data is read on demand.
#[ derive( Debug ) ]
pub struct Vfs
{
    path:    PathBuf,
    entries: IndexMap<String, Entry>,
}

impl Vfs
{
    pub fn open( path: impl AsRef<Path> ) -> Result<Self>
    {
        let path = path.as_ref().to_path_buf();
        let mut reader = BufReader::new( File::open( &path )? );

        let signature: [u8; SIGNATURE.len()] = read_array( &mut reader )?;
        if signature != SIGNATURE
        {
            return Err( Error::BadSignature );
        }

        let mut entries = IndexMap::new();
        read_node( &mut reader, "", &mut entries )?;
        Ok( Self { path, entries } )
    }

    pub fn path( &self ) -> &Path
    {
        &self.path
    }

    pub fn len( &self ) -> usize
    {
        self.entries.len()
    }

    pub fn is_empty( &self ) -> bool
    {
        self.entries.is_empty()
    }

    /// Entries in on-disk order.
    pub fn entries( &self ) -> impl ExactSizeIterator<Item = &Entry>
    {
        self.entries.values()
    }

    /// Case-insensitive; accepts `/` or `\` separators.
    pub fn get( &self, path: &str ) -> Option<&Entry>
    {
        self.entries.get( &lookup_key( path ) )
    }

    pub fn contains( &self, path: &str ) -> bool
    {
        self.entries.contains_key( &lookup_key( path ) )
    }

    /// Opens the archive file for this one read; use [`Vfs::reader`] for many.
    pub fn read( &self, path: &str ) -> Result<Vec<u8>>
    {
        self.reader()?.read( path )
    }

    /// Opens the archive file for this one read; use [`Vfs::reader`] for many.
    pub fn read_entry( &self, entry: &Entry ) -> Result<Vec<u8>>
    {
        self.reader()?.read_entry( entry )
    }

    /// Keeps the archive file open across reads. Not shared between threads: open one per thread.
    pub fn reader( &self ) -> Result<VfsReader<'_>>
    {
        let archive = File::open( &self.path )?;
        Ok( VfsReader { vfs: self, archive } )
    }

    /// Extracts every entry under `dir`, recreating subdirectories.
    pub fn extract_all( &self, dir: impl AsRef<Path> ) -> Result<()>
    {
        self.extract_matching( dir, |_| true )
    }

    pub fn extract_matching( &self, dir: impl AsRef<Path>, mut predicate: impl FnMut( &Entry ) -> bool ) -> Result<()>
    {
        let dir = dir.as_ref();
        let mut archive = BufReader::new( File::open( &self.path )? );
        let mut data = Vec::new();
        for entry in self.entries().filter( |entry| predicate( entry ) )
        {
            let target = dir.join( &entry.path );
            if let Some( parent ) = target.parent()
            {
                fs::create_dir_all( parent )?;
            }
            read_entry_into( &mut archive, entry, &mut data )?;
            fs::write( target, &data )?;
        }
        Ok( () )
    }
}

/// Reads entries of a [`Vfs`] through one open archive file.
#[ derive( Debug ) ]
pub struct VfsReader<'a>
{
    vfs:     &'a Vfs,
    archive: File,
}

impl VfsReader<'_>
{
    pub fn read( &mut self, path: &str ) -> Result<Vec<u8>>
    {
        let entry = self.vfs.get( path ).ok_or_else( || Error::NotFound( path.to_owned() ) )?;
        self.read_entry( entry )
    }

    pub fn read_entry( &mut self, entry: &Entry ) -> Result<Vec<u8>>
    {
        let mut data = Vec::new();
        self.read_entry_into( entry, &mut data )?;
        Ok( data )
    }

    /// Replaces the contents of `data`, keeping its allocation.
    pub fn read_entry_into( &mut self, entry: &Entry, data: &mut Vec<u8> ) -> Result<()>
    {
        read_entry_into( &mut self.archive, entry, data )
    }
}

fn read_entry_into( archive: &mut ( impl Read + Seek ), entry: &Entry, data: &mut Vec<u8> ) -> Result<()>
{
    let size = usize::try_from( entry.size ).map_err( |_| Error::TooLarge )?;

    data.clear();
    archive.seek( SeekFrom::Start( entry.offset.into() ) )?;
    archive.take( entry.size.into() ).read_to_end( data )?;
    if data.len() < size
    {
        return Err( io::Error::from( io::ErrorKind::UnexpectedEof ).into() );
    }
    Ok( () )
}

#[ derive( Debug ) ]
struct PendingFile
{
    name:     Vec<u8>,
    data:     Vec<u8>,
    modified: u64,
}

impl PendingFile
{
    fn record_size( &self ) -> usize
    {
        stored_name_size( &self.name ) + FILE_FIELDS_SIZE
    }

    fn write_record( &self, writer: &mut impl Write, offset: usize ) -> Result<()>
    {
        let size = to_u32( self.data.len() )?;
        let offset = to_u32( offset )?;

        write_name( writer, &self.name )?;
        write_u32( writer, size )?;
        write_u32( writer, offset )?;
        write_u64( writer, self.modified )?;
        Ok( () )
    }
}

#[ derive( Debug, Default ) ]
struct DirNode
{
    name:  Vec<u8>,
    files: IndexMap<String, PendingFile>,
    dirs:  IndexMap<String, DirNode>,
}

impl DirNode
{
    fn subdir_or_create( &mut self, name: &str, encoded: Vec<u8> ) -> &mut Self
    {
        self.dirs.entry( lookup_key( name ) ).or_insert_with( || Self { name: encoded, ..Self::default() } )
    }

    fn record_size( &self ) -> usize
    {
        stored_name_size( &self.name ) + self.node_size()
    }

    fn node_size( &self ) -> usize
    {
        let file_records: usize = self.files.values().map( PendingFile::record_size ).sum();
        let dir_records: usize = self.dirs.values().map( Self::record_size ).sum();
        NODE_HEADER_SIZE + file_records + dir_records
    }

    fn write_record( &self, writer: &mut impl Write, next_offset: &mut usize ) -> Result<()>
    {
        write_name( writer, &self.name )?;
        self.write_node( writer, next_offset )
    }

    fn write_node( &self, writer: &mut impl Write, next_offset: &mut usize ) -> Result<()>
    {
        let dir_count = to_u32( self.dirs.len() )?;
        let file_count = to_u32( self.files.len() )?;
        write_u32( writer, dir_count )?;
        write_u32( writer, file_count )?;

        for file in self.files.values()
        {
            file.write_record( writer, *next_offset )?;
            *next_offset += file.data.len();
        }

        for dir in self.dirs.values()
        {
            dir.write_record( writer, next_offset )?;
        }
        Ok( () )
    }

    fn write_data( &self, writer: &mut impl Write ) -> Result<()>
    {
        for file in self.files.values()
        {
            writer.write_all( &file.data )?;
        }
        for dir in self.dirs.values()
        {
            dir.write_data( writer )?;
        }
        Ok( () )
    }
}

fn stored_name_size( name: &[u8] ) -> usize
{
    size_of::<u8>() + name.len()
}

fn to_u32( value: usize ) -> Result<u32>
{
    u32::try_from( value ).map_err( |_| Error::TooLarge )
}

fn write_u8( writer: &mut impl Write, value: u8 ) -> io::Result<()>
{
    writer.write_all( &value.to_le_bytes() )
}

fn write_u32( writer: &mut impl Write, value: u32 ) -> io::Result<()>
{
    writer.write_all( &value.to_le_bytes() )
}

fn write_u64( writer: &mut impl Write, value: u64 ) -> io::Result<()>
{
    writer.write_all( &value.to_le_bytes() )
}

fn write_name( writer: &mut impl Write, name: &[u8] ) -> Result<()>
{
    let len = u8::try_from( name.len() ).map_err( |_| Error::InvalidName( decode_name( name ) ) )?;
    write_u8( writer, len )?;
    writer.write_all( name )?;
    Ok( () )
}

/// Builds a new archive. Entry order is preserved, so re-adding the entries of an
/// opened [`Vfs`] in order reproduces it byte for byte.
#[ derive( Debug, Default ) ]
pub struct VfsWriter
{
    root: DirNode,
}

impl VfsWriter
{
    pub fn new() -> Self
    {
        Self::default()
    }

    /// Adds or replaces a file. `path` is `/`- or `\`-separated.
    pub fn add( &mut self, path: &str, data: Vec<u8>, modified: u64 ) -> Result<()>
    {
        let normalized = normalize_separators( path );
        let mut components = normalized.split( '/' );
        let file_name = components.next_back().unwrap_or_default();

        let file = PendingFile { name: encode_name( file_name )?, data, modified };
        let dirs: Vec<( &str, Vec<u8> )> = components
            .map( |dir_name| encode_name( dir_name ).map( |encoded| ( dir_name, encoded ) ) )
            .collect::<Result<_>>()?;

        let mut node = &mut self.root;
        for ( dir_name, encoded ) in dirs
        {
            node = node.subdir_or_create( dir_name, encoded );
        }
        node.files.insert( lookup_key( file_name ), file );
        Ok( () )
    }

    pub fn write_to( &self, writer: impl Write ) -> Result<()>
    {
        let mut writer = BufWriter::new( writer );
        let mut next_offset = SIGNATURE.len() + self.root.node_size();

        writer.write_all( &SIGNATURE )?;
        self.root.write_node( &mut writer, &mut next_offset )?;
        self.root.write_data( &mut writer )?;
        writer.flush()?;
        Ok( () )
    }

    pub fn save( &self, path: impl AsRef<Path> ) -> Result<()>
    {
        self.write_to( File::create( path )? )
    }
}

#[ cfg( test ) ]
mod tests
{
    use super::*;

    #[ test ]
    fn roundtrip_nested()
    {
        let mut writer = VfsWriter::new();
        writer.add( "a.txt", b"hello".to_vec(), 1 ).unwrap();
        writer.add( "UI/b.tga", b"xy".to_vec(), 2 ).unwrap();
        writer.add( "ui/Sub/c.bin", vec![ 7; 3 ], 3 ).unwrap();
        writer.add( "mono_anna11 д.mms", b"cp1251".to_vec(), 4 ).unwrap();

        let path = std::env::temp_dir().join( format!( "morvfs-test-{}.vfs", std::process::id() ) );
        writer.save( &path ).unwrap();
        let vfs = Vfs::open( &path ).unwrap();

        let paths: Vec<_> = vfs.entries().map( |entry| entry.path.as_str() ).collect();
        assert_eq!( paths, [ "a.txt", "mono_anna11 д.mms", "UI/b.tga", "UI/Sub/c.bin" ] );
        assert_eq!( vfs.read( "A.TXT" ).unwrap(), b"hello" );
        assert_eq!( vfs.read( "ui\\sub\\C.bin" ).unwrap(), [ 7; 3 ] );
        assert_eq!( vfs.get( "UI/b.tga" ).unwrap().modified, 2 );

        let mut reader = vfs.reader().unwrap();
        let mut data = Vec::new();
        let contents: Vec<Vec<u8>> = vfs
            .entries()
            .map( |entry| reader.read_entry_into( entry, &mut data ).map( |()| data.clone() ) )
            .collect::<Result<_>>()
            .unwrap();
        assert_eq!( contents, [ b"hello".to_vec(), b"cp1251".to_vec(), b"xy".to_vec(), vec![ 7; 3 ] ] );
        fs::remove_file( path ).unwrap();
    }

    fn temp_archive_path( test_name: &str ) -> PathBuf
    {
        std::env::temp_dir().join( format!( "morvfs-{test_name}-{}.vfs", std::process::id() ) )
    }

    fn archive_with_one_file( name: &[u8] ) -> Vec<u8>
    {
        let ( dir_count, file_count ) = ( 0, 1 );
        let ( size, offset, modified ) = ( 0, 0, 0 );

        let mut archive = SIGNATURE.to_vec();
        write_u32( &mut archive, dir_count ).unwrap();
        write_u32( &mut archive, file_count ).unwrap();
        write_name( &mut archive, name ).unwrap();
        write_u32( &mut archive, size ).unwrap();
        write_u32( &mut archive, offset ).unwrap();
        write_u64( &mut archive, modified ).unwrap();
        archive
    }

    #[ test ]
    fn rejects_bad_names()
    {
        let mut writer = VfsWriter::new();
        assert!( writer.add( "../evil", vec![], 0 ).is_err() );
        assert!( writer.add( "dir//x", vec![], 0 ).is_err() );
        assert!( writer.add( "emoji_🙂", vec![], 0 ).is_err() );
        assert!( writer.add( "C:evil", vec![], 0 ).is_err() );
    }

    #[ test ]
    fn rejects_drive_relative_name_on_open()
    {
        let path = temp_archive_path( "drive-name" );
        fs::write( &path, archive_with_one_file( b"C:evil" ) ).unwrap();
        let opened = Vfs::open( &path );
        fs::remove_file( &path ).unwrap();
        assert!( matches!( opened, Err( Error::InvalidName( name ) ) if name == "C:evil" ) );
    }

    #[ test ]
    fn failed_add_leaves_no_directories()
    {
        let mut empty = Vec::new();
        VfsWriter::new().write_to( &mut empty ).unwrap();

        for bad_path in [ "UI/Sub/bad:name", "UI/../x" ]
        {
            let mut writer = VfsWriter::new();
            assert!( writer.add( bad_path, vec![], 0 ).is_err() );

            let mut written = Vec::new();
            writer.write_to( &mut written ).unwrap();
            assert_eq!( written, empty, "{bad_path}" );
        }
    }

    #[ test ]
    fn filetime_conversion()
    {
        let time = filetime_to_system_time( FILETIME_UNIX_EPOCH );
        assert_eq!( time, UNIX_EPOCH );
        assert_eq!( system_time_to_filetime( time ), FILETIME_UNIX_EPOCH );
    }

    #[ test ]
    fn clamps_out_of_range_filetime()
    {
        let latest = filetime_to_system_time( u64::MAX );
        assert_eq!( latest, filetime_to_system_time( FILETIME_MAX ) );
        assert_eq!( system_time_to_filetime( latest ), FILETIME_MAX );
    }
}
