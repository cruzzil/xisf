//! Conformance with Revision 1 of the XISF 1.0 specification.
//!
//! Revision 1 (version 1.01, September 2026) does not change the format
//! version: "Every XISF unit valid under the original document remains valid
//! under this revision." What it does is settle a number of things the
//! original left implicit, and each of those is a place where an
//! implementation written against the original text can be quietly wrong.
//! These tests pin the ones that were.

use xisf_core::block::{Location, TextEncoding};
use xisf_core::{ErrorKind, Reader};

fn unit(body: &str) -> Vec<u8> {
    let xml = format!(
        r#"<xisf version="1.0" xmlns="http://www.pixinsight.com/xisf"><Metadata/>{body}</xisf>"#
    );
    let mut bytes = Vec::from(*b"XISF0100");
    bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(xml.as_bytes());
    bytes
}

fn by_id<'a>(reader: &'a Reader, id: &str) -> &'a xisf_core::header::Element {
    reader
        .header()
        .root
        .descendants()
        .into_iter()
        .find(|e| e.attr("id") == Some(id))
        .expect("no element with that id")
}

/// "The values of empty vector and matrix properties are serialized as inline
/// data blocks with empty character data contents, the only data blocks of
/// zero length." An inline block with no character data is therefore a block
/// of no bytes, not a block that is missing.
#[test]
fn an_empty_inline_block_is_a_block_of_no_bytes() {
    let reader = Reader::from_bytes(unit(
        r#"<Property id="Empty" type="F32Vector" length="0" location="inline:base64"/>"#,
    ))
    .expect("header");
    let block = reader.block(&by_id(&reader, "Empty").data).expect("an empty block is legal");
    assert!(block.is_empty(), "expected zero bytes, got {}", block.len());
}

/// The schema makes `encoding` a required attribute of `<Data>` and allows
/// `hex` as well as `base64`. Assuming base64 turns a legal hex block into a
/// decoding error.
#[test]
fn an_embedded_block_is_decoded_with_the_encoding_data_declares() {
    for (encoding, text) in [("base64", "QUJD"), ("hex", "414243")] {
        let reader = Reader::from_bytes(unit(&format!(
            r#"<Property id="S" type="String" location="embedded"><Data encoding="{encoding}">{text}</Data></Property>"#
        )))
        .expect("header");
        let block =
            reader.block(&by_id(&reader, "S").data).unwrap_or_else(|e| panic!("{encoding}: {e}"));
        assert_eq!(&*block, b"ABC", "{encoding} decoded wrongly");
    }
}

/// `<Data>` carries its own checksum, and it covers the parent's block. A
/// decoder that drops it hands over unverified bytes while the file went to
/// the trouble of saying how to detect tampering.
#[test]
fn a_checksum_on_the_data_element_is_honoured() {
    // A deliberately wrong SHA-1 over "ABC".
    let wrong = "0000000000000000000000000000000000000000";
    let reader = Reader::from_bytes(unit(&format!(
        r#"<Property id="S" type="String" location="embedded"><Data encoding="base64" checksum="sha-1:{wrong}">QUJD</Data></Property>"#
    )))
    .expect("header");

    let data = &by_id(&reader, "S").data;
    assert!(data.checksum.is_some(), "the <Data> checksum was dropped on the floor");
    let err = reader.block(data).expect_err("a wrong checksum was accepted");
    assert_eq!(err.kind(), ErrorKind::ChecksumMismatch);
}

/// A `<Data>` element's compression describes the parent's block too.
///
/// Zstandard rather than zlib, so this runs in every build: Revision 1 makes
/// it a standard codec and this crate compiles it in unconditionally, while
/// zlib remains a feature a consumer may turn off.
#[test]
fn compression_on_the_data_element_is_honoured() {
    let reader = Reader::from_bytes(unit(
        r#"<Property id="S" type="String" location="embedded"><Data encoding="base64" compression="zstd:3">KLUv/SQDGQAAQUJDmO7PTw==</Data></Property>"#,
    ))
    .expect("header");
    let data = &by_id(&reader, "S").data;
    assert!(data.compression.is_some(), "the <Data> compression was dropped");
    assert_eq!(&*reader.block(data).expect("decompress"), b"ABC");
}

/// "Child elements of the XISF root element not defined by this specification
/// should belong to an XML namespace other than the XISF namespace."
///
/// Matching on the local name alone reads such an element as the core element
/// it happens to be named after -- inventing an image the file does not
/// contain, with whatever geometry the extension chose.
#[test]
fn an_extension_element_is_not_mistaken_for_the_core_element_it_is_named_after() {
    let reader = Reader::from_bytes(unit(
        r#"<ext:Image xmlns:ext="http://example.com/notxisf" geometry="9999:9999:3" sampleFormat="Float64"/>"#,
    ))
    .expect("header");
    assert_eq!(reader.header().images().len(), 0, "an extension element was read as an Image");
}

/// The same name in the XISF namespace, however, is the core element, whether
/// it is reached through the default namespace or through a prefix bound to
/// the same URI.
#[test]
fn a_prefixed_core_element_is_still_a_core_element() {
    let xml = r#"<x:xisf version="1.0" xmlns:x="http://www.pixinsight.com/xisf"><x:Metadata/><x:Image geometry="2:2:1" sampleFormat="UInt8" location="attachment:100:4"/></x:xisf>"#;
    let mut bytes = Vec::from(*b"XISF0100");
    bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(xml.as_bytes());
    let reader = Reader::from_bytes(bytes).expect("header");
    assert_eq!(reader.header().images().len(), 1, "a prefixed core Image was not found");
}

/// A header that declares no namespace at all is still read. The
/// specification's own examples are written that way, and so is a good deal
/// of real output; refusing them would reject files every other decoder
/// accepts.
#[test]
fn a_header_with_no_namespace_is_still_read() {
    let xml = r#"<xisf version="1.0"><Metadata/><Image geometry="2:2:1" sampleFormat="UInt8" location="attachment:100:4"/></xisf>"#;
    let mut bytes = Vec::from(*b"XISF0100");
    bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(xml.as_bytes());
    let reader = Reader::from_bytes(bytes).expect("header");
    assert_eq!(reader.header().images().len(), 1);
}

/// An undeclared prefix is a malformed document, not an extension: whoever
/// wrote it meant some namespace, and guessing which would invent content.
#[test]
fn an_undeclared_namespace_prefix_is_refused() {
    let xml = r#"<xisf version="1.0" xmlns="http://www.pixinsight.com/xisf"><nope:Thing/></xisf>"#;
    let mut bytes = Vec::from(*b"XISF0100");
    bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(xml.as_bytes());
    assert_eq!(Reader::from_bytes(bytes).unwrap_err().kind(), ErrorKind::BadHeader);
}

/// The embedded location records which encoding its `<Data>` used.
#[test]
fn the_embedded_location_carries_its_encoding() {
    let reader = Reader::from_bytes(unit(
        r#"<Property id="S" type="String" location="embedded"><Data encoding="hex">414243</Data></Property>"#,
    ))
    .expect("header");
    assert_eq!(
        by_id(&reader, "S").data.location,
        Some(Location::Embedded { encoding: TextEncoding::Hex })
    );
}

/// "Besides NaN, +Inf and -Inf, plain text serializations of floating point
/// values can represent non-finite values with the alternative forms nan,
/// -nan, inf and -inf, which decoders must accept."
///
/// All eight forms happen to be accepted by Rust's own float parser, which is
/// what the property path uses -- so this pins the behaviour against a future
/// change that tightens it.
#[test]
fn every_spelling_of_a_non_finite_float_is_accepted() {
    use xisf_core::property::Property;

    for (text, finite) in [
        ("NaN", false),
        ("nan", false),
        ("-nan", false),
        ("+Inf", false),
        ("-Inf", false),
        ("inf", false),
        ("-inf", false),
        ("1.5", true),
    ] {
        let reader = Reader::from_bytes(unit(&format!(
            r#"<Property id="V" type="Float64" value="{text}"/>"#
        )))
        .expect("header");
        let property = Property::parse(by_id(&reader, "V")).expect("parse");
        let value = property.value().unwrap_or_else(|| panic!("{text:?} was rejected"));
        match value {
            xisf_core::property::ScalarValue::Float(v) => {
                assert_eq!(v.is_finite(), finite, "{text:?} parsed as {v}");
            }
            other => panic!("{text:?} gave {other:?}"),
        }
    }
}

/// "Plain text serializations of Boolean values use the words true and false,
/// and decoders also accept the integers 1 and 0."
#[test]
fn boolean_properties_accept_words_and_integers() {
    use xisf_core::property::{Property, ScalarValue};

    for (text, expected) in [("true", true), ("false", false), ("1", true), ("0", false)] {
        let reader = Reader::from_bytes(unit(&format!(
            r#"<Property id="B" type="Boolean" value="{text}"/>"#
        )))
        .expect("header");
        let property = Property::parse(by_id(&reader, "B")).expect("parse");
        assert_eq!(
            property.value(),
            Some(ScalarValue::Bool(expected)),
            "{text:?} did not read as {expected}"
        );
    }
}

/// "The id attribute of an Image core element must be unique within the XISF
/// unit." Reported rather than refused, since the pixels remain readable.
#[test]
fn duplicate_image_ids_are_reported() {
    let reader = Reader::from_bytes(unit(
        r#"<Image id="Light" geometry="1:1:1" sampleFormat="UInt8" location="attachment:200:1"/><Image id="Light" geometry="1:1:1" sampleFormat="UInt8" location="attachment:201:1"/><Image id="Dark" geometry="1:1:1" sampleFormat="UInt8" location="attachment:202:1"/>"#,
    ))
    .expect("header");
    assert_eq!(reader.header().duplicate_image_ids(), vec!["Light"]);
}

/// The `imageType` values Revision 1's schema fixes are recognised, and one it
/// does not define is kept rather than costing the image.
#[test]
fn image_types_are_recognised_and_unknown_ones_preserved() {
    use xisf_core::image::{Image, ImageType};

    for (text, expected) in [
        ("Light", ImageType::Light),
        ("MasterFlat", ImageType::MasterFlat),
        ("SlopeMap", ImageType::SlopeMap),
        ("WeightMap", ImageType::WeightMap),
        ("BinaryRejectionMapLow", ImageType::BinaryRejectionMapLow),
        ("SomethingNew", ImageType::Other("SomethingNew".into())),
    ] {
        let reader = Reader::from_bytes(unit(&format!(
            r#"<Image imageType="{text}" geometry="1:1:1" sampleFormat="UInt8" location="attachment:200:1"/>"#
        )))
        .expect("header");
        let image = Image::parse(reader.header().images()[0]).expect("parse");
        assert_eq!(image.image_type.as_ref(), Some(&expected), "{text}");
        assert_eq!(image.image_type.unwrap().name(), text, "{text} did not round-trip its name");
    }
}

/// A decimal literal that does not fit its declared type is out of range, not
/// a bit pattern. Reading `Int8 value="200"` as -56 invents a number the file
/// does not contain, and nothing downstream can tell.
///
/// Radix literals are different, and the specification says so: "if the
/// represented value is a two's complement signed 32-bit integer, the value
/// is 80E950AB = -2132193109". That reinterpretation is for radix literals
/// only.
#[test]
fn an_out_of_range_decimal_is_refused_but_a_radix_literal_is_two_s_complement() {
    use xisf_core::property::{Property, ScalarValue};

    let value_of = |kind: &str, text: &str| -> Option<ScalarValue> {
        let reader = Reader::from_bytes(unit(&format!(
            r#"<Property id="V" type="{kind}" value="{text}"/>"#
        )))
        .expect("header");
        Property::parse(by_id(&reader, "V")).expect("parse").value()
    };

    // Out of range, in decimal: refused.
    assert_eq!(value_of("Int8", "200"), None, "200 does not fit an Int8");
    assert_eq!(value_of("Int8", "-200"), None);
    assert_eq!(value_of("UInt8", "256"), None);
    assert_eq!(value_of("Int32", "3000000000"), None);
    assert_eq!(value_of("UInt16", "70000"), None);

    // In range: read as written.
    assert_eq!(value_of("Int8", "127"), Some(ScalarValue::Signed(127)));
    assert_eq!(value_of("Int8", "-128"), Some(ScalarValue::Signed(-128)));
    assert_eq!(value_of("UInt8", "255"), Some(ScalarValue::Unsigned(255)));
    assert_eq!(value_of("Int32", "0"), Some(ScalarValue::Signed(0)));

    // The specification's own example, and the same rule at other widths.
    assert_eq!(value_of("Int32", "0x80E950AB"), Some(ScalarValue::Signed(-2132193109)));
    assert_eq!(value_of("UInt32", "0x80E950AB"), Some(ScalarValue::Unsigned(2162774187)));
    assert_eq!(value_of("Int8", "0xFF"), Some(ScalarValue::Signed(-1)));
    assert_eq!(value_of("Int64", "0xFFFFFFFFFFFFFFFF"), Some(ScalarValue::Signed(-1)));
}

/// Table 8 gives a whole-name alias for every vector type and every matrix
/// type. The matrix ones were missing, which made three schema-valid type
/// names unreadable -- the only case where this decoder refused a valid file.
#[test]
fn every_whole_name_type_alias_is_understood() {
    use xisf_core::property::PropertyType;

    for (alias, canonical) in [
        ("ByteArray", "UI8Vector"),
        ("IVector", "I32Vector"),
        ("UIVector", "UI32Vector"),
        ("Vector", "F64Vector"),
        ("ByteMatrix", "UI8Matrix"),
        ("IMatrix", "I32Matrix"),
        ("UIMatrix", "UI32Matrix"),
        ("Matrix", "F64Matrix"),
    ] {
        let parsed = PropertyType::parse(alias).unwrap_or_else(|e| panic!("{alias}: {e}"));
        assert_eq!(parsed.name(), canonical, "{alias}");
    }
}

/// "Image elements shall be child elements of the unique XISF root element",
/// and an extension element -- which must be in another namespace, and which
/// decoders are required to ignore -- may not contain core elements. Mining
/// one for an `<Image>` invents an image the unit does not contain.
#[test]
fn an_image_buried_in_an_extension_element_is_not_an_image() {
    let reader = Reader::from_bytes(unit(
        r#"<ext:Wrapper xmlns:ext="http://example.com/notxisf"><Image geometry="9999:9999:3" sampleFormat="UInt8" location="attachment:200:1"/></ext:Wrapper>"#,
    ))
    .expect("header");
    assert_eq!(reader.header().images().len(), 0, "a buried Image was reported as real");
}

/// "External XISF data blocks shall not occur in a monolithic XISF file."
///
/// Beyond conformance this is a substitution channel: a monolithic file that
/// quietly draws its pixels from a sibling on disk cannot be detected by its
/// own checksum, which covers whatever the sibling held.
#[test]
fn a_monolithic_file_may_not_reference_an_external_block() {
    for locator in ["path(@header_dir/blocks.xisb):0x1", "url(https://example.com/b.xisb)"] {
        let reader = Reader::from_bytes(unit(&format!(
            r#"<Image id="I" geometry="2:2:1" sampleFormat="UInt8" location="{locator}"/>"#
        )))
        .expect("the header still parses");

        let image = reader.header().images()[0];
        let err = reader.stored_block(&image.data).expect_err("{locator} was followed");
        assert_eq!(err.kind(), ErrorKind::BadAttribute);
        assert!(err.message().contains("monolithic"), "{}", err.message());
    }
}

/// "A unique element identifier must be unique in an XISF unit."
///
/// A duplicate is not cosmetic: a `<Reference>` resolves to whichever element
/// came first in document order, so an image silently acquires another
/// image's colour working space, ICC profile or CFA pattern and is rendered
/// or demosaiced with the wrong one.
#[test]
fn two_core_elements_may_not_share_a_unique_element_identifier() {
    let err = Reader::from_bytes(unit(
        r#"<Resolution uid="X" horizontal="300" vertical="300"/><Resolution uid="X" horizontal="72" vertical="72"/>"#,
    ))
    .expect_err("a duplicate uid was accepted");
    assert_eq!(err.kind(), ErrorKind::BadHeader);
    assert!(err.message().contains("\"X\""), "{}", err.message());

    // Across different kinds of element too: the rule is over all core
    // elements, not within one kind.
    assert!(
        Reader::from_bytes(unit(
            r#"<Resolution uid="Y" horizontal="72" vertical="72"/><Thumbnail uid="Y" geometry="2:2:1" sampleFormat="UInt8" location="attachment:200:4"/>"#
        ))
        .is_err(),
        "a uid shared across element kinds was accepted"
    );

    // And distinct identifiers must still be fine.
    assert!(
        Reader::from_bytes(unit(
            r#"<Resolution uid="A" horizontal="72" vertical="72"/><Resolution uid="B" horizontal="300" vertical="300"/>"#
        ))
        .is_ok()
    );
}

/// "The Unicode code point U+0000 (NULL control character) shall not occur in
/// a string property." It is not a legal XML 1.0 character either, and a NUL
/// reaching a C consumer through the FFI truncates the string there.
#[test]
fn a_null_character_may_not_occur_in_character_data() {
    let err =
        Reader::from_bytes(unit(r#"<Property id="S" type="String">before&#0;after</Property>"#))
            .expect_err("U+0000 was accepted");
    assert_eq!(err.kind(), ErrorKind::BadHeader);

    // A surrogate cannot be encoded in UTF-8 and is refused for that reason.
    assert!(
        Reader::from_bytes(unit(r#"<Property id="S" type="String">&#xD800;</Property>"#)).is_err()
    );
}
