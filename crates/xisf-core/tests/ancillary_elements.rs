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
