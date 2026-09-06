//! Read the PixInsight sample files.
//!
//! These are the only files in the project not produced by it, which makes
//! them the only evidence that the engine reads what the format actually is
//! rather than what we assumed. Every assertion here is about a value
//! PixInsight wrote, not one we chose.

use std::path::PathBuf;

use xisf_core::block::{Codec, Location};
use xisf_core::reader::ChecksumStatus;
use xisf_core::{ErrorKind, Reader};

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/pixinsight")
}

fn open(name: &str) -> Reader {
    Reader::open(corpus().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn reads_an_embedded_base64_image() {
    let reader = open("simple_10by10.xisf");
    let images = reader.header().images();
    assert_eq!(images.len(), 1);

    let image = images[0];
    assert_eq!(image.attr("geometry"), Some("10:10:3"));
    assert_eq!(image.attr("sampleFormat"), Some("UInt8"));
    assert_eq!(image.attr("colorSpace"), Some("RGB"));
    assert_eq!(image.data.location, Some(Location::Embedded));

    // 10 x 10 pixels, 3 channels, one byte per sample.
    let data = reader.block(&image.data).expect("block");
    assert_eq!(data.len(), 10 * 10 * 3);
}

#[test]
fn reads_an_uncompressed_attached_image() {
    let reader = open("Sample_F32_NoCompression_NoSecurity.xisf");
    let image = reader.header().images()[0];

    assert_eq!(image.attr("sampleFormat"), Some("Float32"));
    assert!(
        matches!(image.data.location, Some(Location::Attachment { .. })),
        "expected an attached block, got {:?}",
        image.data.location
    );
    assert!(image.data.compression.is_none());

    let geometry = image.attr("geometry").expect("geometry");
    let expected = expected_bytes(geometry, 4);
    assert_eq!(reader.block(&image.data).expect("block").len(), expected);
}

/// The interesting one: byte-shuffled zlib, and a checksum to prove the bytes
/// came back exactly as PixInsight wrote them. Needs both compiled in.
#[cfg(all(feature = "zlib", feature = "checksums"))]
#[test]
fn reads_a_shuffled_zlib_image_and_verifies_its_checksum() {
    let reader = open("Sample_F32_ZlibCompression_Sha256Security.xisf");
    let image = reader.header().images()[0];

    let compression = image.data.compression.clone().expect("compression");
    assert_eq!(compression.codec, Codec::Zlib);
    assert_eq!(
        compression.shuffle_item_size,
        Some(4),
        "Float32 samples shuffle four bytes at a time"
    );

    // The checksum is over the stored (compressed) bytes, so this passing
    // means the block was located correctly.
    assert_eq!(
        reader.verify(&image.data).expect("verify"),
        ChecksumStatus::Valid,
        "the recorded sha-256 did not match the stored block"
    );

    // And this passing means it was decompressed and unshuffled correctly.
    let data = reader.block(&image.data).expect("block");
    assert_eq!(data.len() as u64, compression.uncompressed_size);

    let geometry = image.attr("geometry").expect("geometry");
    assert_eq!(data.len(), expected_bytes(geometry, 4));
}

#[test]
fn fits_keywords_survive_the_header() {
    let reader = open("Sample_F32_ZlibCompression_Sha256Security.xisf");
    let image = reader.header().images()[0];

    let keywords: Vec<_> = image.children_named("FITSKeyword").collect();
    assert!(!keywords.is_empty(), "this file has FITS keywords");
    let simple = keywords.iter().find(|k| k.attr("name") == Some("SIMPLE"));
    assert_eq!(simple.and_then(|k| k.attr("value")), Some("T"));
}

/// Every sample file, read end to end, with checksums honoured.
#[test]
fn every_sample_file_reads_and_verifies() {
    let (mut checked, mut skipped) = (0, 0);
    for entry in std::fs::read_dir(corpus()).expect("corpus directory").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "xisf") {
            continue;
        }
        let reader = Reader::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));

        for element in reader.header().root.descendants() {
            if element.data.location.is_none() {
                continue;
            }
            // Only when a hash implementation is compiled in; otherwise
            // `verify` correctly reports that it cannot check.
            if cfg!(feature = "checksums") {
                let status = reader
                    .verify(&element.data)
                    .unwrap_or_else(|e| panic!("{}: verify: {e}", path.display()));
                assert!(!status.is_failure(), "{}: checksum mismatch", path.display());
            }

            match reader.block(&element.data) {
                Ok(_) => checked += 1,
                // A codec or hash left out of the build is not a corpus
                // failure; the library says so plainly and that is correct.
                Err(e) if e.kind() == ErrorKind::Unsupported => skipped += 1,
                Err(e) => panic!("{}: block: {e}", path.display()),
            }
        }
    }
    eprintln!("corpus: {checked} data blocks read and verified, {skipped} skipped");
    assert!(
        checked + skipped >= 3,
        "expected at least one block per sample file, saw {}",
        checked + skipped
    );
}

/// Samples are pixels x channels x bytes-per-sample. `geometry` is
/// `width:height:channels`.
fn expected_bytes(geometry: &str, bytes_per_sample: usize) -> usize {
    let dims: Vec<usize> = geometry.split(':').map(|d| d.parse().unwrap()).collect();
    dims.iter().product::<usize>() * bytes_per_sample
}
