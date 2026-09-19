# Changelog

This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
The four crates share a version so that a reader does not have to correlate
four numbers to know what fits with what; they may diverge once one of them
needs a breaking change the others do not.

## [Unreleased]

Brings the implementation up to Revision 1 of the XISF 1.0 specification
(version 1.01, September 2026). The format version is unchanged and Revision 1
guarantees that "every XISF unit valid under the original document remains
valid", so no file stops being readable. There are breaking changes to the
Rust API, listed at the end.

### Fixed

Four misreadings of the original specification, found while going through the
revision. None depends on it; the revision is what made them visible.

- **Element names were matched without their namespace.** Revision 1 requires
  extension elements to live in a namespace other than XISF's, so a header may
  carry an `<ext:Image>` that means nothing to a decoder — and taking the
  local name alone read it as a core `<Image>`, inventing an image the file
  does not contain. A document that declares no namespace is still read as
  XISF, since the specification's own examples are written that way. An
  undeclared prefix is now an error rather than a guess.
- **An inline block with no character data was refused as missing.** Revision
  1 serializes empty vectors and matrices exactly that way — "the only data
  blocks of zero length" — so every empty aggregate property was unreadable.
- **The `encoding` attribute of a `<Data>` element was ignored** and base64
  assumed, turning a legal hex block into a decoding error.
- **A `<Data>` element's own `compression` and `checksum` were dropped.** The
  checksum is the serious one: a block that recorded how to detect tampering
  was handed over unverified, and a deliberately wrong digest passed without a
  word.

### Changed

- **Zstandard and the checksum hashes are no longer optional.** Revision 1
  makes `zstd` and `zstd+sh` standard codecs and requires conforming decoders
  to support them and to verify SHA-1, SHA-256 and SHA-512. A build with those
  off cannot claim conformance, so the `zstd` and `checksums` features are
  gone and their dependencies are unconditional. `zlib` and `lz4` remain
  optional, which the specification allows.
- **Zstandard can now be written**, not only read, via `zrip` — pure Rust, so
  the workspace still needs no C toolchain. `ruzstd` keeps the decoding job,
  since that is the side facing hostile files and it has far more use behind
  it. `Codec2::Zstd` is the writer's default, Revision 1 having made
  Zstandard the recommended codec.
- **The decompression-bomb ceiling is now per codec.** One ceiling of 2048,
  chosen for DEFLATE, would have rejected legitimate Zstandard blocks: eight
  megabytes of zeroes compress to 781 bytes, 10741:1, and an all-zero
  calibration frame is an ordinary thing to find in an astronomical image.
  This also tightens LZ4 from 2048 to 512.
- **RGB working space luminance coefficients are derived, not given.**
  Revision 1 makes them follow from the chromaticities and the D50 reference
  white; encoders "shall" compute them that way. The writer does, and refuses
  degenerate primaries. `RgbWorkingSpace::new` derives them, and `srgb()` now
  derives its own rather than carrying copied literals.
- **`imageType` is an enumeration** rather than an opaque string, including
  the `SlopeMap` and `WeightMap` values, with an `Other` variant so an
  unrecognized value costs nothing.
- **Image ids must be unique within a unit.** The writer refuses duplicates;
  the reader reports them through `Header::duplicate_image_ids`.
- The writer emits `XISF:ChecksumAlgorithms` and `XISF:CompressionCodecs`,
  derived from the blocks it actually wrote.

### Added

- **`xisf_core::astrometry`**: the `AstrometricSolution` namespace, modelled
  with its four layers and the availability rules that govern them — an
  unknown projection system costs the whole solution, an unknown basis
  function or term kind costs only the distortion model, an unknown celestial
  reference system costs nothing, and an unsupported major revision means
  interpreting none of it. It models and validates a solution; it does not
  evaluate one. See the module documentation for why.
- **Schema validation.** `scripts/fetch-xsd.sh` downloads the official XML
  Schema that Revision 1 publishes, and `scripts/validate-headers.sh` checks
  headers against it. The schema is fetched rather than vendored: it is
  all-rights-reserved with no redistribution grant, so `/schema/` is
  gitignored. CI validates both the corpus and a file this writer produced.
- `RgbWorkingSpace::derive_luminance`, `luminance_is_consistent`, and the
  `D50_WHITE` constant.

### Breaking

- `header::Element` gains a `namespace` field, and core-element lookups
  require it.
- `block::Location::Embedded` is now `Embedded { encoding }`.
- `image::Image::image_type` is `Option<ImageType>` rather than
  `Option<String>`.
- The `zstd` and `checksums` cargo features are removed; both are always on.
- `writer::Codec2` gains a `Zstd` variant and now implements `Default`.

## [0.3.0] — 2026-09-17

A security release. The Rust API is unchanged; the one behaviour change is in
the C API, described below, and is the reason this is a minor rather than a
patch release.

### Security

A review of where the library trusts what a file tells it. Every item below is
reachable from an ordinary `open`-then-read of a hostile file; none requires an
unusual API call. There are no breaking changes to the Rust API.

- **Decompression bombs in the zlib and zstd paths.** The expansion-ratio
  check bounded the size a header *claimed*, which a hostile file simply
  understates; nothing bounded what the decoder actually produced, and
  `read_to_end` ran the stream to completion before the length check could
  object. 260 KB of input allocated 256 MiB; a 4 MB block would have reached
  several gigabytes. Both decoders now stop one byte past the declared size,
  which is enough to detect the lie and refuse it — the same block is now
  rejected in microseconds without the allocation. LZ4 was never affected, as
  it decodes into a caller-sized buffer.
- **`xisf_image_read` did not enforce the declared geometry.** The Rust API
  requires a block to hold exactly the pixels the geometry describes; the C
  entry point checked only that the caller's buffer was large enough. A file
  whose block was shorter than its geometry was copied in full and reported as
  success, leaving the tail of a buffer sized from `xisf_image_data_size`
  holding uninitialised memory. It now returns `XISF_ERR_TRUNCATED` and writes
  nothing.
- **`xisftool` printed header strings to the terminal verbatim.** FITS
  keywords, property values, element names and locators are attacker-chosen
  text, and a terminal executes some of it: a file carrying ANSI escapes could
  clear the screen or, through the OSC forms, reach the window title and on
  some terminals the clipboard. Control characters are now shown as escapes.
  The `header` subcommand still emits the exact bytes when its output is
  redirected, since extracting the header unaltered is what it is for.
- **Quadratic cycle detection in the block index walk.** Cycle detection used
  a linear scan of a growing list, so a 1.6 MB file of chained index nodes
  took 830 ms to refuse. Now a `BTreeSet`.
- **Unbounded attribute count in the header.** The element and depth caps said
  nothing about how wide an element could be, and an attribute is the cheaper
  thing to write. A budget of 2,000,000 attributes across the header now
  bounds it.

### Added

- A fuzzing harness (`fuzz/`) with four `cargo-fuzz` targets — `header`,
  `reader`, `codec` and `blocks` — and a CI job that runs each briefly on every
  push under an explicit RSS limit, so a decoder that can be made to allocate
  without bound fails as an out-of-memory rather than passing quietly. Kept out
  of the workspace, since it needs nightly and a sanitizer.

## [0.2.0] — 2026-09-07

The first release of this implementation.

It starts at 0.2.0 rather than 0.1.0 because the `xisf` name on crates.io
already carries a 0.1.0, published in 2023 from an unrelated codebase. Nothing
in this release derives from it.

### The crates

| Crate | What it is |
|---|---|
| `xisf-core` | The engine: parsing, block decoding, codecs, checksums, the writer. |
| `xisf` | The library most callers want, layered over the engine. |
| `libxisf` | A C ABI: `libxisf.so` / `xisf.dll`, with `include/xisf.h`. |
| `xisftool` | A command-line tool: `info`, `header`, `verify`, `dump`. |

### Reading

- Both file forms: monolithic `.xisf`, and distributed `.xish` header files
  with their `.xisb` data blocks files. The two are told apart by content
  rather than by suffix, which is unambiguous because an XML document cannot
  begin with the eight-byte signature.
- All four block locations: `inline`, `embedded`, `attachment`, and the
  external `path(...)` and `url(...)` forms, including the `@header_dir`
  token the specification defines for relative paths.
- Every codec the specification names — zlib, lz4, lz4hc — with byte
  shuffling, plus zstd, which is not standard but which libXISF writes.
  Compression subblocks are honoured, so a block stored as several
  independent streams decodes whole.
- All thirteen core elements, including `Reference`, so an element defined
  once at the root and shared by several images is found by all of them.
- Checksums are verified when a block is read, which the specification
  requires of a decoder. `Reader::set_verify_checksums(false)` opts out.

### Writing

- Monolithic and distributed units, with compression, byte shuffling and
  checksums.
- Every ancillary element the reader understands: resolution, ICC profile,
  RGB working space, display function, colour filter array, thumbnail, FITS
  keywords and tables. A read-modify-write keeps what it read.

### Safety

An XISF header is untrusted input in the ordinary case — images are
downloaded, shared and unpacked from archives. The reader is built for that:

- Nesting depth and element count are bounded, because the element tree is
  dropped recursively and a stack overflow in Rust aborts the process rather
  than unwinding.
- Decompression is bounded by an expansion ratio against the bytes actually
  present, and the block index by what a file of its size could describe.
- `path(...)` locators resolve inside the referring file's directory, with
  both ends canonicalised so a symbolic link cannot leave it. Absolute paths
  are refused unless a caller opts in.
- `url(...)` blocks are never fetched by this library. A caller installs a
  resolver if it wants them, so policy, timeouts and TLS belong to whoever
  can actually decide them.

### Verification

- Read against PixInsight's own sample files, and against 40 files written by
  libXISF, which read back byte for byte.
- Written files are read back by libXISF: 52 of 52, including one carrying
  every ancillary element, where the check is that libXISF *finds* the ICC
  profile, thumbnail and keywords rather than merely parsing the file.
- `seiza-xisf`, a third implementation, as a differential test oracle.
- Miri over the engine and the whole C ABI; a C conformance harness compiling
  real C against the header with `-Werror`; CI on six platforms.

### Known gaps

- XML digital signatures are not implemented.
- The writer does not split a block that exceeds a codec's input limit into
  subblocks; it refuses rather than writing something it could not read back.
- Cells whose values live in data blocks can be read but not written.
