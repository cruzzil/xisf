# Changelog

This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
The four crates share a version so that a reader does not have to correlate
four numbers to know what fits with what; they may diverge once one of them
needs a breaking change the others do not.

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
