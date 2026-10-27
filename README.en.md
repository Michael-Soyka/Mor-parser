# mor-parsers

[Русский](README.md) | **English**

Parsers for the game data of "Pathologic" ("Mor. Utopia", Pathologic Classic HD).

Only game-specific formats live here. Standard ones the game also uses (XML, PNG, OGG Vorbis)
are left to the consumer and stock crates. Every crate documents its binary layout at the top of its
`lib.rs` or module file.

## Crates

| Crate      | Formats                         | What it gives                                                        |
|------------|---------------------------------|----------------------------------------------------------------------|
| `morvfs`   | `.vfs`                          | Archive reader and writer, batch reading through one handle          |
| `mortex`   | `.tex`                          | DDS textures: DXT1/3/5, BGR(A), cubemaps; RGBA decode or raw mips    |
| `morft`    | `.ft`                           | Bitmap fonts: glyphs, metrics, kerning                               |
| `mormesh`  | `.mesh`                         | Geometry of every layout, lightmap decoder, optional glTF export     |
| `morscene` | `.isc`, `.osc`, `.pfd`, `.map`  | Scenes, actors, path finding data, city map, optional glTF export    |

### morvfs

`LP1C` archives: a directory tree with cp1251 names and FILETIME stamps, followed by the file data.
`Vfs::open` reads the tree, `Vfs::reader` keeps one file handle for many reads,
`extract_all` / `extract_matching` unpack to disk, `VfsWriter` builds a new archive.

### mortex

A `.tex` file is a plain DDS. `Texture::decode( face, mip )` returns RGBA8,
`Texture::level( face, mip )` returns the stored bytes for upload as compressed textures.

### morft

`.ft` bitmap fonts: line and cell height, per-glyph placement and advance, kerning pairs,
8-bit coverage bitmaps.

### mormesh

`Mesh::parse` picks the layout by the leading zero and by which layout consumes the file exactly:

- `Static`: surfaces with lightmaps, collision hulls;
- `Movable`: surfaces without lightmaps and an OBB;
- `Land`: the terrain, tiles of surfaces;
- `Raw`: a single textured surface;
- `Water`: river and pond meshes.

Layouts the engine selects by context are parsed by their own modules:

- `layout::interior`: interiors, a BSP tree of rooms with collision and lightmapped surfaces;
- `layout::detail`: vegetation, instances of raw meshes over a grid, with the yaw reproduced from the engine's RNG;
- `layout::character`: skinned characters with skeleton, attachments and animations.

`lightmap` decodes lightmap pages: a JPEG-LS (LOCO-I) variant with NEAR = 2, four L8 mip levels,
ported from `D3D.dll` and checked bit-exact against the original code.

The `gltf` feature adds export to `.glb` or `.gltf` + `.bin`.

### morscene

`Scene::load` reads an `.isc` interior or `.osc` outdoor scene the way the game does (lenient XML,
`<reference>` includes resolved through a callback). Further modules:

- `actor`: binary actor descriptions from `Actors.vfs`: scene geometry, lights and light groups
  with coronas, water, vegetation;
- `path`: `.pfd` navigation mesh, edge graph and lookup grid;
- `map`: `city.map`, district outlines and the in-game map picture with its pointable areas.

The `gltf` feature exports a loaded scene: the terrain or interior, every actor with a mesh,
lights as `KHR_lights_punctual` point lights and scene points as empty nodes.

## Not supported

- LifeStudio:Head facial animation: `.lsh` head meshes, `.mms` lip-sync sequences, `.mma` animation libraries;
- shaders;
- export
  - character skeletons and animations in glTF, characters go out in the bind pose;
  - lightmaps in glTF export;
- collision data of geometry actors;
- a few light actors whose layout does not match the rest.

## Usage examples

Command line tools under `crates/*/examples`:

```sh
cargo run -p morvfs  --example extract    -- <archive.vfs> [out_dir]
cargo run -p mortex  --example tex2png    -- <Textures.vfs> <name.tex> <out.png> [mip]
cargo run -p morft   --example text2png   -- <Fonts.vfs> <name.ft> <text> <out.png>
cargo run -p mormesh --example mesh2obj   -- <Geometries.vfs> <name.mesh> <out.obj>
cargo run -p mormesh --example mesh2gltf  --features gltf -- <data_dir> <name.mesh> <out.glb | out.gltf>
cargo run -p morscene --example scene2gltf --features gltf -- <data_dir> <name.isc | name.osc> <out.glb | out.gltf>
```

`data_dir` is the game's `data` directory with the `.vfs` archives; textures are embedded as PNG.

## Coordinates

The game is left-handed Direct3D with clockwise front faces, Y up. The parsers return data as stored;
glTF export negates z and reverses the winding.

## Code example

```toml
[dependencies]
morvfs   = { git = "https://github.com/Michael-Soyka/mor-parsers" }
mortex   = { git = "https://github.com/Michael-Soyka/mor-parsers" }
mormesh  = { git = "https://github.com/Michael-Soyka/mor-parsers", features = [ "gltf" ] }
```

```rust
use mormesh::Mesh;
use mortex::Texture;
use morvfs::Vfs;

let textures = Vfs::open( "data/Textures.vfs" )?;
let image = Texture::parse( &textures.read( "wall.tex" )? )?.decode( 0, 0 )?;

let geometries = Vfs::open( "data/Geometries.vfs" )?;
let mesh = Mesh::parse( &geometries.read( "bench.mesh" )? )?;
```

## Tests

```sh
cargo test --workspace --all-features
```

Unit tests run on synthetic data.

Tests in `crates/*/tests` run against the installed game, located by
`PATHOLOGIC_DATA` (default in `.cargo/config.toml`, overridable from the shell), and are skipped
when that directory does not exist.
