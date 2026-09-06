# Performance

```console
$ cargo bench -p xisf              # everything
$ cargo bench -p xisf -- read      # one group
```

Benchmarks live in `crates/xisf/benches/throughput.rs` and use
[divan](https://crates.io/crates/divan) as a dev-dependency, so nothing is
added to what a consumer builds.

## Why they exist from the first commit

In a sibling project the typed array read decoded every element into an enum
and then converted it. It ran **twelve times slower than the reference
implementation**, and every test passed -- the numbers were right, there were
just far too many instructions between the file and the caller. No test
notices that, and nothing did until benchmarks were added late.

So here the bulk read path was written first and these measure it. The
comparison that matters is `read_native_u16` against `raw_bytes`: reading
little-endian samples on a little-endian host has nothing to do but copy, so
the typed read should be memory-bandwidth bound rather than a factor slower.

## Measurements

4M samples (8 MB of `u16`, 32 MB of `f64`), x86-64, release, medians:

| | Throughput | |
|---|---|---|
| `bytes()` (raw block) | 281 ns | borrowed from the mapping; nothing copied |
| `read::<u16>()` | 7.98 GB/s | the bulk path |
| `read::<f32>()` | 8.22 GB/s | |
| `read::<f64>()` | 7.51 GB/s | |
| `read::<u16>()` via zlib | 1.11 GB/s | decompression-bound |
| `read::<u16>()` via lz4 | 975 MB/s | decompression-bound |
| `verify()` (sha-256) | 1.98 GB/s | |
| write, uncompressed | 2.20 GB/s | |
| write, lz4 | 772 MB/s | |
| write, zlib | 236 MB/s | |
| shuffle + unshuffle | 757 MB/s | both directions, `u16` items |

Around 8 GB/s for a typed read is memory-bandwidth class, which is what the
fast path is for. It is roughly 28,000 times slower than `bytes()` only
because that borrows rather than copying -- the honest comparison is against
`memcpy`, and it is close to it.

## Known costs, not yet addressed

- **Byte shuffling is the slowest non-codec step**, at about 757 MB/s for a
  shuffle and unshuffle pair. It is a byte-at-a-time transpose and could be
  done a word at a time, but it is only paid on compressed blocks, where the
  codec dominates anyway.
- **zlib writes at 236 MB/s**, roughly nine times slower than writing
  uncompressed. That is flate2's cost at the default level, not this crate's;
  lz4 costs about a third as much for a weaker ratio.
- **`read` allocates a fresh `Vec`.** For a caller streaming many images that
  is a copy that could be avoided with a read-into-slice variant. Nobody has
  needed it yet.
