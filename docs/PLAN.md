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

Everything here is written from that document. The XSD schema it references,
`http://pixinsight.com/xisf/xisf-1.0.xsd`, is part of the same definition.

### libXISF is GPL-3.0, and is not a C ABI anyway

Two independent blockers, either of which is fatal to "drop-in libXISF":

1. **Licence.** GPL-3.0 is not compatible with MIT. For libasdf we vendored
   the upstream headers verbatim because they were BSD-3-Clause; we cannot do
   that here. Nothing from libXISF may be copied into this project.
2. **It has no C ABI.** `libxisf.h` declares C++ classes in namespace
   `LibXISF` — `XISFReader`, `XISFWriter`, `Image`, `Variant`, a template
   `Matrix`, and `class Error : public std::exception`. There is no
   `extern "C"` anywhere. A Rust library cannot be a binary drop-in for that:
   it would have to reproduce Itanium/MSVC name mangling, vtable layouts, RTTI,
   `std::string` and `std::vector` internal layouts (which differ between
   libstdc++, libc++ and MSVC STL and are not stable), and throw real C++
   exceptions, which Rust cannot do on stable. The template is instantiated in
   the *caller*, so it cannot be provided by a library at all.

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

It has been removed from the working tree. **It remains in git history**, which
is worth deciding about separately: publishing an MIT crate from a repository
whose history contains unlicensed PCL-derived code is untidy at best. Options
are to leave it (history is not distributed by `cargo publish`), or to start
the history fresh.

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

1. **The official XSD schema** (`xisf-1.0.xsd`). Every header we emit is
   validated against the format's own schema. This is the strongest gate and
   has no analogue in the ASDF project.
2. **PixInsight sample files.** Three to start (`corpus/pixinsight/`), already
   covering embedded base64, attachment blocks, byte-shuffled zlib, SHA-256 and
   FITS keywords. To be extended.
3. **libXISF as a black-box oracle.** Running a GPL program and comparing its
   output to ours is not derivation and creates no licensing obligation — we
   just must not link it or copy from it. It reads and writes XISF, so it can
   both check our output and generate corpus files. This is the analogue of the
   Python `asdf` differential tests.
4. **The `xisf-rs` crate** as a second opinion, used the same way — as an
   independent reader to compare against, never as a source to copy.

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
   Gate: XSD validation, and libXISF reads what we write.
5. **`xisf`** — the idiomatic API, with benchmarks alongside.
6. **`xisf-c`** — the C ABI, its header, and a C conformance harness modelled
   on the ASDF one. Miri from the first commit.
7. **`xisf-cli`**.
8. **Distributed (non-monolithic) XISF**, then optional extras.

Digital signatures are out of scope for 1.0 and will be recorded as a known
gap rather than left implicit.
