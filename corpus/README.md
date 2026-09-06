# Test corpus

## `pixinsight/`

XISF files produced by PixInsight, kept as read fixtures.

| File | What it exercises |
|---|---|
| `simple_10by10.xisf` | `UInt8` RGB, `location="embedded"`, base64 |
| `Sample_F32_NoCompression_NoSecurity.xisf` | `Float32`, attached block, no compression or checksum |
| `Sample_F32_ZlibCompression_Sha256Security.xisf` | `Float32`, `compression="zlib+sh:30000:4"` (byte-shuffled zlib), `checksum="sha256:..."`, FITS keywords |

**Provenance.** These are the *output* of PixInsight, not PCL source code: data
a program produced, which the PCL licence's restrictions on source code do not
reach. They came with this repository's earlier contents. Nothing in this
project derives from PCL or libXISF source; see `docs/PLAN.md` §1.

They are fixtures, not a substitute for a real corpus. Three files cannot
cover a format this large, and the plan is to grow this using libXISF as a
black-box generator — running it produces files to read, which is not
derivation and carries no licensing obligation.

## `generated/`

40 files written by `tools/corpus-gen`, which links libXISF: five sample
formats (`UInt8`, `UInt16`, `UInt32`, `Float32`, `Float64`) against seven
codecs (none, zlib, zlib+sh, lz4, lz4+sh, lz4hc, zstd), plus an RGB case per
format to prove the channel count reaches the geometry.

Each image is filled with a deterministic pattern the tests recompute, so
`generated_corpus.rs` compares byte for byte rather than merely checking that
parsing succeeded. A reader that transposed the geometry, dropped a channel or
unshuffled with the wrong item size would parse happily and fail there.

**Why generate rather than hand-write.** These bytes were laid out by an
implementation that is not ours and was not consulted while writing ours, so
agreeing with them is evidence about the format rather than about our own
assumptions. Running a program creates no licensing obligation on its output,
which is what lets GPL-licensed libXISF produce fixtures for an MIT project;
`tools/corpus-gen` is itself GPL-3.0 and is never linked into anything here.

To regenerate:

```console
$ cmake -S ~/code/libXISF -B /tmp/libxisf-build -DCMAKE_BUILD_TYPE=Release
$ cmake --build /tmp/libxisf-build -j
$ g++ -std=c++17 -O2 -o /tmp/xisf-generate tools/corpus-gen/generate.cpp \
      -I ~/code/libXISF -L /tmp/libxisf-build -lXISF
$ LD_LIBRARY_PATH=/tmp/libxisf-build /tmp/xisf-generate corpus/generated
```
