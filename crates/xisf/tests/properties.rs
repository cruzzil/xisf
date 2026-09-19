//! Properties, including the ones whose values live in data blocks.
//!
//! Scalars and strings sit in the header where anything can read them. A
//! vector or matrix does not: its value is binary, in a block, and the header
//! carries only the shape needed to interpret it. Those are the interesting
//! ones, because a reader that ignores the declared shape and trusts the
//! block's size instead will accept a truncated file as a shorter vector and
//! report no error at all.

use xisf::{Shape, XisfFile};

/// Build a monolithic file whose header is `body`, with `trailing` after it.
fn file_with(body: &str, trailing: &[u8]) -> Vec<u8> {
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><xisf version="1.0" \
xmlns="http://www.pixinsight.com/xisf">{body}</xisf>"#
    )
    .replace("\\\n", "");

    let mut bytes = Vec::from(*b"XISF0100");
    bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(xml.as_bytes());
    bytes.extend_from_slice(trailing);
    bytes
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(TABLE[((n >> shift) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[test]
fn scalar_and_string_properties_read_as_text() {
    let bytes = file_with(
        r#"<Metadata>
        <Property id="FocalDistance" type="UInt32" value="2540"/>
        <Property id="Instrument:Camera:Name" type="String">SBIG STF-8300M</Property>
        <Property id="Observation:Time:Start" type="TimePoint" value="2026-09-06T21:00:00Z"/>
        </Metadata>"#,
        &[],
    );
    let file = XisfFile::from_bytes(bytes).expect("read");

    let focal = file.property("FocalDistance").expect("FocalDistance");
    assert_eq!(focal.kind().shape, Shape::Scalar);
    assert_eq!(focal.as_str(), Some("2540"));

    let camera = file.property("Instrument:Camera:Name").expect("camera");
    assert_eq!(camera.kind().shape, Shape::String);
    assert_eq!(camera.as_str(), Some("SBIG STF-8300M"));

    let time = file.property("Observation:Time:Start").expect("time");
    assert_eq!(time.kind().shape, Shape::TimePoint);
    assert_eq!(time.as_str(), Some("2026-09-06T21:00:00Z"));

    assert!(file.property("NoSuchThing").is_none());
}

/// A vector's value is binary and has no textual form, so `as_str` must say
/// so rather than handing back the raw base64 as though it were the value.
#[test]
fn a_vector_property_reads_from_its_data_block() {
    let values: Vec<f64> = vec![1.5, -2.25, 3.0, 1e300];
    let raw: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();

    let bytes = file_with(
        &format!(
            r#"<Metadata><Property id="v" type="F64Vector" length="4"
            location="inline:base64">{}</Property></Metadata>"#,
            base64(&raw)
        ),
        &[],
    );
    let file = XisfFile::from_bytes(bytes).expect("read");
    let property = file.property("v").expect("v");

    assert_eq!(property.kind().shape, Shape::Vector);
    assert_eq!(property.attributes().component_count(), Some(4));
    assert_eq!(property.attributes().data_size(), Some(32));
    assert_eq!(property.as_str(), None, "a vector has no textual value");

    assert_eq!(property.read::<f64>().expect("read"), values);
    assert_eq!(property.bytes().expect("bytes").len(), 32);
}

#[test]
fn a_matrix_property_reads_rows_times_columns() {
    let values: Vec<u16> = (0..12u16).map(|i| i * 1000).collect();
    let raw: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();

    let bytes = file_with(
        &format!(
            r#"<Metadata><Property id="m" type="UI16Matrix" rows="3" columns="4"
            location="inline:base64">{}</Property></Metadata>"#,
            base64(&raw)
        ),
        &[],
    );
    let file = XisfFile::from_bytes(bytes).expect("read");
    let property = file.property("m").expect("m");

    assert_eq!(property.attributes().component_count(), Some(12));
    assert_eq!(property.read::<u16>().expect("read"), values);
}

/// The declared shape is what says how long the value is. A block shorter
/// than the header claims is a truncated file, and accepting it as a shorter
/// vector would lose data with no error.
#[test]
fn a_block_that_disagrees_with_the_declared_shape_is_refused() {
    let raw: Vec<u8> = (0..16u8).collect(); // two f64, not the four claimed
    let bytes = file_with(
        &format!(
            r#"<Metadata><Property id="v" type="F64Vector" length="4"
            location="inline:base64">{}</Property></Metadata>"#,
            base64(&raw)
        ),
        &[],
    );
    let file = XisfFile::from_bytes(bytes).expect("read");
    let err = file.property("v").expect("v").read::<f64>().unwrap_err();
    assert_eq!(err.kind(), xisf::ErrorKind::Truncated);
    assert!(err.message().contains("declares 32 bytes"), "{}", err.message());
}

/// Asking for the wrong element type is an error, not a reinterpretation.
#[test]
fn the_wrong_element_type_is_refused() {
    let raw = vec![0u8; 32];
    let bytes = file_with(
        &format!(
            r#"<Metadata><Property id="v" type="F64Vector" length="4"
            location="inline:base64">{}</Property></Metadata>"#,
            base64(&raw)
        ),
        &[],
    );
    let file = XisfFile::from_bytes(bytes).expect("read");
    let err = file.property("v").expect("v").read::<u16>().unwrap_err();
    assert_eq!(err.kind(), xisf::ErrorKind::InvalidArgument);
    assert!(err.message().contains("Float64"), "{}", err.message());
}

/// A big-endian block is byte-swapped for the host, exactly as image data is.
#[test]
fn a_big_endian_property_block_is_swapped() {
    let values: Vec<u32> = vec![1, 0x0102_0304, u32::MAX];
    let raw: Vec<u8> = values.iter().flat_map(|v| v.to_be_bytes()).collect();

    let bytes = file_with(
        &format!(
            r#"<Metadata><Property id="v" type="UI32Vector" length="3" byteOrder="big"
            location="inline:base64">{}</Property></Metadata>"#,
            base64(&raw)
        ),
        &[],
    );
    let file = XisfFile::from_bytes(bytes).expect("read");
    assert_eq!(file.property("v").expect("v").read::<u32>().expect("read"), values);
}

/// A property with no block at all reports that rather than returning empty.
#[test]
fn a_property_with_no_block_says_so() {
    let bytes =
        file_with(r#"<Metadata><Property id="x" type="UInt32" value="1"/></Metadata>"#, &[]);
    let file = XisfFile::from_bytes(bytes).expect("read");
    let err = file.property("x").expect("x").bytes().unwrap_err();
    assert_eq!(err.kind(), xisf::ErrorKind::NotFound);
}

/// Properties are found wherever they appear, not only under `<Metadata>`.
#[test]
fn properties_are_collected_from_anywhere_in_the_header() {
    let bytes = file_with(
        r#"<Metadata><Property id="unit" type="String">a</Property></Metadata>
        <Image geometry="2:2:1" sampleFormat="UInt8" location="inline:base64">AAAAAA==</Image>
        <Property id="standalone" type="String">c</Property>"#,
        &[],
    );
    let file = XisfFile::from_bytes(bytes).expect("read");
    let ids: Vec<String> = file.properties().iter().map(|p| p.id().to_string()).collect();
    assert!(ids.contains(&"unit".to_string()), "{ids:?}");
    assert!(ids.contains(&"standalone".to_string()), "{ids:?}");
}

/// The format permits integers in binary, octal and hexadecimal, which Rust's
/// own `parse` rejects outright -- so a caller who reached for `.parse()` on
/// `as_str` would fail on values the specification explicitly allows. The
/// fixtures are the spec's own examples, including its worked conversions.
#[test]
fn integer_properties_decode_in_every_radix_the_spec_allows() {
    use xisf::ScalarValue;

    let bytes = file_with(
        r#"<Metadata>
        <Property id="bin" type="UInt32" value="0b10100111100101"/>
        <Property id="oct" type="UInt32" value="0o570261"/>
        <Property id="hexU" type="UInt32" value="0x80E950AB"/>
        <Property id="hexI" type="Int32" value="0x80E950AB"/>
        <Property id="dec" type="Int32" value="-42"/>
        <Property id="upper" type="UInt32" value="0XFF"/>
        <Property id="neg" type="UInt32" value="-1"/>
        <Property id="junk" type="UInt32" value="0xZZ"/>
        </Metadata>"#,
        &[],
    );
    let file = XisfFile::from_bytes(bytes).expect("read");
    let value = |id: &str| file.property(id).unwrap_or_else(|| panic!("{id}")).value();

    // The spec's own worked examples: 0b10100111100101 is 10725, 0o570261 is
    // 192689, and 0x80E950AB is 2162774187 unsigned but -2132193109 signed.
    assert_eq!(value("bin"), Some(ScalarValue::Unsigned(10725)));
    assert_eq!(value("oct"), Some(ScalarValue::Unsigned(192689)));
    assert_eq!(value("hexU"), Some(ScalarValue::Unsigned(2_162_774_187)));
    assert_eq!(value("hexI"), Some(ScalarValue::Signed(-2_132_193_109)));
    assert_eq!(value("dec"), Some(ScalarValue::Signed(-42)));
    assert_eq!(value("upper"), Some(ScalarValue::Unsigned(255)));

    // A negative literal for an unsigned type, and digits that are not
    // digits in the declared base, are refused rather than guessed at.
    assert_eq!(value("neg"), None);
    assert_eq!(value("junk"), None);
}

/// Floats carry the spelled-out non-numeric values, and booleans may be
/// written as words or as 0 and 1 -- both spellings are legal, and a decoder
/// that only understood one would read half the files.
#[test]
fn float_and_boolean_properties_decode_in_every_spelling() {
    use xisf::ScalarValue;

    let bytes = file_with(
        r#"<Metadata>
        <Property id="a" type="Float64" value=".123"/>
        <Property id="b" type="Float32" value="-123.456"/>
        <Property id="nan" type="Float64" value="NaN"/>
        <Property id="pinf" type="Float64" value="+Inf"/>
        <Property id="ninf" type="Float64" value="-Inf"/>
        <Property id="t1" type="Boolean" value="true"/>
        <Property id="t2" type="Boolean" value="1"/>
        <Property id="f1" type="Boolean" value="false"/>
        <Property id="f2" type="Boolean" value="0"/>
        <Property id="z" type="Complex32" value="(0.123,-0.735e-02)"/>
        </Metadata>"#,
        &[],
    );
    let file = XisfFile::from_bytes(bytes).expect("read");
    let value = |id: &str| file.property(id).unwrap_or_else(|| panic!("{id}")).value();

    assert_eq!(value("a"), Some(ScalarValue::Float(0.123)));
    assert_eq!(value("b"), Some(ScalarValue::Float(-123.456)));
    assert!(matches!(value("nan"), Some(ScalarValue::Float(v)) if v.is_nan()));
    assert_eq!(value("pinf"), Some(ScalarValue::Float(f64::INFINITY)));
    assert_eq!(value("ninf"), Some(ScalarValue::Float(f64::NEG_INFINITY)));

    assert_eq!(value("t1"), Some(ScalarValue::Bool(true)));
    assert_eq!(value("t2"), Some(ScalarValue::Bool(true)));
    assert_eq!(value("f1"), Some(ScalarValue::Bool(false)));
    assert_eq!(value("f2"), Some(ScalarValue::Bool(false)));

    // The spec's own complex example.
    assert_eq!(value("z"), Some(ScalarValue::Complex(0.123, -0.00735)));

    // A vector has no scalar value, however it is asked for.
    let bytes = file_with(
        r#"<Metadata><Property id="v" type="F64Vector" length="1"
           location="inline:base64">AAAAAAAAAAA=</Property></Metadata>"#,
        &[],
    );
    let file = XisfFile::from_bytes(bytes).expect("read");
    assert_eq!(file.property("v").expect("v").value(), None);
}

/// "A Property element serializing a String property shall not have a value
/// attribute, and must serialize the property value either directly in its
/// character data contents, or as an XISF data block."
///
/// A file carrying one anyway is malformed, and the malformed half must not
/// win: preferring it let a crafted file make a property read back as
/// something other than its own contents.
#[test]
fn character_data_outranks_a_forbidden_value_attribute() {
    let xml = r#"<xisf version="1.0" xmlns="http://www.pixinsight.com/xisf"><Metadata/><Property id="S" type="String" value="wrong">right</Property></xisf>"#;
    let mut bytes = Vec::from(*b"XISF0100");
    bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(xml.as_bytes());

    let file = XisfFile::from_bytes(bytes).expect("read");
    let property = file.property("S").expect("the property");
    assert_eq!(property.as_str(), Some("right"), "the forbidden value attribute won");
}

/// A baseline encoder must "write properties of all 8-bit, 16-bit, 32-bit and
/// 64-bit scalar types". Until `add_scalar_property` existed the only property
/// this writer could emit was a string, so a caller had no way to record an
/// exposure time as a Float32 without writing the number out as text under a
/// type that says it is text.
///
/// Each value must also read back as itself, which is what says the plain text
/// serialization and the type agree.
#[test]
fn every_baseline_scalar_type_round_trips_through_the_writer() {
    use xisf_core::property::ScalarValue;
    use xisf_core::writer::{BlockOptions, PendingImage, ScalarProperty, Writer};

    let cases: Vec<(&str, ScalarProperty, ScalarValue)> = vec![
        ("B", ScalarProperty::Boolean(true), ScalarValue::Bool(true)),
        ("I8", ScalarProperty::Int8(-128), ScalarValue::Signed(-128)),
        ("U8", ScalarProperty::UInt8(255), ScalarValue::Unsigned(255)),
        ("I16", ScalarProperty::Int16(-32768), ScalarValue::Signed(-32768)),
        ("U16", ScalarProperty::UInt16(65535), ScalarValue::Unsigned(65535)),
        ("I32", ScalarProperty::Int32(-2147483648), ScalarValue::Signed(-2147483648)),
        ("U32", ScalarProperty::UInt32(4294967295), ScalarValue::Unsigned(4294967295)),
        ("I64", ScalarProperty::Int64(i64::MIN), ScalarValue::Signed(i64::MIN as i128)),
        ("U64", ScalarProperty::UInt64(u64::MAX), ScalarValue::Unsigned(u64::MAX as u128)),
        ("F32", ScalarProperty::Float32(0.5), ScalarValue::Float(0.5)),
        ("F64", ScalarProperty::Float64(-1.25e-3), ScalarValue::Float(-1.25e-3)),
    ];

    use xisf_core::image::{ColorSpace, Image, PixelStorage, SampleFormat};

    let mut writer = Writer::new();
    for (id, value, _) in &cases {
        writer.add_scalar_property(*id, *value).expect("add_scalar_property");
    }
    let image = Image {
        dimensions: vec![2, 2],
        channels: 1,
        sample_format: SampleFormat::UInt8,
        color_space: ColorSpace::Gray,
        pixel_storage: PixelStorage::Planar,
        bounds: None,
        id: None,
        uuid: None,
        image_type: None,
        offset: None,
        orientation: None,
    };
    writer
        .add_image(PendingImage::new(image, vec![0; 4], BlockOptions::default()))
        .expect("add_image");

    let file = XisfFile::from_bytes(writer.to_bytes().expect("write")).expect("read back");
    for (id, _, expected) in &cases {
        let property = file.property(id).unwrap_or_else(|| panic!("{id} is missing"));
        assert_eq!(
            property.attributes().value(),
            Some(*expected),
            "{id} did not survive the round trip"
        );
    }
}
