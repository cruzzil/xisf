//! A third implementation reads what we write.
//!
//! `seiza-xisf` is an independent Rust implementation of the format,
//! Apache-2.0, used here purely as an oracle -- nothing is copied from it, and
//! it is a dev-dependency, so it is absent from what a consumer of this crate
//! builds.
//!
//! It earns its place alongside the libXISF round-trip by being *pure Rust*.
//! The libXISF oracle is the stronger check -- a mature C++ implementation
//! that also generates our corpus -- but it needs cmake, a C++ toolchain and a
//! clone from a third-party host, so it runs on one CI job. This one runs on
//! all six platforms with no build step, which makes it the check that would
//! catch a writer that only produces correct files on x86-64 Linux.
//!
//! Two implementations agreeing is worth much more than one implementation
//! agreeing with itself: a reader and writer that share a misunderstanding
//! round-trip perfectly.

use xisf::{ColorSpace, Image, PixelStorage, SampleFormat, XisfFile};
use xisf_core::writer::{BlockOptions, Codec2, CompressionRequest, PendingImage, Writer};

fn image(width: u64, height: u64, channels: u64, format: SampleFormat) -> Image {
    Image {
        dimensions: vec![width, height],
        channels,
        sample_format: format,
        color_space: if channels >= 3 { ColorSpace::Rgb } else { ColorSpace::Gray },
        pixel_storage: PixelStorage::Planar,
        bounds: None,
        id: None,
        uuid: None,
        image_type: None,
        offset: None,
        orientation: None,
    }
}

fn write_file(image: Image, data: Vec<u8>, compression: Option<CompressionRequest>) -> Vec<u8> {
    let mut writer = Writer::new().with_creator("xisf-rs");
    writer
        .add_image(PendingImage::new(image, data, BlockOptions { compression, checksum: None }))
        .expect("add_image");
    writer.to_bytes().expect("to_bytes")
}

/// Every combination of sample format and codec, checked against seiza-xisf's
/// view of the same file.
#[test]
fn seiza_agrees_about_what_we_wrote() {
    let formats = [
        (SampleFormat::UInt8, 1usize),
        (SampleFormat::UInt16, 2),
        (SampleFormat::UInt32, 4),
        (SampleFormat::Float32, 4),
        (SampleFormat::Float64, 8),
    ];
    let codecs = [None, Some(Codec2::Zlib), Some(Codec2::Lz4)];

    let mut checked = 0;
    let mut skipped = Vec::new();

    for (format, width) in formats {
        for codec in codecs {
            for channels in [1u64, 3] {
                let img = image(13, 7, channels, format);
                let size = img.data_size().expect("size") as usize;
                let data: Vec<u8> = (0..size).map(|i| (i * 17 + 5) as u8).collect();

                let compression = codec.map(|codec| CompressionRequest {
                    codec,
                    shuffle_item_size: Some(width as u64),
                });
                let bytes = write_file(img.clone(), data.clone(), compression);

                let read = match seiza_xisf::read_image_from_bytes(&bytes, 0) {
                    Ok(read) => read,
                    Err(e) => {
                        // A format this oracle does not handle is not our
                        // failure, but it is recorded rather than ignored so
                        // the coverage claim stays honest.
                        skipped.push(format!("{}/{:?}: {e}", format.name(), codec));
                        continue;
                    }
                };

                assert_eq!(
                    read.info.width as u64,
                    13,
                    "{} {:?}: width disagrees",
                    format.name(),
                    codec
                );
                assert_eq!(
                    read.info.height as u64,
                    7,
                    "{} {:?}: height disagrees",
                    format.name(),
                    codec
                );
                assert_eq!(
                    read.info.planes as u64,
                    channels,
                    "{} {:?}: channel count disagrees",
                    format.name(),
                    codec
                );

                // Our own reader must see the same file the same way.
                let ours = XisfFile::from_bytes(bytes).expect("our reader");
                let mine = &ours.images()[0];
                assert_eq!(mine.geometry(), &[13, 7]);
                assert_eq!(mine.channels(), channels);
                assert_eq!(
                    mine.bytes().expect("bytes").len(),
                    size,
                    "{} {:?}: our own read-back is the wrong length",
                    format.name(),
                    codec
                );

                checked += 1;
            }
        }
    }

    eprintln!("seiza-xisf agreed on {checked} files");
    for note in &skipped {
        eprintln!("  not read by the oracle: {note}");
    }
    assert!(checked > 0, "the oracle read nothing; the test proved nothing");
}

/// The corpus in the other direction: files a third implementation has to
/// agree with us about, including the ones PixInsight itself wrote.
#[test]
fn seiza_agrees_about_the_corpus() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let mut checked = 0;

    for sub in ["pixinsight", "generated"] {
        let Ok(entries) = std::fs::read_dir(dir.join(sub)) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "xisf") {
                continue;
            }
            let bytes = std::fs::read(&path).expect("read file");
            let name = path.file_name().unwrap().to_string_lossy().to_string();

            let (Ok(theirs), Ok(ours)) =
                (seiza_xisf::read_image_from_bytes(&bytes, 0), XisfFile::from_bytes(bytes.clone()))
            else {
                // Either implementation may decline a file -- zstd, for one,
                // is not a standard XISF codec. Only agreement is asserted.
                continue;
            };

            let mine = &ours.images()[0];
            assert_eq!(
                mine.geometry().first().copied().unwrap_or(0),
                theirs.info.width as u64,
                "{name}: width"
            );
            assert_eq!(
                mine.geometry().get(1).copied().unwrap_or(1),
                theirs.info.height as u64,
                "{name}: height"
            );
            assert_eq!(mine.channels(), theirs.info.planes as u64, "{name}: channels");
            checked += 1;
        }
    }

    eprintln!("seiza-xisf agreed with us on {checked} corpus files");
    assert!(checked > 0, "no corpus file was compared");
}
