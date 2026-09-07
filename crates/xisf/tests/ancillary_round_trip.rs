//! Writing the ancillary elements, and reading them back unchanged.
//!
//! A reader that understands more than the writer can produce is a data loss
//! bug waiting for someone to open a file, change one property and save it:
//! everything the writer cannot express is dropped, quietly, at the moment of
//! saving. So the pair has to be closed, and this is the test that says so.

use xisf::{
    BlockOptions, CfaElement, ColorFilterArray, ColorSpace, DisplayFunction, Gamma, Image,
    PendingImage, PendingThumbnail, PixelStorage, Resolution, ResolutionUnit, RgbWorkingSpace,
    SampleFormat, Writer, XisfFile,
};

fn image(width: u64, height: u64, format: SampleFormat, space: ColorSpace) -> Image {
    Image {
        dimensions: vec![width, height],
        channels: space.nominal_channels(),
        sample_format: format,
        color_space: space,
        pixel_storage: PixelStorage::Planar,
        bounds: None,
        id: None,
        image_type: None,
        offset: None,
        orientation: None,
        uuid: None,
    }
}

/// Everything at once, so nothing is lost in the presence of anything else --
/// three blocks belong to this one image and their positions must not collide.
#[test]
fn every_ancillary_element_survives_a_round_trip() {
    let main = image(8, 4, SampleFormat::UInt16, ColorSpace::Rgb);
    let pixels = vec![0x5au8; 8 * 4 * 3 * 2];
    let thumb = image(2, 2, SampleFormat::UInt8, ColorSpace::Gray);
    let thumb_pixels: Vec<u8> = vec![1, 2, 3, 4];
    let profile: Vec<u8> = (0..64u8).collect();

    let space = RgbWorkingSpace {
        gamma: Gamma::Exponent(2.2),
        x: [0.648431, 0.230154, 0.155886],
        y: [0.330856, 0.701572, 0.066044],
        luminance: [0.311114, 0.625662, 0.063224],
        name: Some("Adobe RGB (1998)".into()),
    };
    let function = DisplayFunction {
        midtones: [0.25, 0.25, 0.25, 0.5],
        shadows: [0.01, 0.02, 0.03, 0.0],
        highlights: [0.9, 0.95, 1.0, 1.0],
        low_range: [0.0; 4],
        high_range: [1.0; 4],
        name: Some("Stretch".into()),
    };
    let cfa = ColorFilterArray {
        pattern: vec![CfaElement::Red, CfaElement::Green, CfaElement::Green, CfaElement::Blue],
        width: 2,
        height: 2,
        name: Some("RGGB".into()),
    };

    let mut writer = Writer::new().with_creator("xisf-rs test");
    writer
        .add_image(
            PendingImage::new(main, pixels.clone(), BlockOptions::default())
                .with_resolution(Resolution {
                    horizontal: 300.0,
                    vertical: 300.0,
                    unit: ResolutionUnit::Centimetre,
                })
                .with_rgb_working_space(space.clone())
                .with_display_function(function.clone())
                .with_color_filter_array(cfa.clone())
                .with_icc_profile(profile.clone())
                .with_thumbnail(PendingThumbnail::new(
                    thumb,
                    thumb_pixels.clone(),
                    BlockOptions::default(),
                ))
                .with_fits_keyword("EXPTIME", "300.0", "exposure in seconds"),
        )
        .expect("add_image");

    let file = XisfFile::from_bytes(writer.to_bytes().expect("write")).expect("read back");
    let read = &file.images()[0];

    // The pixel data first: the extra blocks must not have displaced it.
    assert_eq!(read.bytes().expect("pixels").as_ref(), pixels.as_slice());

    let resolution = read.resolution().expect("resolution");
    assert_eq!(resolution.horizontal, 300.0);
    assert_eq!(resolution.unit, ResolutionUnit::Centimetre);

    assert_eq!(read.rgb_working_space().expect("RGBWS"), space);
    assert_eq!(read.display_function().expect("display function"), function);
    assert_eq!(read.color_filter_array().expect("CFA"), cfa);

    assert_eq!(read.icc_profile().expect("ICC").expect("ICC block").as_ref(), profile.as_slice());

    let thumbnail = read.thumbnail().expect("thumbnail");
    assert_eq!(thumbnail.geometry(), &[2, 2]);
    assert_eq!(thumbnail.sample_format(), SampleFormat::UInt8);
    assert_eq!(thumbnail.bytes().expect("thumbnail block").as_ref(), thumb_pixels.as_slice());

    assert_eq!(
        read.fits_keywords(),
        vec![("EXPTIME".to_string(), "300.0".to_string(), "exposure in seconds".to_string())]
    );
}

/// `gamma="sRGB"` is a word, not a number, and must survive as one.
#[test]
fn an_srgb_working_space_round_trips_as_the_word() {
    let mut writer = Writer::new();
    writer
        .add_image(
            PendingImage::new(
                image(2, 2, SampleFormat::UInt8, ColorSpace::Gray),
                vec![0; 4],
                BlockOptions::default(),
            )
            .with_rgb_working_space(RgbWorkingSpace::srgb()),
        )
        .expect("add_image");

    let bytes = writer.to_bytes().expect("write");
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains(r#"gamma="sRGB""#), "gamma was not written as a word");

    let file = XisfFile::from_bytes(bytes).expect("read back");
    assert_eq!(file.images()[0].rgb_working_space().expect("RGBWS"), RgbWorkingSpace::srgb());
}

/// With several images each owning several blocks, every locator must point at
/// its own bytes. Off-by-one here hands one image another's data and reads
/// back without error, so the check is that the contents differ as written.
#[test]
fn blocks_belonging_to_different_images_do_not_collide() {
    let mut writer = Writer::new();
    for n in 0..3u8 {
        let profile = vec![0xa0 + n; 16 + n as usize];
        writer
            .add_image(
                PendingImage::new(
                    image(2, 2, SampleFormat::UInt8, ColorSpace::Gray),
                    vec![n; 4],
                    BlockOptions::default(),
                )
                .with_icc_profile(profile)
                .with_thumbnail(PendingThumbnail::new(
                    image(1, 1, SampleFormat::UInt8, ColorSpace::Gray),
                    vec![0xf0 + n],
                    BlockOptions::default(),
                )),
            )
            .expect("add_image");
    }

    let file = XisfFile::from_bytes(writer.to_bytes().expect("write")).expect("read back");
    for (n, read) in file.images().iter().enumerate() {
        let n = n as u8;
        assert_eq!(read.bytes().expect("pixels").as_ref(), vec![n; 4].as_slice());
        assert_eq!(
            read.icc_profile().expect("ICC").expect("block").as_ref(),
            vec![0xa0 + n; 16 + n as usize].as_slice()
        );
        assert_eq!(
            read.thumbnail().expect("thumbnail").bytes().expect("block").as_ref(),
            &[0xf0 + n]
        );
    }
}

/// The same, through a distributed unit, where blocks are named by identifier
/// rather than by position -- a separate addressing path with its own way to
/// be wrong.
#[test]
fn ancillary_blocks_survive_a_distributed_unit() {
    let mut writer = Writer::new();
    writer
        .add_image(
            PendingImage::new(
                image(2, 2, SampleFormat::UInt8, ColorSpace::Gray),
                vec![7; 4],
                BlockOptions::default(),
            )
            .with_icc_profile(vec![0xcc; 32]),
        )
        .expect("add_image");

    let unit = writer.to_distributed("blocks.xisb").expect("to_distributed");
    let dir = std::env::temp_dir().join(format!("xisf-ancillary-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch");
    std::fs::write(dir.join("unit.xish"), &unit.header).expect("header");
    std::fs::write(dir.join("blocks.xisb"), &unit.blocks).expect("blocks");

    let file = XisfFile::open(dir.join("unit.xish")).expect("open");
    let read = &file.images()[0];
    assert_eq!(read.bytes().expect("pixels").as_ref(), &[7, 7, 7, 7]);
    assert_eq!(
        read.icc_profile().expect("ICC").expect("block").as_ref(),
        vec![0xcc; 32].as_slice()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A thumbnail the spec would not allow is refused when it is added, rather
/// than written into a file a conforming reader may reject.
#[test]
fn an_invalid_thumbnail_is_refused() {
    let cases: Vec<(&str, PendingThumbnail)> = vec![
        ("three-dimensional", {
            let mut thumb = image(2, 2, SampleFormat::UInt8, ColorSpace::Gray);
            thumb.dimensions = vec![2, 2, 2];
            PendingThumbnail::new(thumb, vec![0; 8], BlockOptions::default())
        }),
        (
            "floating point",
            PendingThumbnail::new(
                image(2, 2, SampleFormat::Float32, ColorSpace::Gray),
                vec![0; 16],
                BlockOptions::default(),
            ),
        ),
        (
            "wrong data length",
            PendingThumbnail::new(
                image(2, 2, SampleFormat::UInt8, ColorSpace::Gray),
                vec![0; 3],
                BlockOptions::default(),
            ),
        ),
    ];

    for (why, thumbnail) in cases {
        let mut writer = Writer::new();
        let result = writer.add_image(
            PendingImage::new(
                image(2, 2, SampleFormat::UInt8, ColorSpace::Gray),
                vec![0; 4],
                BlockOptions::default(),
            )
            .with_thumbnail(thumbnail),
        );
        assert!(result.is_err(), "a {why} thumbnail was accepted");
    }
}

/// The spec requires `<Metadata>` to carry `XISF:CreationTime` and
/// `XISF:CreatorApplication`. Neither is conditional on the caller having
/// asked for it, so a file written with no metadata at all still conforms.
#[test]
fn the_mandatory_metadata_properties_are_always_written() {
    let mut writer = Writer::new().with_creation_time("2014-12-09T12:38:15Z");
    writer
        .add_image(PendingImage::new(
            image(2, 2, SampleFormat::UInt8, ColorSpace::Gray),
            vec![0; 4],
            BlockOptions::default(),
        ))
        .expect("add_image");

    let file = XisfFile::from_bytes(writer.to_bytes().expect("write")).expect("read back");
    let time = file.property("XISF:CreationTime").expect("XISF:CreationTime is mandatory");
    assert_eq!(time.as_str(), Some("2014-12-09T12:38:15Z"));
    assert_eq!(time.kind().shape, xisf::Shape::TimePoint);

    let creator = file.property("XISF:CreatorApplication").expect("creator is mandatory");
    assert!(!creator.as_str().unwrap_or_default().is_empty());
}

/// Left to the clock, the timestamp must still be a plausible instant rather
/// than the epoch or a malformed string.
#[test]
fn an_unset_creation_time_comes_from_the_clock() {
    let mut writer = Writer::new();
    writer
        .add_image(PendingImage::new(
            image(2, 2, SampleFormat::UInt8, ColorSpace::Gray),
            vec![0; 4],
            BlockOptions::default(),
        ))
        .expect("add_image");

    let file = XisfFile::from_bytes(writer.to_bytes().expect("write")).expect("read back");
    let value = file.property("XISF:CreationTime").expect("time").as_str().unwrap().to_string();

    assert_eq!(value.len(), 20, "expected YYYY-MM-DDThh:mm:ssZ, got {value:?}");
    assert!(value.ends_with('Z'), "{value:?}");
    let year: u32 = value[..4].parse().expect("a four-digit year");
    assert!((2026..2200).contains(&year), "implausible year in {value:?}");
}

/// The image attributes that used to be read but never written: dropping one
/// on a save is silent, and `offset` in particular changes the numbers a
/// calibration pipeline computes.
#[test]
fn image_attributes_survive_a_round_trip() {
    let mut source = image(4, 4, SampleFormat::UInt16, ColorSpace::Gray);
    source.offset = Some(512.0);
    source.orientation = Some(xisf::Orientation { rotation: -90, flip_horizontal: true });
    source.uuid = Some("c5c93b6d-9072-4e85-9548-1a5391377683".into());
    source.id = Some("light_0001".into());
    source.image_type = Some("Light".into());

    let mut writer = Writer::new();
    writer
        .add_image(PendingImage::new(source.clone(), vec![0; 32], BlockOptions::default()))
        .expect("add_image");

    let file = XisfFile::from_bytes(writer.to_bytes().expect("write")).expect("read back");
    let read = file.images()[0].attributes().clone();

    assert_eq!(read.offset, source.offset, "the offset was lost");
    assert_eq!(read.orientation, source.orientation, "the orientation was lost");
    assert_eq!(read.uuid, source.uuid, "the uuid was lost");
    assert_eq!(read.id, source.id);
    assert_eq!(read.image_type, source.image_type);
}
