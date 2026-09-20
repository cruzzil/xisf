//! `RGBWorkingSpace`, `DisplayFunction`, `Structure` and `Table`.
//!
//! The fixtures are the specification's own examples, copied verbatim, so
//! these tests check the implementation against the document rather than
//! against a reading of it.

use xisf_core::header::parse;
use xisf_core::image::{DisplayFunction, Gamma, RgbWorkingSpace};
use xisf_core::table::{Structure, Table};

fn header(body: &str) -> xisf_core::header::Header {
    parse(&format!(r#"<xisf version="1.0">{body}</xisf>"#)).expect("header")
}

/// The spec's Adobe RGB example. Three colon-separated triples, one exponent.
#[test]
fn an_rgb_working_space_reads_its_primaries_and_gamma() {
    let h = header(
        r#"<Image geometry="2:2:1" sampleFormat="UInt8">
        <RGBWorkingSpace x="0.648431:0.230154:0.155886"
                         y="0.330856:0.701572:0.066044"
                         Y="0.311114:0.625662:0.063224" gamma="2.2"
                         name="Adobe RGB (1998)"/>
        </Image>"#,
    );
    let element = h.images()[0].children_named("RGBWorkingSpace").next().expect("element");
    let space = RgbWorkingSpace::parse(element).expect("parse");

    assert_eq!(space.gamma, Gamma::Exponent(2.2));
    assert_eq!(space.x, [0.648431, 0.230154, 0.155886]);
    assert_eq!(space.y, [0.330856, 0.701572, 0.066044]);
    assert_eq!(space.luminance, [0.311114, 0.625662, 0.063224]);
    assert_eq!(space.name.as_deref(), Some("Adobe RGB (1998)"));
}

/// `gamma="sRGB"` is a transfer *function*, not an exponent, and the spec
/// says the word is case-insensitive. Parsing it as a number would silently
/// fail, and defaulting it to 2.2 would be visibly wrong in the shadows.
#[test]
fn srgb_gamma_is_a_function_rather_than_an_exponent() {
    for spelling in ["sRGB", "srgb", "SRGB"] {
        let h = header(&format!(
            r#"<RGBWorkingSpace x="1:0:0" y="0:1:0" Y="0.2:0.7:0.1" gamma="{spelling}"/>"#
        ));
        let element = h.root.children_named("RGBWorkingSpace").next().unwrap();
        assert_eq!(RgbWorkingSpace::parse(element).expect(spelling).gamma, Gamma::Srgb);
    }

    // And the default when no element is present is sRGB, per the spec.
    assert_eq!(RgbWorkingSpace::srgb().gamma, Gamma::Srgb);
}

#[test]
fn malformed_colour_parameters_are_refused() {
    for attrs in [
        r#"x="1:0" y="0:1:0" Y="0.2:0.7:0.1" gamma="2.2""#, // too few components
        r#"x="1:0:0:0" y="0:1:0" Y="0.2:0.7:0.1" gamma="2.2""#, // too many
        r#"x="1:0:0" y="0:1:0" Y="0.2:0.7:0.1" gamma="0""#, // gamma must be > 0
        r#"x="1:0:0" y="0:1:0" Y="0.2:0.7:0.1" gamma="wide""#, // neither number nor sRGB
        r#"x="1:0:0" y="0:1:0" gamma="2.2""#,               // Y is mandatory
    ] {
        let h = header(&format!("<RGBWorkingSpace {attrs}/>"));
        let element = h.root.children_named("RGBWorkingSpace").next().unwrap();
        assert!(RgbWorkingSpace::parse(element).is_err(), "accepted: {attrs}");
    }
}

/// The spec's AutoStretch example. Each parameter is four components: red or
/// grey, green, blue, lightness.
#[test]
fn a_display_function_reads_five_four_component_parameters() {
    let h = header(
        r#"<DisplayFunction m="0.000735:0.000735:0.000735:0.5"
                            s="0.003758:0.003758:0.003758:0"
                            h="1:1:1:1" l="0:0:0:0" r="1:1:1:1" name="AutoStretch"/>"#,
    );
    let element = h.root.children_named("DisplayFunction").next().unwrap();
    let df = DisplayFunction::parse(element).expect("parse");

    assert_eq!(df.midtones, [0.000735, 0.000735, 0.000735, 0.5]);
    assert_eq!(df.shadows, [0.003758, 0.003758, 0.003758, 0.0]);
    assert_eq!(df.highlights, [1.0; 4]);
    assert_eq!(df.low_range, [0.0; 4]);
    assert_eq!(df.high_range, [1.0; 4]);
    assert_eq!(df.name.as_deref(), Some("AutoStretch"));

    // This one stretches, so it is emphatically not the identity: a viewer
    // that skipped it would show a black frame for a normal linear image.
    assert!(!df.is_identity());
    assert!(DisplayFunction::identity().is_identity());
}

/// The spec's Messier catalogue example, structure and table both, with the
/// structure standalone and reached through a `<Reference>`.
#[test]
fn a_table_resolves_a_referenced_structure_and_reads_its_rows() {
    let h = header(
        r#"<Structure uid="MessierCatalogStructure">
            <Field id="number" type="UInt8" header="Messier Number"/>
            <Field id="ngc_ic" type="String" header="NGC/IC"/>
            <Field id="commonName" type="String" header="Common Name"/>
            <Field id="distance" type="Float32" header="Distance"
                   format="float:fixed;precision:2;unit:kly"/>
        </Structure>
        <Table id="MessierCatalog" caption="The Messier Catalog" rows="2" columns="4">
            <Reference ref="MessierCatalogStructure"/>
            <Row>
                <Cell value="1"/><Cell value="NGC 1952"/>
                <Cell value="Crab Nebula"/><Cell value="6.5"/>
            </Row>
            <Row>
                <Cell value="2"/><Cell value="NGC 7089"/>
                <Cell value=""/><Cell value="33"/>
            </Row>
        </Table>"#,
    );
    let element = h.root.children_named("Table").next().expect("<Table>");
    let table = Table::parse(element, &h).expect("parse");

    assert_eq!(table.id, "MessierCatalog");
    assert_eq!(table.caption.as_deref(), Some("The Messier Catalog"));
    assert_eq!(table.structure.fields.len(), 4);
    assert_eq!(table.structure.fields[0].header.as_deref(), Some("Messier Number"));
    assert_eq!(
        table.structure.fields[3].format.as_deref(),
        Some("float:fixed;precision:2;unit:kly")
    );

    assert_eq!(table.rows.len(), 2);
    assert_eq!(table.cell(0, "commonName").unwrap().as_str(), Some("Crab Nebula"));
    assert_eq!(table.cell(1, "ngc_ic").unwrap().as_str(), Some("NGC 7089"));
    // An empty common name is empty, not missing: M2 has no popular name.
    assert_eq!(table.cell(1, "commonName").unwrap().as_str(), Some(""));
    assert!(table.cell(0, "noSuchField").is_none());
}

/// A structure may also sit inside its table, which is the simpler form.
#[test]
fn a_table_may_carry_its_own_structure() {
    let h = header(
        r#"<Table id="t">
            <Structure><Field id="a" type="Int32"/></Structure>
            <Row><Cell value="7"/></Row>
        </Table>"#,
    );
    let table = Table::parse(h.root.children_named("Table").next().unwrap(), &h).expect("parse");
    assert_eq!(table.cell(0, "a").unwrap().as_str(), Some("7"));
}

/// Which field a cell belongs to is decided by position alone. A row with the
/// wrong number of cells has no reading at all, so it must not be guessed at:
/// a short row would otherwise shift every value after the gap into the wrong
/// column and report nothing.
#[test]
fn rows_that_do_not_match_the_structure_are_refused() {
    let cases = [
        // A row one cell short.
        r#"<Table id="t"><Structure><Field id="a" type="Int32"/><Field id="b" type="Int32"/>
           </Structure><Row><Cell value="1"/></Row></Table>"#,
        // A declared row count that disagrees with the rows present.
        r#"<Table id="t" rows="9"><Structure><Field id="a" type="Int32"/></Structure>
           <Row><Cell value="1"/></Row></Table>"#,
        // A declared column count that disagrees with the structure.
        r#"<Table id="t" columns="3"><Structure><Field id="a" type="Int32"/></Structure>
           </Table>"#,
        // No structure at all, and no reference to one.
        r#"<Table id="t"><Row><Cell value="1"/></Row></Table>"#,
        // A reference that names nothing.
        r#"<Table id="t"><Reference ref="nowhere"/></Table>"#,
        // A structure with no fields.
        r#"<Table id="t"><Structure/></Table>"#,
    ];
    for case in cases {
        let h = header(case);
        let element = h.root.children_named("Table").next().unwrap();
        assert!(Table::parse(element, &h).is_err(), "accepted: {case}");
    }
}

/// A standalone structure is usable on its own, without any table.
#[test]
fn a_standalone_structure_keeps_its_uid() {
    let h = header(r#"<Structure uid="s"><Field id="a" type="String"/></Structure>"#);
    let s = Structure::parse(h.root.children_named("Structure").next().unwrap()).expect("parse");
    assert_eq!(s.uid.as_deref(), Some("s"));
    assert_eq!(s.fields[0].kind.shape, xisf_core::property::Shape::String);
}

/// `offset` is a pedestal added to every sample. Dropping it gives an image
/// the wrong zero point, which a calibration pipeline then propagates into
/// every frame it touches -- so it is read, and it is validated.
#[test]
fn an_image_offset_is_read_and_bounded() {
    let h = header(r#"<Image geometry="2:2:1" sampleFormat="UInt16" offset="512.5"/>"#);
    let image = xisf_core::image::Image::parse(h.images()[0]).expect("parse");
    assert_eq!(image.offset, Some(512.5));

    // Absent means zero by the spec's default, but absent and zero are told
    // apart so a writer can reproduce a file that said nothing.
    let h = header(r#"<Image geometry="2:2:1" sampleFormat="UInt16"/>"#);
    assert_eq!(xisf_core::image::Image::parse(h.images()[0]).expect("parse").offset, None);

    // A pedestal is added, never subtracted, so a negative one is meaningless.
    for bad in ["-1", "nonsense", "NaN"] {
        let h =
            header(&format!(r#"<Image geometry="2:2:1" sampleFormat="UInt16" offset="{bad}"/>"#));
        assert!(xisf_core::image::Image::parse(h.images()[0]).is_err(), "accepted offset={bad}");
    }
}

/// Every orientation the spec lists, round-tripped through its own spelling.
#[test]
fn every_orientation_the_spec_defines_round_trips() {
    use xisf_core::image::Orientation;

    for (text, rotation, flip) in [
        ("0", 0, false),
        ("flip", 0, true),
        ("90", 90, false),
        ("90;flip", 90, true),
        ("-90", -90, false),
        ("-90;flip", -90, true),
        ("180", 180, false),
        ("180;flip", 180, true),
    ] {
        let parsed = Orientation::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!((parsed.rotation, parsed.flip_horizontal), (rotation, flip), "{text}");
        assert_eq!(parsed.to_attribute(), text, "{text} did not survive being written back");
        assert_eq!(parsed.is_identity(), text == "0", "{text}");
    }

    for bad in ["45", "flop", "90;flop", "", "90;flip;flip"] {
        assert!(Orientation::parse(bad).is_err(), "accepted orientation={bad:?}");
    }
}

/// A compressed block may be stored as several independent streams laid end
/// to end, described by a `subblocks` attribute. Codecs have input limits --
/// zlib takes at most 4GiB at once -- and splitting also lets an encoder
/// compress the pieces in parallel, so files written this way exist;
/// libXISF, the reference implementation, both reads and writes them.
///
/// A decoder that ignored the attribute would hand the whole buffer to one
/// codec call and recover only the first piece.
#[cfg(feature = "zlib")]
#[test]
fn a_block_stored_as_several_compression_subblocks_decodes_whole() {
    use xisf_core::block::{Codec, Compression};

    // Three distinguishable pieces, each compressed on its own.
    let pieces: Vec<Vec<u8>> =
        vec![vec![0xAA; 300], (0..=255u8).cycle().take(500).collect(), vec![0x11; 200]];

    let compress = |data: &[u8]| -> Vec<u8> {
        use std::io::Write;
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).expect("write");
        encoder.finish().expect("finish")
    };

    let mut stored = Vec::new();
    let mut subblocks = Vec::new();
    let mut expected = Vec::new();
    for piece in &pieces {
        let compressed = compress(piece);
        subblocks.push((compressed.len() as u64, piece.len() as u64));
        stored.extend_from_slice(&compressed);
        expected.extend_from_slice(piece);
    }

    let compression = Compression {
        codec: Codec::Zlib,
        uncompressed_size: expected.len() as u64,
        shuffle_item_size: None,
        subblocks: subblocks.clone(),
    };
    assert_eq!(xisf_core::codec::decode(&stored, &compression).expect("decode"), expected);

    // Without the attribute the same bytes decode to the first piece only,
    // which is exactly the silent truncation the attribute exists to prevent
    // -- and the declared size catches it rather than returning a short image.
    let ignored = Compression { subblocks: Vec::new(), ..compression.clone() };
    let err = xisf_core::codec::decode(&stored, &ignored).expect_err("decoded without subblocks");
    assert_eq!(err.kind(), xisf_core::ErrorKind::Compression);

    // Lengths that do not account for the bytes present are refused before
    // anything is decoded.
    for bad in [
        vec![(stored.len() as u64 + 1, expected.len() as u64)],
        vec![(stored.len() as u64, expected.len() as u64 + 1)],
        vec![(u64::MAX, 1), (u64::MAX, 1)],
    ] {
        let broken = Compression { subblocks: bad, ..compression.clone() };
        assert!(xisf_core::codec::decode(&stored, &broken).is_err(), "a bad subblock list passed");
    }
}

/// The attribute is parsed off the element, and describes the compression, so
/// it is meaningless on a block that declares none.
#[test]
fn the_subblocks_attribute_is_read_and_requires_compression() {
    let h = header(
        r#"<Image geometry="2:2:1" sampleFormat="UInt8"
           compression="zlib:400" subblocks="10,200:12,200"
           location="attachment:1024:22"/>"#,
    );
    let compression = h.images()[0].data.compression.clone().expect("compression");
    assert_eq!(compression.subblocks, vec![(10, 200), (12, 200)]);

    // Attribute order must not matter: XML does not fix it.
    let h = header(
        r#"<Image geometry="2:2:1" sampleFormat="UInt8"
           subblocks="10,200:12,200" compression="zlib:400"
           location="attachment:1024:22"/>"#,
    );
    assert_eq!(
        h.images()[0].data.compression.clone().expect("compression").subblocks,
        vec![(10, 200), (12, 200)]
    );

    for bad in [
        r#"subblocks="10,200" location="inline:base64""#, // no compression at all
        r#"compression="zlib:400" subblocks="10""#,       // not a pair
        r#"compression="zlib:400" subblocks="a,b""#,      // not numbers
    ] {
        let xml = format!(
            r#"<xisf version="1.0"><Image geometry="2:2:1" sampleFormat="UInt8" {bad}/></xisf>"#
        );
        assert!(parse(&xml).is_err(), "accepted: {bad}");
    }
}

/// "Field property identifiers must be unique, i.e., two or more fields
/// pertaining to the same table cannot have the same property identifier."
///
/// A column is found by the position of the first field with a given id, so a
/// duplicate makes the second column permanently unreachable and hands its
/// values to whoever asks for the first: right ascensions read as
/// declinations, with the table looking complete.
#[test]
fn two_fields_of_a_structure_may_not_share_an_identifier() {
    let xml = r#"<xisf version="1.0" xmlns="http://www.pixinsight.com/xisf"><Metadata/>
        <Table id="T">
          <Structure>
            <Field id="ra" type="Float64"/>
            <Field id="dec" type="Float64"/>
            <Field id="ra" type="Float64"/>
          </Structure>
          <Row><Cell value="1"/><Cell value="2"/><Cell value="3"/></Row>
        </Table></xisf>"#;
    let mut bytes = Vec::from(*b"XISF0100");
    bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(xml.as_bytes());

    let reader = xisf_core::Reader::from_bytes(bytes).expect("the header parses");
    let element = reader
        .header()
        .root
        .descendants()
        .into_iter()
        .find(|e| e.is("Table"))
        .expect("the table element");

    let err = xisf_core::table::Table::parse(element, reader.header())
        .expect_err("a duplicate field identifier was accepted");
    assert_eq!(err.kind(), xisf_core::ErrorKind::BadHeader);
    assert!(err.message().contains("\"ra\""), "{}", err.message());
}

/// "the byte shuffling transform shall be applied to the entire data block
/// before its division into subblocks, and the reverse transform after all
/// subblocks have been decompressed and concatenated."
///
/// This is the clause an implementation written against the original text is
/// most likely to get wrong, because unshuffling each subblock as it is
/// decoded is the natural way to write the loop and is indistinguishable from
/// the correct version whenever the split happens to fall on an item boundary.
/// The subblock lengths below are deliberately *not* multiples of the item
/// size, so the two orderings disagree.
///
/// Nothing else in the suite covers both at once: the subblock test uses no
/// shuffling, and the shuffling tests use no subblocks.
#[cfg(feature = "zlib")]
#[test]
fn shuffling_spans_the_whole_block_rather_than_each_subblock() {
    use std::io::Write;
    use xisf_core::block::{Codec, Compression};

    // Distinguishable four-byte items, so a byte landing in the wrong place is
    // visible rather than merely different.
    const ITEM: usize = 4;
    let plain: Vec<u8> = (0..250u32).flat_map(|i| (i * 0x0103_0107).to_le_bytes()).collect();
    assert_eq!(plain.len() % ITEM, 0);

    // Shuffle the whole block first, as an encoder must.
    let shuffled = xisf_core::codec::shuffle(&plain, ITEM);

    // Then cut it at offsets that are not item boundaries.
    let cuts = [301usize, 499];
    assert!(cuts.iter().all(|c| c % ITEM != 0), "the cuts must straddle items");
    let pieces = [&shuffled[..cuts[0]], &shuffled[cuts[0]..cuts[1]], &shuffled[cuts[1]..]];

    let mut stored = Vec::new();
    let mut subblocks = Vec::new();
    for piece in pieces {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(piece).unwrap();
        let compressed = encoder.finish().unwrap();
        subblocks.push((compressed.len() as u64, piece.len() as u64));
        stored.extend_from_slice(&compressed);
    }

    let compression = Compression {
        codec: Codec::Zlib,
        uncompressed_size: plain.len() as u64,
        shuffle_item_size: Some(ITEM as u64),
        subblocks,
    };
    assert_eq!(
        xisf_core::codec::decode(&stored, &compression).expect("decode"),
        plain,
        "the block did not survive shuffling across a subblock split"
    );

    // And the distinction is real: unshuffling each piece separately gives
    // something else, so the test above is not passing by coincidence.
    let per_piece: Vec<u8> =
        pieces.iter().flat_map(|p| xisf_core::codec::unshuffle(p, ITEM)).collect();
    assert_ne!(per_piece, plain, "per-subblock unshuffling happened to agree");
}

/// "For compressed data blocks, the byteOrder attribute applies to the
/// uncompressed data."
///
/// Three stages have to happen in one order -- decompress, unshuffle, then
/// swap -- and no test exercised them as a chain: the two big-endian tests use
/// uncompressed inline blocks, and no corpus file is big-endian. Swapping the
/// compressed bytes instead would return every sample byte-reversed, which for
/// a Float32 turns 1.0 into 4.6e-41.
#[cfg(feature = "zlib")]
#[test]
fn a_big_endian_block_is_swapped_after_it_is_decompressed_and_unshuffled() {
    use std::io::Write;
    use xisf_core::block::{ByteOrder, Codec, Compression};

    let values: Vec<u32> = (0..64u32).map(|i| i.wrapping_mul(0x0100_0193) | 1).collect();
    let big_endian: Vec<u8> = values.iter().flat_map(|v| v.to_be_bytes()).collect();

    let shuffled = xisf_core::codec::shuffle(&big_endian, 4);
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&shuffled).unwrap();
    let stored = encoder.finish().unwrap();

    let plain = xisf_core::codec::decode(
        &stored,
        &Compression {
            codec: Codec::Zlib,
            uncompressed_size: big_endian.len() as u64,
            shuffle_item_size: Some(4),
            subblocks: Vec::new(),
        },
    )
    .expect("decode");

    // What comes out of the codec is the stored byte order, untouched.
    assert_eq!(plain, big_endian, "decoding must not reorder bytes");

    // The swap is the caller's step, and it applies to these bytes -- the
    // uncompressed ones -- not to the compressed stream.
    assert_eq!(ByteOrder::Big.is_native(), cfg!(target_endian = "big"));
    let decoded: Vec<u32> =
        plain.as_chunks::<4>().0.iter().map(|c| u32::from_be_bytes(*c)).collect();
    assert_eq!(decoded, values, "the samples did not survive the three stages");
}
