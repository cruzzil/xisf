# xisf-rs — research findings and implementation plan

## 1. The licensing position, which decides the architecture

This was researched before any code was written, because it rules out three of
the four obvious starting points.

| Source | Status | Can we use it? |
|---|---|---|
| **The XISF 1.0 specification** | Explicitly open | **Yes — our sole basis** |
| **libXISF** (`~/code/libXISF`) | GPL-3.0 | No |
| **PixInsight PCL** | PCL License 2.0.1 | No |
| **`cruzzil/xisf`** (this repo's old contents) | PCL-derived, unlicensed | No |

### The specification is free to implement

Section 4 of the spec is unambiguous, and is what makes this project possible:

> The entire definition of XISF shall be publicly available without any
> restrictions or monetary cost to anyone. Anyone shall be able to use or
> implement XISF freely without any monetary cost for any purpose. XISF shall
> not be subject to patents or royalties of any kind that could limit its
> availability or impose a monetary cost for its availability or its use.
> These conditions shall constitute a legally binding assignment.

Everything here is written from that document. It references an XSD schema at
`http://pixinsight.com/xisf/xisf-1.0.xsd`, which would be part of the same
definition -- but that URL returns 404, so the specification prose is the only
normative source actually available.

### libXISF is GPL-3.0, and is not a C ABI anyway

Two independent blockers, either of which is fatal to "drop-in libXISF":

1. **Licence.** GPL-3.0 is not compatible with MIT. For libasdf we vendored
   the upstream headers verbatim because they were BSD-3-Clause; we cannot do
   that here. Nothing from libXISF may be copied into this project.
2. **It has no C ABI.** This was checked against a built library rather than
   inferred from the header. `libXISF.so.0.2.13` exports **479 symbols**:

   | | |
   |---|---|
   | C++ mangled (`_Z…`) | 480 |
   | Unmangled | 75 — **every one** from the bundled LZ4/ZSTD/XXH |
   | libXISF's own, with C linkage | **0** |
   | Demangling to `LibXISF::` | 187 |

   The only `extern "C"` in the source tree is in the vendored zlib and lz4;
   `LIBXISF_EXPORT` is a visibility/`dllexport` macro and implies nothing about
   linkage. So the entire public surface is C++ classes in namespace
   `LibXISF`, and the signatures show why that is not reachable from Rust:

   ```
   LibXISF::XISFModify::open(std::__cxx11::basic_string<char, …> const&)
   LibXISF::XISFModify::open(std::filesystem::__cxx11::path const&)
   LibXISF::XISFModify::open(std::basic_istream<char, …>*)
   LibXISF::XISFModify::save(LibXISF::ByteArray&)
   ```

   Note the `__cxx11` ABI tags: these mangled names are specific to
   libstdc++. Built against libc++ or MSVC STL the symbols are *different*, so
   even a hypothetical replacement would have to pick one C++ standard library
   and match its internal `std::string` and `std::vector` layouts, which are
   not stable. It would also have to throw real C++ exceptions
   (`class Error : public std::exception`), which Rust cannot do on stable,
   and the `Matrix` template is instantiated in the *caller*, so no library
   can supply it at all.

   **On writing our own limited header instead:** a header we author declaring
   the same *C* functions would be sound practice — that is what `xisf-c` is.
   But there are no C functions here to declare. Writing our own header that
   re-declared libXISF's *classes* would mean reproducing its class layouts and
   member signatures, which is much closer to copying its expression than
   declaring a C function is, and it would still not link, for the reasons
   above. The honest options are a C ABI of our own design, or, later, a C++
   convenience header of our own design layered on top of it.

**Consequence: there will be no drop-in libXISF replacement.** See §3 for what
replaces that goal.

### PCL must not be used, by its own terms

The PCL licence (v2.0.1) carries a restriction beyond its BSD-style clauses:

> The use of this source code in whole or in part, in both source and binary
> forms, is strictly forbidden for: [...] (c) Providing API services that
> incorporate or derive from this software for automated code generation. This
> restriction applies to all commercial or non-profit entities and expressly
> includes use via APIs, wrappers, or third-party tools.

That describes this exact working arrangement. PCL source is therefore off
limits as an input — including `gitlab.com/pixinsight/PCL`. It is not consulted
anywhere in this project. Clause 4 would also require a PixInsight
acknowledgment in the end-user documentation of any derived product, which is
incompatible with a clean MIT story regardless.

### This repository's previous contents were PCL-derived

The ~5,000 lines previously in `src/` are a transliteration of PCL: the file
names are PCL class names (`RGBColorSystem`, `PixelTraits`, `CharTraits`,
`ICCProfile`, `XISFReaderEngine`), and `src/ImageInfo.rs` still carries the
verbatim PCL banner —

```
//  / ____// /___ / /___   PixInsight Class Library
// /_/     \____//_____/   PCL 2.4.29
// pcl/ImageInfo.h - Released 2022-05-17T17:14:45Z
// Copyright (c) 2003-2022 Pleiades Astrophoto S.L. All Rights Reserved.
```

It has been removed, and so has the history containing it: all twenty original
commits carried `src/`, so the branch was re-rooted onto a fresh initial commit
rather than filtered. No commit reachable from `main` contains PCL-derived
source. (The phrase "PixInsight Class Library" still appears in this document
and in `xisf-core/src/lib.rs`, where it is prose recording *why* none of it is
used.)

Two caveats worth knowing. The old history survives locally on the
`backup-pcl-history` branch and the `old-history-backup` tag, which can be
deleted once nobody wants them. And GitHub keeps unreachable objects for a
while after a force-push, so an old commit may still be fetchable by its SHA
until they are garbage-collected; deleting and recreating the repository is
the only way to be certain.

The three `.xisf` sample files were kept, under `corpus/pixinsight/`. They are
PixInsight *output*, not PCL source — data produced by a program, which the
licence's restrictions on *source code* do not reach. Their provenance is
recorded in `corpus/README.md`.

## 2. What XISF is

Structurally close to ASDF, which is convenient: the architecture that worked
there transfers.

A **monolithic XISF file** is:

```
"XISF0100"          8 bytes, ASCII signature
header length       u32, little-endian
reserved            4 bytes, zero
XISF header         XML 1.0, UTF-8, `<xisf version="1.0">` root
[attached blocks]   raw bytes, addressed from the header
```

There is also a **distributed** form: an XISF header file plus separate data
block files, the direct analogue of ASDF's exploded form.

**Data blocks** carry a `location` attribute in one of these forms:

- `inline:base64` / `inline:hex` — encoded in the element's character data
- `embedded` — in a child `<Data>` element
- `attachment:position:size` — a byte range in this file
- `url:...` / `path:...` — external

with optional `compression="codec[+sh]:uncompressedSize[:itemSize]"` and
`checksum="algorithm:hexdigest"`.

**Codecs:** zlib/deflate, LZ4, LZ4HC, each with an optional byte-shuffling
variant (`+sh`). **Checksums:** SHA-1, SHA-256, SHA-512, SHA3-256, SHA3-512.

**Core elements:** `Property` (a rich type system — scalars, complex, string,
`TimePoint`, vectors, matrices), `Image` (geometry, `sampleFormat`,
`colorSpace`, `pixelStorage`, `bounds`), `Table`, `Metadata`, plus
`FITSKeyword`, `ICCProfile`, `Thumbnail`, `ColorFilterArray`, `DisplayFunction`.

Optional and not planned for the first release: XML digital signatures (X.509).

## 3. Crates

Mirroring the shape that worked for ASDF — one engine, thin faces — so the two
public APIs cannot drift apart in semantics.

| Crate | Purpose |
|---|---|
| `xisf-core` | The engine. Header parse/emit, data blocks, codecs, checksums, properties, images. All behaviour lives here. |
| `xisf` | The idiomatic Rust API. Published under the name already held. |
| `xisf-c` | **Our own** C ABI, with headers we author. Usable from C and C++. Explicitly *not* a libXISF binary drop-in — see §1. |
| `xisf-cli` | A command-line tool: inspect, verify checksums, dump blocks, convert. |

XML handling uses `quick-xml` (MIT) rather than a hand-written parser: unlike
ASDF's YAML — where libasdf's C `strtod` semantics forced a bespoke scalar
layer — XISF mandates plain standard XML 1.0, so there is nothing custom to
honour.

`xisf-c` deserves a note on naming: calling it `libxisf-rs` would imply a
drop-in relationship that does not exist. The header will say plainly that it
is an independent C API for the XISF format, not an implementation of
libXISF's interface.

## 4. How correctness will be judged

The hardest lesson from the ASDF work: a green test suite proved very little
until it was checked against oracles someone else wrote. Four are available
here, and none of them requires deriving from restricted source.

1. ~~**The official XSD schema**~~ -- **not available.** This plan called it
   the strongest gate. It does not exist: `xisf-1.0.xsd` returns 404 at every
   location tried, including `http://pixinsight.com/xisf/xisf-1.0.xsd`, which
   is the exact URL every XISF file's own `xsi:schemaLocation` names -- the
   files in `corpus/pixinsight/` included. The schema is referenced by the
   format and is not published, so there is nothing to validate against.

   Its replacement is item 3, and is arguably better: a schema checks that a
   header is *shaped* right, while another implementation reading the file
   checks that it *means* what we intended.


2. **PixInsight sample files.** Three to start (`corpus/pixinsight/`), already
   covering embedded base64, attachment blocks, byte-shuffled zlib, SHA-256 and
   FITS keywords. To be extended.
3. **libXISF as a black-box oracle, in both directions.** Running a GPL
   program and comparing its output to ours is not derivation and creates no
   licensing obligation -- we just must not link it or copy from it. Both
   directions are wired up and both matter, because they fail differently:

   - `tools/corpus-gen/generate.cpp` writes files we read. 40 of 40 match
     byte for byte.
   - `tools/corpus-gen/verify.cpp` reads files we write. 51 of 51 accepted.

   The second is the one no Rust test can replace. A reader and writer that
   share a misunderstanding round-trip through each other perfectly; only a
   second implementation notices.


4. **`seiza-xisf`** as a third implementation, used the same way — an
   independent reader to compare against, never a source to copy. Apache-2.0,
   and a dev-dependency, so it is absent from what a consumer builds. It
   agrees on 30 files we write and 17 corpus files.

   It complements the libXISF oracle rather than duplicating it. libXISF is
   the stronger check and also generates the corpus, but needs cmake, a C++
   toolchain and a clone from a third-party host, so it runs on one CI job.
   `seiza-xisf` is pure Rust and runs on all six platforms with no build
   step, which makes it the one that would catch a writer producing files
   correct only on x86-64 Linux.

   (An earlier draft named the `xisf-rs` crate here; `seiza-xisf` is more
   complete and actively maintained.)

Plus, carried over from the ASDF project because they earned their place:

- **Round-trip and property tests** — write, read back, compare at the value
  level.
- **Robustness tests** — truncation at every length, single-byte corruption,
  absurd declared sizes. XISF has more attack surface than ASDF here, since a
  `location="attachment:pos:size"` is caller-controlled addressing.
- **Benchmarks from the start**, not at the end. The ASDF array read path was
  twelve times slower than the reference implementation and every test passed;
  only a benchmark found it.
- **Miri over the C ABI crate**, which found two real UB bugs in the ASDF FFI
  layer that all other gates missed.

## 5. Sequence

1. **Foundations** — workspace, MIT licence, CI on the six platforms from the
   start, `corpus/README.md` recording provenance.
2. **`xisf-core` reading** — signature and header, XML parse, data block
   location/compression/checksum attributes, the codecs, byte unshuffling.
   Gate: read all three sample files.
3. **Properties and images** — the type system, geometry, sample formats,
   colour spaces. Gate: full value-level comparison against the samples.
4. **`xisf-core` writing** — header emit, block layout, checksums, compression.
   Gate: libXISF reads what we write (the XSD is unpublished; see above).
5. ~~**`xisf`** — the idiomatic API, with benchmarks alongside.~~ Done; see
   `docs/PERFORMANCE.md`.
6. ~~**`xisf-c`**~~ — done, and named `libxisf` at the user's request. Its own
   header, a C conformance harness compiling real C with `-Werror`, and Miri
   from the first commit.
7. ~~**`xisf-cli`**~~ — done: `info`, `header`, `verify`, `dump`.
8. ~~**Distributed (non-monolithic) XISF**~~ — done for reading: `.xish`
   header files, and `.xisb` data blocks files with their linked-list block
   index. Writing a distributed unit is not wired into `Writer` yet.

Still open:

- ~~Writing distributed units from `Writer`~~ — done:
  `Writer::to_distributed` returns the `.xish` header and the `.xisb` blocks
  file, and a round trip through the ordinary `Reader` is tested.
- ~~Properties serialised as data blocks~~ — done for vectors and matrices.
- ~~`Table` and `Structure`~~ — done. A table's shape lives in a `Structure`
  that may be shared, and its cells are positional, so a row whose cell count
  disagrees with the structure is refused rather than read into the wrong
  columns.
- ~~`ICCProfile`, `Thumbnail`, `Resolution`, `ColorFilterArray`~~ — done.
- ~~`RGBWorkingSpace`, `DisplayFunction`~~ — done. Both default when absent
  (sRGB and the identity), and both are exposed as `Option` so a writer can
  tell "the file said nothing" from "the file said the default".
- ~~`Reference`~~ — done, and it was the one gap that lost data silently.
  An ancillary element may sit inside the element it belongs to *or* sit at
  the root with a `uid`, pointed at by a `<Reference>` child — the second form
  is how one thumbnail or colour space serves several images. Looking only at
  direct children found none of it and reported no error. `Header::associated`
  now follows both, and every lookup in the high-level API goes through it.
- ~~`url:` locators~~ — done, via a caller-supplied resolver. The library
  never makes a network request itself: honouring a URL written in a file
  would let `open` on a local file reach a host the caller never named, which
  is a server-side request forgery primitive when the caller is a service.
  `Reader::set_url_resolver` hands the URL to the caller and takes bytes back,
  so policy, timeouts and TLS belong to whoever can actually decide them --
  and the workspace keeps its lack of an HTTP stack.
- ~~Writing the ancillary elements~~ — done. The reader understood seven
  elements the writer could not produce, which meant a read-modify-write
  through this library dropped every one of them at the moment of saving.
  `PendingImage` now carries them, and an image may own three blocks (pixels,
  ICC profile, thumbnail) rather than one, so block positions are settled
  before any element is emitted rather than accumulated while emitting.
  libXISF reads the result, and the verifier now *requires* that it finds the
  profile, thumbnail and keywords rather than merely parsing the file.
- ~~Reading a `.xish` header file~~ — done. `Writer::to_distributed` produced
  a header file that nothing in the library could open; `Reader::open` now
  accepts both forms, told apart by content rather than by suffix, which is
  unambiguous because an XML document cannot begin with the eight signature
  bytes.
- ~~`XISF:CreationTime`~~ — done. The spec makes it mandatory in `<Metadata>`
  and it was never written. It comes from the clock by default, with
  `Writer::with_creation_time` to pin it, since a timestamp otherwise makes
  output non-reproducible. The date arithmetic is written out rather than
  pulled in as a dependency every user would carry for one line of output.
- ~~`offset` and `orientation` image attributes~~ — done, along with writing
  `uuid`, which was read but never emitted. `offset` is a pedestal that
  calibration subtracts, so an image read without it has the wrong zero point.
  `orientation` is kept as a declaration rather than applied: the spec is
  explicit that a decoder must not reorient pixels for processing that depends
  on their physical layout.
- ~~Structural limits on the header~~ — done. Nesting is capped at 256 and
  element count at a million. The tree is built iteratively but walked,
  cloned, compared and *dropped* recursively, and a stack overflow in Rust
  aborts rather than unwinding, so a 2MB file of nested tags took the whole
  process down. `descendants` is iterative now too.
- ~~Checksums checked by default~~ — done. A file that records a checksum is
  saying how to tell whether its bytes are the bytes that were written;
  handing them over unchecked was a silent corruption the format went out of
  its way to make detectable. `Reader::set_verify_checksums(false)` opts out.
- ~~Bounded block index~~ — done. Index nodes may not repeat a position but
  may overlap, so a hundred thousand of them could each declare a whole
  file's worth of elements: 64KB of input asking for 134MB of index, and a
  megabyte asking for tens of gigabytes. A `path(...)` block now seeks
  through the index holding one element at a time, which also stops a
  distributed unit's blocks file being read whole to take one image out of
  it; the slice parser a `url(...)` block uses is bounded by what a file of
  that size could honestly describe.
- ~~Integer literals in binary, octal and hexadecimal~~ — done. The spec
  permits `0b`, `0o` and `0x`, all of which Rust's own `parse` rejects, and
  only the declared type says whether `0x80E950AB` is 2162774187 or
  -2132193109. `Property::value` decodes per the declared type.
- ~~The normal pixel storage model~~ — done. Both models were parsed, but
  the samples came back as stored and the two are indistinguishable in a
  `Vec`, so an unwary caller got shuffled colour channels silently.
  `ImageRef::read_planar` returns channel order whatever the file used.
- ~~The `@header_dir` locator token~~ — done, and it was a conformance bug
  rather than an omission. The grammar defines exactly two `path()` forms:
  an absolute path, and `path(@header_dir/rel-path)` for one relative to the
  directory holding the header. We handled neither of those spellings for
  the relative case -- a bare `path(name)` was assumed -- so a distributed
  unit written to the spec's own worked examples failed to open, and our own
  writer emitted a form the grammar does not define. Both are fixed; the
  token is stripped before the containment check, so `@header_dir/../..` is
  refused like any other attempt to climb out.

  Worth recording for whoever verifies this next: **libXISF implements only
  `inline`, `embedded` and `attachment` locations**, so the round-trip oracle
  does not cover external blocks at all, and this interpretation rests on the
  specification text rather than on agreement with another implementation.
- ~~Thumbnail and FITS keyword conformance on write~~ — done. A thumbnail may
  not declare `bounds` and must be grayscale or RGB; a FITS keyword name must
  satisfy the FITS 3.0 grammar the spec cites, since being readable as FITS is
  the element's only purpose. Both are checked where the file is built.
- ~~Three targets named `xisf`~~ — fixed. The Rust library, the C ABI library
  and the CLI binary all had the target name `xisf`, which collided twice in
  cargo's output directory. The rlib collision cargo warned about and said may
  become a hard error; the second one broke Windows builds intermittently,
  because the C shared library and the CLI binary raced to write the same
  `xisf.pdb`. The C artifact name is an ABI contract -- a C caller writes
  `-lxisf` -- and a command name is not, so the command became `xisftool`, and
  the C crate dropped an `rlib` that nothing consumed.
- ~~Writing tables, and validating property identifiers~~ — done. Tables were
  readable but not writable, the same asymmetry the ancillary elements had, so
  a read-modify-write dropped them. Cells whose values live in data blocks are
  refused with a message rather than written empty, since this writer does not
  allocate blocks for them.

  An identifier is the only handle a reader has on a property, so it is
  checked when one is written. Worth recording: **the regular expression the
  specification prints for property identifiers rejects its own example.**
  `[_a-zA-Z][_a-zA-Z0-9]*(:([_a-zA-Z][_a-zA-Z0-9])+)*` matches namespace
  segments in *pairs* of characters -- a `*` is missing inside the group -- so
  `foo:bar:Foo2_Bar3`, given as valid three lines later in the same section,
  does not match it. The evident intent is what is implemented.
- XML digital signatures, deliberately out of scope for 1.0.

Digital signatures are out of scope for 1.0 and will be recorded as a known
gap rather than left implicit.
