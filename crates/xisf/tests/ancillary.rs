//! Resolution, ICC profiles and thumbnails, against real PixInsight files.
//!
//! These are not exotic: every PixInsight file in the corpus carries a
//! `<Resolution>`, and two of the three carry an ICC profile and a thumbnail.
//! Missing them means a reader silently drops the colour management and the
//! preview from every file the format's own reference implementation writes.
//!
//! The assertions are against what PixInsight actually wrote rather than
//! against a fixture of our own, which is the only way this says anything.

use std::path::PathBuf;

use xisf::{ResolutionUnit, XisfFile};

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/pixinsight")
}

fn open(name: &str) -> XisfFile {
    XisfFile::open(corpus().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn every_pixinsight_file_declares_a_resolution() {
    let mut seen = 0;
    for name in [
        "simple_10by10.xisf",
        "Sample_F32_NoCompression_NoSecurity.xisf",
        "Sample_F32_ZlibCompression_Sha256Security.xisf",
    ] {
        let file = open(name);
        let image = &file.images()[0];
        let resolution = image.resolution().unwrap_or_else(|| panic!("{name} has no <Resolution>"));

        assert!(resolution.horizontal > 0.0, "{name}: non-positive horizontal resolution");
        assert!(resolution.vertical > 0.0, "{name}: non-positive vertical resolution");

        // Whatever unit was written, the per-inch figure must be sane.
        let (x, y) = resolution.per_inch();
        assert!(x > 0.0 && y > 0.0, "{name}: {x} x {y} per inch");
        if resolution.unit == ResolutionUnit::Centimetre {
            assert!(x > resolution.horizontal, "{name}: cm should convert to a larger ppi");
        }
        seen += 1;
    }
    assert_eq!(seen, 3);
}

/// The ICC profile is a real one, so it can be checked against the ICC
/// specification's own header rather than merely being non-empty.
#[test]
fn an_icc_profile_is_read_as_the_icc_specification_defines_it() {
    let file = open("Sample_F32_ZlibCompression_Sha256Security.xisf");
    let image = &file.images()[0];

    let profile = image
        .icc_profile()
        .expect("this file carries an ICC profile")
        .expect("the profile block should read");

    // An ICC profile begins with its own size as a big-endian u32, then a
    // four-character preferred CMM signature; bytes 36..40 are 'acsp'. That
    // last one is the profile file signature and is the strongest check that
    // the bytes were handed over unswapped and unmangled.
    assert!(profile.len() >= 132, "an ICC profile has at least a 128-byte header");
    assert_eq!(&profile[36..40], b"acsp", "the ICC profile signature is wrong");

    let declared = u32::from_be_bytes(profile[0..4].try_into().unwrap()) as usize;
    assert_eq!(
        declared,
        profile.len(),
        "the profile's own big-endian size field disagrees with the block length"
    );
}

/// A thumbnail is an image in its own right, and its data must be the size
/// its geometry implies.
#[test]
fn a_thumbnail_reads_as_an_image() {
    let file = open("Sample_F32_ZlibCompression_Sha256Security.xisf");
    let image = &file.images()[0];

    let thumbnail = image.thumbnail().expect("this file carries a thumbnail");
    assert_eq!(thumbnail.geometry().len(), 2, "a thumbnail is two-dimensional");
    assert!(thumbnail.channels() >= 1);

    let expected = thumbnail.attributes().data_size().expect("size");
    let data = thumbnail.bytes().expect("thumbnail block");
    assert_eq!(data.len() as u64, expected, "the thumbnail block is the wrong size");

    // A thumbnail is a *preview*, not necessarily a smaller file: this one
    // is 400x400 UInt8 against a 50x50 Float32 image, so it is sixteen times
    // the bytes. What holds is that it is stored as 8-bit, which is what
    // makes it cheap to display regardless of the image's sample format.
    assert_eq!(thumbnail.sample_format(), xisf::SampleFormat::UInt8);
    assert_eq!(thumbnail.geometry(), &[400, 400]);
    assert_eq!(thumbnail.channels(), 3);
}

/// A file without these elements says so rather than inventing them.
#[test]
fn absent_elements_are_absent_rather_than_defaulted() {
    let file = open("simple_10by10.xisf");
    let image = &file.images()[0];

    assert!(image.icc_profile().is_none(), "this file has no ICC profile");
    assert!(image.thumbnail().is_none(), "this file has no thumbnail");
    // It does have a resolution, which is why it is the interesting case:
    // absent and present are told apart, not guessed.
    assert!(image.resolution().is_some());
}

/// Generated files carry none of these, so the accessors must cope with that
/// rather than only with PixInsight's output.
#[test]
fn generated_files_have_none_of_them() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/generated/UInt8_Gray_none.xisf");
    let Ok(file) = XisfFile::open(&path) else {
        eprintln!("skipping: {} not generated", path.display());
        return;
    };
    let image = &file.images()[0];
    assert!(image.resolution().is_none());
    assert!(image.icc_profile().is_none());
    assert!(image.thumbnail().is_none());
}

/// The spec lets an element be defined once, at the root, with a `uid`, and
/// pointed at from each image that uses it. That is how one thumbnail or one
/// colour profile serves a whole file without being stored several times.
/// A reader that looks only at an image's own children finds none of it and
/// reports no error, which is the worst failure available: silent data loss.
#[test]
fn shared_elements_reach_the_images_that_reference_them() {
    // The image's own data goes in a <Data> child, because the element also
    // holds the References and text alongside child elements would be
    // ambiguous. This is the shape PixInsight uses for the same reason.
    let body = r#"<xisf version="1.0">
        <Resolution uid="res" horizontal="150" vertical="150" unit="inch"/>
        <Thumbnail uid="thumb" geometry="2:2:1" sampleFormat="UInt8"
                   location="inline:base64">AAECAw==</Thumbnail>
        <Image geometry="2:2:1" sampleFormat="UInt8">
            <Reference ref="res"/>
            <Reference ref="thumb"/>
            <Data location="inline:base64">AAECAw==</Data>
        </Image>
        </xisf>"#;

    let mut bytes = Vec::from(*b"XISF0100");
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(body.as_bytes());

    let file = XisfFile::from_bytes(bytes).expect("read");
    let image = &file.images()[0];

    let resolution = image.resolution().expect("the referenced Resolution was dropped");
    assert_eq!(resolution.horizontal, 150.0);

    let thumbnail = image.thumbnail().expect("the referenced Thumbnail was dropped");
    assert_eq!(thumbnail.bytes().expect("thumbnail block").as_ref(), &[0, 1, 2, 3]);
}
