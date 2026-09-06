# xisf-rs

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
| `xisf-cli` | The `xisf` command-line tool. |

## Using it from Rust

```toml
[dependencies]
xisf = "0.1"
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
$ xisf info image.xisf
$ xisf info -v image.xisf     # with FITS keywords and properties
$ xisf header image.xisf      # the raw XML
$ xisf verify *.xisf          # check every recorded checksum
$ xisf dump image.xisf > pixels.raw
```

## How correctness is judged

Three independent implementations are used as oracles, because an
implementation that only agrees with itself has shown nothing — a reader and
writer sharing a misunderstanding round-trip perfectly.

- **PixInsight's own files.** Three samples in `corpus/pixinsight/`, covering
  embedded base64, attached blocks, byte-shuffled zlib and SHA-256.
- **libXISF, both directions.** `tools/corpus-gen` writes a matrix of 40 files
  across five sample formats and seven codecs, all of which read back **byte
  for byte**; and libXISF reads **51 of 51** files we write. It is GPL-3.0, so
  it is used as a black box and never linked.
- **`seiza-xisf`**, a third implementation, as a pure-Rust dev-dependency that
  runs on every platform.

Plus a C conformance harness compiling real C against the header with
`-Werror`, Miri over the FFI layer, benchmarks, and CI on six platforms: Linux,
macOS and Windows, each on x86-64 and aarch64.

The specification references an XSD schema; it is unpublished (404 at the URL
every XISF file names), so it cannot be used as a gate. See `docs/PLAN.md`.

## Documentation

| | |
|---|---|
| [`docs/PLAN.md`](docs/PLAN.md) | Research findings, the licensing analysis, and the plan. |
| [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md) | Benchmarks and the costs left unaddressed. |
| [`corpus/README.md`](corpus/README.md) | Where the test files come from. |

## Licence

MIT. `tools/corpus-gen` is GPL-3.0 because it links libXISF; it is a
development tool and nothing under `crates/` builds or links it.
