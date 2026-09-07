//! Files we write, read by libXISF.
//!
//! The corpus tests prove we can read what libXISF wrote. This proves the
//! other direction, and no test written in Rust can: a reader and writer that
//! share a misunderstanding still round-trip through each other perfectly.
//! Only a second implementation notices.
//!
//! It needs `tools/corpus-gen/verify.cpp` built against libXISF, which links
//! GPL-3.0 code and so is never part of the library. Point `XISF_VERIFY` at
//! the binary, and `LD_LIBRARY_PATH` at libXISF if it is not installed:
//!
//! ```console
//! $ g++ -std=c++17 -O2 -o /tmp/xisf-verify-cpp tools/corpus-gen/verify.cpp \
//!       -I ~/code/libXISF -L /tmp/libxisf-build -lXISF
//! $ XISF_VERIFY=/tmp/xisf-verify-cpp LD_LIBRARY_PATH=/tmp/libxisf-build \
//!       cargo test -p xisf-core --test round_trip_libxisf -- --nocapture
//! ```
//!
//! Without it the test skips with a note rather than failing, so a bare
//! checkout stays green -- which does mean a green run here is weaker
//! evidence than a green run with it. CI says which it got.

use std::path::PathBuf;
use std::process::Command;

use xisf_core::block::ChecksumAlgorithm;
use xisf_core::image::{
    ColorSpace, DisplayFunction, Image, PixelStorage, Resolution, ResolutionUnit, RgbWorkingSpace,
    SampleFormat,
};
use xisf_core::writer::{
    BlockOptions, Codec2, CompressionRequest, PendingImage, PendingThumbnail, Writer,
};

fn verifier() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("XISF_VERIFY")?);
    path.is_file().then_some(path)
}

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

#[test]
fn libxisf_reads_what_we_write() {
    let Some(verify) = verifier() else {
        eprintln!("skipping: set XISF_VERIFY to the built tools/corpus-gen/verify binary");
        return;
    };

    let dir = std::env::temp_dir().join(format!("xisf-rs-roundtrip-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp directory");

    // The matrix is the same shape as the generated corpus, so a failure here
    // can be compared directly against the read direction.
    let mut cases = Vec::new();
    for format in [
        SampleFormat::UInt8,
        SampleFormat::UInt16,
        SampleFormat::UInt32,
        SampleFormat::Float32,
        SampleFormat::Float64,
    ] {
        // Only what this build can write. Asserting that an absent codec
        // round-trips would be testing the build configuration, not the code.
        let mut compressions = vec![None];
        if cfg!(feature = "zlib") {
            compressions
                .push(Some(CompressionRequest { codec: Codec2::Zlib, shuffle_item_size: None }));
            compressions.push(Some(CompressionRequest {
                codec: Codec2::Zlib,
                shuffle_item_size: Some(format.size() as u64),
            }));
        }
        if cfg!(feature = "lz4") {
            compressions
                .push(Some(CompressionRequest { codec: Codec2::Lz4, shuffle_item_size: None }));
            compressions.push(Some(CompressionRequest {
                codec: Codec2::Lz4,
                shuffle_item_size: Some(format.size() as u64),
            }));
        }
        let checksums = if cfg!(feature = "checksums") {
            vec![None, Some(ChecksumAlgorithm::Sha256)]
        } else {
            vec![None]
        };

        for compression in compressions {
            for checksum in &checksums {
                cases.push((format, compression, *checksum));
            }
        }
    }
    // One multi-channel case per run, since geometry is where a writer most
    // easily disagrees with a reader.
    cases.push((SampleFormat::UInt16, None, None));

    let (mut passed, mut failures) = (0usize, Vec::new());

    for (index, (format, compression, checksum)) in cases.iter().enumerate() {
        let channels = if index == cases.len() - 1 { 3 } else { 1 };
        let image = image(23, 19, channels, *format);
        let expected = image.data_size().expect("size");
        let data: Vec<u8> = (0..expected as usize).map(|i| (i * 29 + 3) as u8).collect();

        let mut writer = Writer::new().with_creator("xisf-rs");
        writer
            .add_image(PendingImage::new(
                image,
                data,
                BlockOptions { compression: *compression, checksum: *checksum },
            ))
            .expect("add_image");
        let bytes = writer.to_bytes().expect("to_bytes");

        let name = format!(
            "{}_{}_{}{}.xisf",
            format.name(),
            compression.map_or("none".into(), |c| format!(
                "{:?}{}",
                c.codec,
                if c.shuffle_item_size.is_some() { "+sh" } else { "" }
            )),
            if checksum.is_some() { "sum" } else { "nosum" },
            if channels == 3 { "_rgb" } else { "" }
        );
        let path = dir.join(&name);
        std::fs::write(&path, &bytes).expect("write");

        let output = Command::new(&verify)
            .arg(&path)
            .arg(expected.to_string())
            .output()
            .expect("run the verifier");

        if output.status.success() {
            passed += 1;
        } else {
            failures.push(format!(
                "{name}: {}{}",
                String::from_utf8_lossy(&output.stderr).trim(),
                String::from_utf8_lossy(&output.stdout).trim()
            ));
        }
    }

    // One file carrying every ancillary element. These changed the shape of
    // the emitted XML -- an `<Image>` with children rather than a self-closing
    // one, and three data blocks belonging to a single image -- so an
    // independent reader is the check that the change is still conforming and
    // not merely self-consistent.
    {
        let main = image(23, 19, 3, SampleFormat::UInt16);
        let expected = main.data_size().expect("size");
        let data: Vec<u8> = (0..expected as usize).map(|i| (i * 7 + 1) as u8).collect();
        let thumbnail = image(8, 8, 1, SampleFormat::UInt8);
        let thumbnail_data = vec![0x40u8; thumbnail.data_size().expect("size") as usize];

        let mut writer = Writer::new().with_creator("xisf-rs");
        writer
            .add_image(
                PendingImage::new(main, data, BlockOptions::default())
                    .with_resolution(Resolution {
                        horizontal: 300.0,
                        vertical: 300.0,
                        unit: ResolutionUnit::Inch,
                    })
                    .with_rgb_working_space(RgbWorkingSpace::srgb())
                    .with_display_function(DisplayFunction::identity())
                    .with_icc_profile(vec![0u8; 128])
                    .with_thumbnail(PendingThumbnail::new(
                        thumbnail,
                        thumbnail_data,
                        BlockOptions::default(),
                    ))
                    .with_fits_keyword("EXPTIME", "300.0", "seconds"),
            )
            .expect("add_image");

        let path = dir.join("ancillary.xisf");
        std::fs::write(&path, writer.to_bytes().expect("to_bytes")).expect("write");
        // `--ancillary` makes the verifier require that libXISF actually
        // found the profile, thumbnail and keywords, not merely that the file
        // parsed: an element another implementation cannot see is, for that
        // implementation, an element that is not there.
        let output = Command::new(&verify)
            .arg(&path)
            .arg(expected.to_string())
            .arg("--ancillary")
            .output()
            .expect("run the verifier");
        if output.status.success() {
            passed += 1;
        } else {
            failures.push(format!(
                "ancillary.xisf: {}{}",
                String::from_utf8_lossy(&output.stderr).trim(),
                String::from_utf8_lossy(&output.stdout).trim()
            ));
        }
    }
    let total = cases.len() + 1;

    let _ = std::fs::remove_dir_all(&dir);

    eprintln!("libXISF read {passed} of {total} files we wrote");
    assert!(
        failures.is_empty(),
        "libXISF could not read {} of the files we wrote:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
