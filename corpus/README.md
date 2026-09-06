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
