//! Write a file exercising as much of the header as this writer can produce.
//!
//! This exists for `scripts/validate-headers.sh`. Validating the corpus checks
//! that we can *read* what other implementations write; only a file we wrote
//! ourselves checks that what we *emit* satisfies the authority's own schema,
//! and a header with one uncompressed grey image would not exercise much of
//! it. So this turns on as many attributes and child elements as the writer
//! knows how to produce.
//!
//! Run as: `cargo run --example write-showcase -p xisf-core -- out.xisf`
use xisf_core::block::ChecksumAlgorithm;
use xisf_core::image::*;
use xisf_core::writer::*;

fn img(w: u64, h: u64, f: SampleFormat, cs: ColorSpace, ch: u64) -> Image {
    Image {
        dimensions: vec![w, h],
        channels: ch,
        sample_format: f,
        color_space: cs,
        pixel_storage: PixelStorage::Planar,
        bounds: if f.is_float() { Some(Bounds { low: 0.0, high: 1.0 }) } else { None },
        id: None,
        uuid: None,
        image_type: None,
        offset: None,
        orientation: None,
    }
}

fn main() {
    let mut w = Writer::new();
    let base = img(4, 4, SampleFormat::UInt16, ColorSpace::Rgb, 3);
    let n = base.data_size().unwrap() as usize;

    let mut a = base.clone();
    a.id = Some("Light".into());
    a.image_type = Some(ImageType::MasterLight);
    a.orientation = Some(Orientation { rotation: 90, flip_horizontal: true });
    a.uuid = Some("f81d4fae-7dec-11d0-a765-00a0c91e6bf6".into());
    a.offset = Some(0.0);

    let thumb = img(2, 2, SampleFormat::UInt8, ColorSpace::Gray, 1);
    let tn = thumb.data_size().unwrap() as usize;

    w.add_image(
        PendingImage::new(
            a,
            vec![7u8; n],
            BlockOptions {
                compression: Some(CompressionRequest {
                    codec: Codec2::Zstd,
                    shuffle_item_size: Some(2),
                }),
                checksum: Some(ChecksumAlgorithm::Sha256),
            },
        )
        .with_resolution(Resolution {
            horizontal: 300.0,
            vertical: 300.0,
            unit: ResolutionUnit::Inch,
        })
        .with_rgb_working_space(RgbWorkingSpace::srgb())
        .with_display_function(DisplayFunction::identity())
        .with_thumbnail(PendingThumbnail::new(thumb, vec![1u8; tn], BlockOptions::default()))
        .with_fits_keyword("OBJECT", "M31", "the target")
        .with_icc_profile(vec![0u8; 12]),
    )
    .expect("add_image");

    let mut b = base.clone();
    b.id = Some("Dark".into());
    w.add_image(PendingImage::new(
        b,
        vec![3u8; n],
        BlockOptions { compression: None, checksum: Some(ChecksumAlgorithm::Sha512) },
    ))
    .expect("b");

    w.add_metadata("Instrument:Telescope:Name", "Newtonian").expect("metadata");
    let bytes = w.to_bytes().expect("to_bytes");
    std::fs::write(std::env::args().nth(1).expect("out path"), &bytes).expect("write");
    eprintln!("wrote {} bytes", bytes.len());
}
