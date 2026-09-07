//! Throughput of the paths that move bulk pixel data.
//!
//! Run with `cargo bench -p xisf`, or one group with
//! `cargo bench -p xisf -- read`.
//!
//! # Why these exist from the first commit
//!
//! In a sibling project the typed array read decoded every element into an
//! enum and then converted it, which ran twelve times slower than the
//! reference implementation. Every test passed: the numbers were right, there
//! were just far too many instructions between the file and the caller. No
//! test notices that, and it went unnoticed until benchmarks were added late.
//!
//! So the fast path here was written first and this measures it, rather than
//! the other way round. The two figures to compare are `read_native_u16`
//! against `raw_bytes`: the first should be within a small factor of the
//! second, because on a little-endian host reading little-endian samples there
//! is nothing to do but copy.

use xisf::{ColorSpace, Image, PixelStorage, SampleFormat, XisfFile};
use xisf_core::writer::{BlockOptions, Codec2, CompressionRequest, PendingImage, Writer};

fn main() {
    divan::main();
}

/// Four million samples: 8 MB of `u16`, 32 MB of `f64`. Large enough that
/// per-call overhead disappears.
const SAMPLES: u64 = 4_000_000;

fn image_of(format: SampleFormat) -> Image {
    Image {
        dimensions: vec![2000, 2000],
        channels: 1,
        sample_format: format,
        color_space: ColorSpace::Gray,
        pixel_storage: PixelStorage::Planar,
        bounds: None,
        id: None,
        uuid: None,
        image_type: None,
        offset: None,
        orientation: None,
    }
}

/// Deterministic, and not trivially compressible: a run of zeroes would make
/// the compression benchmarks measure nothing.
fn pixels(bytes: usize) -> Vec<u8> {
    (0..bytes).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect()
}

fn build(format: SampleFormat, compression: Option<CompressionRequest>) -> XisfFile {
    let image = image_of(format);
    let data = pixels(image.data_size().expect("size") as usize);
    let mut writer = Writer::new();
    writer
        .add_image(PendingImage::new(image, data, BlockOptions { compression, checksum: None }))
        .expect("add_image");
    XisfFile::from_bytes(writer.to_bytes().expect("to_bytes")).expect("read back")
}

#[divan::bench_group(name = "read")]
mod read {
    use super::*;

    /// The floor: block bytes straight out of the mapping, nothing decoded.
    #[divan::bench(bytes_count = SAMPLES * 2)]
    fn raw_bytes(bencher: divan::Bencher) {
        let file = build(SampleFormat::UInt16, None);
        bencher.bench(|| file.images()[0].bytes().expect("bytes").len());
    }

    /// Typed samples in the machine's own order. Should be close to the floor
    /// above -- there is nothing to do but copy.
    #[divan::bench(bytes_count = SAMPLES * 2)]
    fn read_native_u16(bencher: divan::Bencher) {
        let file = build(SampleFormat::UInt16, None);
        bencher.bench(|| file.images()[0].read::<u16>().expect("read").len());
    }

    #[divan::bench(bytes_count = SAMPLES * 4)]
    fn read_native_f32(bencher: divan::Bencher) {
        let file = build(SampleFormat::Float32, None);
        bencher.bench(|| file.images()[0].read::<f32>().expect("read").len());
    }

    #[divan::bench(bytes_count = SAMPLES * 8)]
    fn read_native_f64(bencher: divan::Bencher) {
        let file = build(SampleFormat::Float64, None);
        bencher.bench(|| file.images()[0].read::<f64>().expect("read").len());
    }

    /// Decompression-bound, for scale against the uncompressed figures.
    #[divan::bench(bytes_count = SAMPLES * 2)]
    fn read_zlib(bencher: divan::Bencher) {
        let file = build(
            SampleFormat::UInt16,
            Some(CompressionRequest { codec: Codec2::Zlib, shuffle_item_size: Some(2) }),
        );
        bencher.bench(|| file.images()[0].read::<u16>().expect("read").len());
    }

    #[divan::bench(bytes_count = SAMPLES * 2)]
    fn read_lz4(bencher: divan::Bencher) {
        let file = build(
            SampleFormat::UInt16,
            Some(CompressionRequest { codec: Codec2::Lz4, shuffle_item_size: Some(2) }),
        );
        bencher.bench(|| file.images()[0].read::<u16>().expect("read").len());
    }

    /// Checksum verification, which is over the stored bytes.
    #[divan::bench(bytes_count = SAMPLES * 2)]
    fn verify_sha256(bencher: divan::Bencher) {
        let image = image_of(SampleFormat::UInt16);
        let data = pixels(image.data_size().unwrap() as usize);
        let mut writer = Writer::new();
        writer
            .add_image(PendingImage::new(
                image,
                data,
                BlockOptions { compression: None, checksum: Some(xisf::ChecksumAlgorithm::Sha256) },
            ))
            .unwrap();
        let file = XisfFile::from_bytes(writer.to_bytes().unwrap()).unwrap();
        bencher.bench(|| file.images()[0].verify().expect("verify"));
    }
}

#[divan::bench_group(name = "write")]
mod write {
    use super::*;

    #[divan::bench(bytes_count = SAMPLES * 2)]
    fn whole_file(bencher: divan::Bencher) {
        let image = image_of(SampleFormat::UInt16);
        let data = pixels(image.data_size().unwrap() as usize);
        bencher.bench(|| {
            let mut writer = Writer::new();
            writer
                .add_image(PendingImage::new(image.clone(), data.clone(), BlockOptions::default()))
                .unwrap();
            writer.to_bytes().unwrap().len()
        });
    }

    #[divan::bench(bytes_count = SAMPLES * 2, args = ["zlib", "lz4"])]
    fn compressed(bencher: divan::Bencher, codec: &str) {
        let image = image_of(SampleFormat::UInt16);
        let data = pixels(image.data_size().unwrap() as usize);
        let request = CompressionRequest {
            codec: if codec == "zlib" { Codec2::Zlib } else { Codec2::Lz4 },
            shuffle_item_size: Some(2),
        };
        bencher.bench(|| {
            let mut writer = Writer::new();
            writer
                .add_image(PendingImage::new(
                    image.clone(),
                    data.clone(),
                    BlockOptions { compression: Some(request), checksum: None },
                ))
                .unwrap();
            writer.to_bytes().unwrap().len()
        });
    }
}

/// Byte shuffling on its own, since it touches every byte twice and is the
/// least obvious cost in the compressed paths.
#[divan::bench_group(name = "shuffle")]
mod shuffle {
    use super::*;

    #[divan::bench(bytes_count = SAMPLES * 2, args = [2usize, 4, 8])]
    fn round_trip(bencher: divan::Bencher, item: usize) {
        let data = pixels(SAMPLES as usize * 2);
        bencher.bench(|| {
            let shuffled = xisf_core::codec::shuffle(&data, item);
            xisf_core::codec::unshuffle(&shuffled, item).len()
        });
    }
}
