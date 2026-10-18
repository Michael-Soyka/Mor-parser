//! Usage: extract <archive.vfs> [out_dir]
//! Without `out_dir`, lists entries instead.

use std::process::ExitCode;

use morvfs::Vfs;

fn main() -> ExitCode
{
    let mut args = std::env::args().skip( 1 );
    let Some( archive ) = args.next() else
    {
        eprintln!( "usage: extract <archive.vfs> [out_dir]" );
        return ExitCode::FAILURE;
    };
    let out_dir = args.next();

    let vfs = match Vfs::open( &archive )
    {
        Ok( vfs ) => vfs,
        Err( err ) =>
        {
            eprintln!( "{archive}: {err}" );
            return ExitCode::FAILURE;
        }
    };

    match out_dir
    {
        Some( out_dir ) =>
        {
            if let Err( err ) = vfs.extract_all( &out_dir )
            {
                eprintln!( "extract failed: {err}" );
                return ExitCode::FAILURE;
            }
            println!( "{} entries -> {out_dir}", vfs.len() );
        }
        None =>
        {
            for entry in vfs.entries()
            {
                println!( "{:>10} {}", entry.size, entry.path );
            }
        }
    }
    ExitCode::SUCCESS
}
