# xisf-rs

[![Crates.io](https://img.shields.io/crates/v/xisf.svg)](https://crates.io/crates/xisf)
[![CI](https://github.com/cruzzil/xisf/actions/workflows/ci.yml/badge.svg)](https://github.com/cruzzil/xisf/actions/workflows/ci.yml)
[![Documentation](https://docs.rs/xisf/badge.svg)](https://docs.rs/xisf/)
[![codecov](https://codecov.io/gh/cruzzil/xisf/graph/badge.svg)](https://codecov.io/gh/cruzzil/xisf)
[![Dependency status](https://deps.rs/repo/github/cruzzil/xisf/status.svg)](https://deps.rs/repo/github/cruzzil/xisf)
[![MSRV](https://img.shields.io/badge/MSRV-1.90-blue)](https://blog.rust-lang.org/)

A Rust implementation of [XISF](https://pixinsight.com/xisf/), the Extensible
Image Serialization Format: an XML header naming images and properties,
followed by the binary blocks that hold them. It is PixInsight's native format
and is used widely in astrophotography.

Written from the published specification, which states that anyone may
implement XISF freely. Nothing here derives from libXISF (GPL-3.0) or from the
PixInsight Class Library; see [`docs/PLAN.md`](docs/PLAN.md) for the reasoning.

## Layout

| Crate | What it is |
|---|---|
| `xisf-core` | The engine. Layout, header, data blocks, codecs, checksums, properties, images. All the behaviour lives here. |
| `xisf` | The idiomatic Rust API. |
| `libxisf` | A C API of our own design. Builds `libxisf.so`. **Not** the unrelated C++ library of that name — see below. |
| `xisftool` | The command-line tool of the same name. |

## Using it from Rust

```toml
[dependencies]
xisf = "0.5"
```

```rust
use xisf::XisfFile;

let file = XisfFile::open("image.xisf")?;
for image in file.images() {
    println!("{:?} {}", image.geometry(), image.sample_format().name());
    let pixels: Vec<u16> = image.read()?;
}
```

## Using it from C

```c
#include <xisf.h>

xisf_error_t err;
xisf_file_t *file = xisf_open("image.xisf", &err);
const xisf_image_t *image = xisf_image_at(file, 0);

size_t size;
void *pixels = xisf_image_read_alloc(image, &size, &err);
/* ... */
xisf_free(pixels);
xisf_close(file);
```

**`libxisf` here is not the C++ libXISF by Dušan Poizl.** That library exposes
only C++ classes, so its symbols are all mangled and these are all plain C
names; the two sets are disjoint, and a program built against one and linked
against the other fails at link time rather than misbehaving. `crates/libxisf/include/xisf.h`
says so at the top.

## The command-line tool

```console
$ xisftool info image.xisf
$ xisftool info -v image.xisf     # with FITS keywords and properties
$ xisftool header image.xisf      # the raw XML
$ xisftool verify *.xisf          # check every recorded checksum
$ xisftool dump image.xisf > pixels.raw
```

## How correctness is judged

Three independent implementations are used as oracles, because an
implementation that only agrees with itself has shown nothing — a reader and
writer sharing a misunderstanding round-trip perfectly.

- **PixInsight's own files.** Three samples in `corpus/pixinsight/`, covering
  embedded base64, attached blocks, byte-shuffled zlib and SHA-256.
- **libXISF, both directions.** `tools/corpus-gen` writes a matrix of 40 files
  across five sample formats and seven codecs, all of which read back **byte
  for byte**; and libXISF reads **52 of 52** files we write, one of which
  carries every ancillary element, where the check is that libXISF *finds*
  the ICC profile, thumbnail and FITS keywords rather than merely parsing the
  file. It is GPL-3.0, so it is used as a black box and never linked.
- **`seiza-xisf`**, a third implementation, as a pure-Rust dev-dependency that
  runs on every platform.

Plus a C conformance harness compiling real C against the header with
`-Werror`, Miri over the FFI layer, benchmarks, and CI on six platforms: Linux,
macOS and Windows, each on x86-64 and aarch64.

Headers are also validated against the official XML Schema, which Revision 1
of the specification publishes. It is fetched rather than kept here: the file
is all-rights-reserved with no redistribution grant, so `scripts/fetch-xsd.sh`
downloads it into a gitignored directory and `scripts/validate-headers.sh`
runs the check. CI validates both the corpus and a file this writer produced,
so the schema gates what we emit as well as what we can read.

## Documentation

| | |
|---|---|
| [`docs/PLAN.md`](docs/PLAN.md) | Research findings, the licensing analysis, and the plan. |
| [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md) | Benchmarks and the costs left unaddressed. |
| [`corpus/README.md`](corpus/README.md) | Where the test files come from. |

## Licence

MIT. `tools/corpus-gen` is GPL-3.0 because it links libXISF; it is a
development tool and nothing under `crates/` builds or links it.
