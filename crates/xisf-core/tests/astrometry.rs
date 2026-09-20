//! The AstrometricSolution namespace, and the layering rules that decide what
//! a decoder may use when it meets something it does not recognize.

use xisf_core::Reader;
use xisf_core::astrometry::{
    self, Availability, BasisFunction, CelestialReferenceSystem, ProjectionClass, ProjectionSystem,
    TermKind, Version,
};

/// Build a unit whose single image carries the given astrometric properties.
///
/// The vector and matrix values are inline base64 of little-endian f64s, which
/// is what the real thing uses for small arrays.
fn with_solution(
    properties: &[(String, String)],
    vectors: &[(String, String, Vec<f64>)],
) -> Vec<u8> {
    let mut body = String::new();
    for (id, value) in properties {
        body.push_str(&format!(
            r#"<Property id="AstrometricSolution:{id}" type="String">{value}</Property>"#
        ));
    }
    for (id, kind, values) in vectors {
        let mut raw = Vec::new();
        for v in values {
            raw.extend_from_slice(&v.to_le_bytes());
        }
        let shape = if kind == "F64Matrix" {
            let columns = 2.max(values.len() / 2);
            let columns = if values.len() == 4 {
                2
            } else if values.len() == 9 {
                3
            } else {
                columns
            };
            format!(r#"rows="{}" columns="{columns}""#, values.len() / columns)
        } else {
            format!(r#"length="{}""#, values.len())
        };
        body.push_str(&format!(
            r#"<Property id="AstrometricSolution:{id}" type="{kind}" {shape} location="inline:base64">{}</Property>"#,
            base64(&raw)
        ));
    }

    let xml = format!(
        r#"<xisf version="1.0" xmlns="http://www.pixinsight.com/xisf"><Metadata/><Image geometry="16:16:1" sampleFormat="UInt8" location="attachment:4096:256">{body}</Image></xisf>"#
    );
    let mut bytes = Vec::from(*b"XISF0100");
    bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(xml.as_bytes());
    bytes
}

fn base64(raw: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in raw.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A solution's scalar properties: identifier and value.
type Scalars = Vec<(String, String)>;
/// A solution's array properties: identifier, XISF type, and values.
type Arrays = Vec<(String, String, Vec<f64>)>;

/// The minimum a valid solution needs: layer 1 and nothing else.
fn layer_one() -> (Scalars, Arrays) {
    (
        vec![("Version".into(), "1.0".into()), ("ProjectionSystem".into(), "Gnomonic".into())],
        vec![
            ("ReferenceCelestialCoordinates".into(), "F64Vector".into(), vec![10.684, 41.269]),
            ("ReferenceImageCoordinates".into(), "F64Vector".into(), vec![8.0, 8.0]),
            (
                "LinearTransformationMatrix".into(),
                "F64Matrix".into(),
                vec![-0.0005, 0.0, 0.0, 0.0005],
            ),
        ],
    )
}

fn read(
    properties: &[(String, String)],
    vectors: &[(String, String, Vec<f64>)],
) -> xisf_core::Result<Option<astrometry::Solution>> {
    let bytes = with_solution(properties, vectors);
    let reader = Reader::from_bytes(bytes).expect("header");
    let image = reader.header().images()[0];
    astrometry::read(&reader, image)
}

/// As [`read`], plus the `Local:*:NodeOffsets` property, which is an
/// `I32Vector` and so cannot go through the `f64` array helper.
fn read_with_offsets(
    properties: &[(String, String)],
    vectors: &[(String, String, Vec<f64>)],
    offsets: &[i32],
) -> Option<astrometry::Solution> {
    let mut body = String::new();
    for (id, value) in properties {
        body.push_str(&format!(
            r#"<Property id="AstrometricSolution:{id}" type="String">{value}</Property>"#
        ));
    }
    for (id, kind, values) in vectors {
        let mut raw = Vec::new();
        for v in values {
            raw.extend_from_slice(&v.to_le_bytes());
        }
        let shape = if *kind == "F64Matrix" {
            // The fixtures use only 2- and 3-column matrices, and the two are
            // told apart by whether the count divides by three without also
            // dividing by two -- enough for these shapes, and not a general
            // rule.
            let columns = if values.len() == 9 || (values.len() % 3 == 0 && values.len() % 2 != 0) {
                3
            } else {
                2
            };
            format!(r#"rows="{}" columns="{columns}""#, values.len() / columns)
        } else {
            format!(r#"length="{}""#, values.len())
        };
        body.push_str(&format!(
            r#"<Property id="AstrometricSolution:{id}" type="{kind}" {shape} location="inline:base64">{}</Property>"#,
            base64(&raw)
        ));
    }
    for which in ["ImageToProjection", "ProjectionToImage"] {
        let mut raw = Vec::new();
        for v in offsets {
            raw.extend_from_slice(&v.to_le_bytes());
        }
        body.push_str(&format!(
            r#"<Property id="AstrometricSolution:DistortionModel:{which}:Local:X:NodeOffsets" type="I32Vector" length="{}" location="inline:base64">{}</Property>"#,
            offsets.len(),
            base64(&raw)
        ));
    }

    let xml = format!(
        r#"<xisf version="1.0" xmlns="http://www.pixinsight.com/xisf"><Metadata/><Image geometry="16:16:1" sampleFormat="UInt8" location="attachment:8192:256">{body}</Image></xisf>"#
    );
    let mut bytes = Vec::from(*b"XISF0100");
    bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(xml.as_bytes());

    let reader = Reader::from_bytes(bytes).expect("header");
    let image = reader.header().images()[0];
    astrometry::read(&reader, image).expect("read")
}

#[test]
fn an_image_without_a_solution_has_none() {
    let solution = read(&[], &[]).expect("read");
    assert!(solution.is_none());
}

#[test]
fn the_first_layer_is_enough_for_a_valid_solution() {
    let (props, vecs) = layer_one();
    let solution = read(&props, &vecs).expect("read").expect("a solution");

    assert_eq!(solution.version, Version { major: 1, minor: 0 });
    assert_eq!(solution.projection.system, ProjectionSystem::Gnomonic);
    assert_eq!(solution.projection.reference_celestial, [10.684, 41.269]);
    assert_eq!(solution.projection.reference_image, [8.0, 8.0]);
    assert_eq!(solution.projection.linear_transformation, [[-0.0005, 0.0], [0.0, 0.0005]]);
    // Absent, so the projection's own default applies.
    assert!(solution.projection.reference_native.is_none());
    assert_eq!(solution.projection.system.default_reference_native(), [0.0, 90.0]);
    // Absent, so ICRS.
    assert_eq!(solution.projection.celestial_reference_system, CelestialReferenceSystem::Icrs);
    assert_eq!(solution.availability, Availability::Complete);
    assert_eq!(solution.usable_layer(), 4);
}

/// "Decoders shall treat an unrecognized identifier as making the whole
/// solution unavailable" -- the geometry rests on the projection, so there is
/// no lower layer to fall back to.
#[test]
fn an_unknown_projection_system_makes_the_whole_solution_unavailable() {
    let (mut props, vecs) = layer_one();
    props[1] = ("ProjectionSystem".into(), "Fisheye".into());
    let err = read(&props, &vecs).expect_err("an unknown projection was accepted");
    assert_eq!(err.kind(), xisf_core::ErrorKind::Unsupported);
    assert!(err.message().contains("whole solution unavailable"), "{}", err.message());
}

/// "A decoder that does not support the major revision of a solution shall not
/// interpret any part of the solution, not even its first layer."
#[test]
fn an_unsupported_major_revision_is_not_interpreted_at_all() {
    let (mut props, vecs) = layer_one();
    props[0] = ("Version".into(), "2.0".into());
    let err = read(&props, &vecs).expect_err("a future major revision was interpreted");
    assert_eq!(err.kind(), xisf_core::ErrorKind::Unsupported);

    // A newer *minor* revision is readable, because additions within a major
    // revision cannot change the meaning of what is already there.
    assert!(Version { major: 1, minor: 7 }.is_supported());
    assert!(!Version { major: 2, minor: 0 }.is_supported());
}

/// An unrecognized basis function costs the third layer and leaves the two
/// below it usable -- "each layer is the fallback for the next one".
#[test]
fn an_unknown_basis_function_costs_only_the_distortion_layer() {
    let (mut props, mut vecs) = layer_one();
    props.push(("DistortionModel:ImageToProjection:BasisFunction".into(), "MysteryKernel".into()));
    props.push(("DistortionModel:ImageToProjection:Order".into(), "2".into()));
    props.push(("DistortionModel:ImageToProjection:Terms".into(), "Global".into()));
    props
        .push(("DistortionModel:ProjectionToImage:BasisFunction".into(), "ThinPlateSpline".into()));
    props.push(("DistortionModel:ProjectionToImage:Order".into(), "2".into()));
    props.push(("DistortionModel:ProjectionToImage:Terms".into(), "Global".into()));
    vecs.push((
        "ProjectiveTransformation:ImageToProjection".into(),
        "F64Matrix".into(),
        vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    ));
    vecs.push((
        "ProjectiveTransformation:ProjectionToImage".into(),
        "F64Matrix".into(),
        vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    ));

    let solution = read(&props, &vecs).expect("read").expect("a solution");
    assert!(solution.distortion.is_none(), "an unusable distortion model was kept");
    assert!(solution.projective.is_some(), "the second layer should survive");
    assert_eq!(solution.usable_layer(), 2);
    match &solution.availability {
        Availability::UpTo { layer: 2, because } => {
            assert!(because.contains("MysteryKernel"), "{because}");
        }
        other => panic!("expected layers up to 2, got {other:?}"),
    }
}

/// "Either both properties are present, or neither of them."
#[test]
fn half_a_projective_transformation_is_refused() {
    let (props, mut vecs) = layer_one();
    vecs.push((
        "ProjectiveTransformation:ImageToProjection".into(),
        "F64Matrix".into(),
        vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    ));
    let err = read(&props, &vecs).expect_err("one direction alone was accepted");
    assert!(err.message().contains("both are required together"), "{}", err.message());
}

/// An unrecognized reference system "does not make any layer unavailable,
/// since the geometric transformation remains fully defined".
#[test]
fn an_unknown_reference_system_costs_nothing() {
    let (mut props, vecs) = layer_one();
    props.push(("CelestialReferenceSystem".into(), "FK5".into()));
    let solution = read(&props, &vecs).expect("read").expect("a solution");
    assert_eq!(
        solution.projection.celestial_reference_system,
        CelestialReferenceSystem::Other("FK5".into())
    );
    assert_eq!(solution.availability, Availability::Complete);
}

#[test]
fn the_projection_vocabulary_is_complete_and_classified() {
    for (name, class) in [
        ("Gnomonic", ProjectionClass::Zenithal),
        ("Stereographic", ProjectionClass::Zenithal),
        ("ZenithalEqualArea", ProjectionClass::Zenithal),
        ("Orthographic", ProjectionClass::Zenithal),
        ("PlateCarree", ProjectionClass::Cylindrical),
        ("Mercator", ProjectionClass::Cylindrical),
        ("HammerAitoff", ProjectionClass::PseudoCylindrical),
    ] {
        let system = ProjectionSystem::parse(name).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(system.name(), name);
        assert_eq!(system.class(), class, "{name}");
        // Zenithal projections default to (0, 90), everything else to (0, 0).
        let expected = if class == ProjectionClass::Zenithal { [0.0, 90.0] } else { [0.0, 0.0] };
        assert_eq!(system.default_reference_native(), expected, "{name}");
    }
    assert!(ProjectionSystem::parse("TAN").is_none(), "WCS codes are not XISF identifiers");
}

#[test]
fn basis_functions_know_their_own_requirements() {
    // "The polynomial part is required" for these two, and neither takes a
    // shape parameter. Their minimum orders differ: Table 17 gives m >= 2 for
    // a thin plate spline and m >= 3 for VariableOrder, because "a kernel of
    // this family with order 2 is a thin plate spline and shall use the
    // ThinPlateSpline identifier" -- the two identifiers partition the family
    // rather than overlapping.
    for f in [BasisFunction::ThinPlateSpline, BasisFunction::VariableOrder] {
        assert!(f.requires_polynomial(), "{}", f.name());
        assert!(!f.has_shape_parameter(), "{}", f.name());
    }
    assert_eq!(BasisFunction::ThinPlateSpline.minimum_order(), 2);
    assert_eq!(BasisFunction::VariableOrder.minimum_order(), 3);
    // These take a shape parameter and the polynomial part is optional.
    for f in [
        BasisFunction::Gaussian,
        BasisFunction::Multiquadric,
        BasisFunction::InverseMultiquadric,
        BasisFunction::InverseQuadratic,
    ] {
        assert!(f.has_shape_parameter(), "{}", f.name());
        assert!(!f.requires_polynomial(), "{}", f.name());
    }
    assert!(BasisFunction::parse("Wendland").is_none());
}

/// "Decoders shall treat an unrecognized identifier in this list as making the
/// third layer unavailable, so that a model with terms the decoder cannot
/// evaluate is never evaluated partially."
#[test]
fn term_lists_are_all_or_nothing() {
    assert_eq!(TermKind::parse_list("Global").unwrap(), vec![TermKind::Global]);
    assert_eq!(
        TermKind::parse_list("Local\nFallback").unwrap(),
        vec![TermKind::Local, TermKind::Fallback]
    );
    // One unknown entry poisons the list rather than being skipped.
    assert!(TermKind::parse_list("Local\nQuadtree").is_err());
    // "A Fallback term ... only together with Local terms."
    assert!(TermKind::parse_list("Fallback").is_err());
    assert!(TermKind::parse_list("").is_err());
}

#[test]
fn versions_parse_and_order() {
    assert_eq!(Version::parse("1.0").unwrap(), Version { major: 1, minor: 0 });
    assert_eq!(Version::parse(" 2.13 ").unwrap(), Version { major: 2, minor: 13 });
    assert!(Version::parse("1").is_err());
    assert!(Version::parse("1.x").is_err());
    assert_eq!(Version::SUPPORTED.to_string(), "1.0");
}

/// Provenance is optional and constrains nothing, but must be read.
#[test]
fn provenance_is_read_when_present() {
    let (mut props, vecs) = layer_one();
    props.push(("Catalog".into(), "Gaia DR3".into()));
    props.push(("CreatorApplication".into(), "SomeSolver 1.2".into()));
    props.push(("CreatorOS".into(), "Linux".into()));
    let solution = read(&props, &vecs).expect("read").expect("a solution");
    assert_eq!(solution.provenance.catalog.as_deref(), Some("Gaia DR3"));
    assert_eq!(solution.provenance.creator_application.as_deref(), Some("SomeSolver 1.2"));
    assert_eq!(solution.provenance.creator_os.as_deref(), Some("Linux"));
    assert_eq!(solution.provenance.creator_module, None);
}

/// The whole pipeline, layer 1 only: image -> plane -> native -> celestial and
/// back. A solution with nothing but the first layer is valid and complete, so
/// this is the case every conforming decoder must get right.
#[test]
fn a_first_layer_solution_evaluates_and_round_trips() {
    let (props, vecs) = layer_one();
    let solution = read(&props, &vecs).expect("read").expect("a solution");

    // The reference image point must land on the reference celestial point:
    // that is what "the image coordinates that correspond to the origin of the
    // projection plane" means, and the origin deprojects to the native pole.
    let [ra, dec] = solution
        .image_to_celestial(solution.projection.reference_image)
        .expect("the reference point is always in domain");
    assert!((ra - 10.684).abs() < 1e-9, "right ascension {ra}");
    assert!((dec - 41.269).abs() < 1e-9, "declination {dec}");

    // And every other pixel must survive the trip out and back, within the
    // tolerance the specification sets: "two conforming implementations
    // evaluating the same solution at the same point shall agree to within
    // 10^-6 pixels in image coordinates, and no implementation should claim
    // exactness beyond this tolerance". Asserting anything tighter would be
    // claiming exactly that, and would turn a harmless change in summation
    // order into a failure.
    const TOLERANCE: f64 = 1e-6;
    let mut worst = 0.0f64;
    for &x in &[0.0, 3.5, 8.0, 12.25, 15.5] {
        for &y in &[0.0, 4.0, 8.0, 11.75, 15.5] {
            let celestial = solution.image_to_celestial([x, y]).expect("in domain");
            let [x2, y2] = solution.celestial_to_image(celestial).expect("and back");
            worst = worst.max((x - x2).abs()).max((y - y2).abs());
            assert!(
                (x - x2).abs() < TOLERANCE && (y - y2).abs() < TOLERANCE,
                "({x}, {y}) came back as ({x2}, {y2})"
            );
        }
    }
    // Comfortably inside it rather than scraping past, which is what says the
    // formulas are right and not merely within a generous bound.
    assert!(worst < TOLERANCE / 10.0, "the worst round trip was {worst} pixels");
}

/// The plate scale the linear transformation declares must be the plate scale
/// the evaluation produces. Half a degree per pixel in the test fixture means
/// one pixel of separation is half a degree on the sky, near the reference
/// point where the projection is locally flat.
#[test]
fn the_linear_transformation_sets_the_plate_scale() {
    let (props, vecs) = layer_one();
    let solution = read(&props, &vecs).expect("read").expect("a solution");
    let reference = solution.projection.reference_image;

    let [ra0, dec0] = solution.image_to_celestial(reference).expect("in domain");
    let [_, dec1] =
        solution.image_to_celestial([reference[0], reference[1] + 1.0]).expect("in domain");

    // The fixture's matrix is 0.0005 degrees per pixel on the second axis.
    let step = (dec1 - dec0).abs();
    assert!((step - 0.0005).abs() < 1e-7, "one pixel moved {step} degrees in declination");
    assert!(ra0 > 0.0);
}

/// Every projection must work end to end, not merely in isolation: the
/// reference point is the one place the answer is known for all of them.
#[test]
fn every_projection_evaluates_at_the_reference_point() {
    for name in [
        "Gnomonic",
        "Stereographic",
        "ZenithalEqualArea",
        "Orthographic",
        "PlateCarree",
        "Mercator",
        "HammerAitoff",
    ] {
        let (mut props, vecs) = layer_one();
        props[1] = ("ProjectionSystem".into(), name.into());
        let solution = read(&props, &vecs).expect("read").expect("a solution");

        let [ra, dec] = solution
            .image_to_celestial(solution.projection.reference_image)
            .unwrap_or_else(|| panic!("{name}: the reference point was out of domain"));
        assert!((ra - 10.684).abs() < 1e-8, "{name}: right ascension {ra}");
        assert!((dec - 41.269).abs() < 1e-8, "{name}: declination {dec}");
    }
}

/// A distortion model must actually reach the evaluation. A model whose
/// structures load but stay empty would report a complete solution and quietly
/// evaluate at second-layer accuracy, which is the kind of wrongness that
/// never announces itself -- so this pins that the terms are loaded and that
/// they move the answer.
#[test]
fn a_distortion_model_is_loaded_and_changes_the_result() {
    let identity = vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    let (mut props, mut vecs) = layer_one();

    for which in ["ImageToProjection", "ProjectionToImage"] {
        props.push((format!("DistortionModel:{which}:BasisFunction"), "ThinPlateSpline".into()));
        props.push((format!("DistortionModel:{which}:Order"), "2".into()));
        props.push((format!("DistortionModel:{which}:Terms"), "Global".into()));
        // A Global term with no nodes and a first-degree polynomial: the
        // residual is the constant 0.001 in u and 0.002 in v, which is small
        // enough to be a plausible distortion and large enough to see.
        vecs.push((
            format!("DistortionModel:{which}:Global:X:Normalization"),
            "F64Vector".into(),
            vec![0.0, 0.0, 1.0],
        ));
        vecs.push((format!("DistortionModel:{which}:Global:X:Nodes"), "F64Matrix".into(), vec![]));
        vecs.push((
            format!("DistortionModel:{which}:Global:X:Coefficients"),
            "F64Vector".into(),
            vec![0.001, 0.0, 0.0],
        ));
        vecs.push((
            format!("DistortionModel:{which}:Global:Y:Coefficients"),
            "F64Vector".into(),
            vec![0.002, 0.0, 0.0],
        ));
    }
    vecs.push((
        "ProjectiveTransformation:ImageToProjection".into(),
        "F64Matrix".into(),
        identity.clone(),
    ));
    vecs.push(("ProjectiveTransformation:ProjectionToImage".into(), "F64Matrix".into(), identity));

    let solution = read(&props, &vecs).expect("read").expect("a solution");

    let model = solution.distortion.as_ref().expect("the distortion model was dropped");
    let global = model.image_to_projection.global.as_ref().expect("the Global term was not loaded");
    assert!(global.x.nodes.is_empty(), "this fixture has no nodes");
    assert_eq!(global.x.coefficients.len(), 3, "three polynomial coefficients");
    assert_eq!(solution.availability, xisf_core::astrometry::Availability::Complete);

    // The residual is constant, so it must appear as exactly that offset in
    // the projection plane wherever it is evaluated.
    let mut scratch = Vec::new();
    let residual = model.image_to_projection.residual([5.0, 9.0], &mut scratch);
    assert!((residual[0] - 0.001).abs() < 1e-12, "u residual {}", residual[0]);
    assert!((residual[1] - 0.002).abs() < 1e-12, "v residual {}", residual[1]);
}

/// A coefficient vector that does not match the node count means the model
/// would be evaluated with coefficients belonging to something else.
#[test]
fn a_mismatched_coefficient_count_costs_the_distortion_layer() {
    let identity = vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    let (mut props, mut vecs) = layer_one();
    for which in ["ImageToProjection", "ProjectionToImage"] {
        props.push((format!("DistortionModel:{which}:BasisFunction"), "ThinPlateSpline".into()));
        props.push((format!("DistortionModel:{which}:Order"), "2".into()));
        props.push((format!("DistortionModel:{which}:Terms"), "Global".into()));
        vecs.push((
            format!("DistortionModel:{which}:Global:X:Normalization"),
            "F64Vector".into(),
            vec![0.0, 0.0, 1.0],
        ));
        vecs.push((format!("DistortionModel:{which}:Global:X:Nodes"), "F64Matrix".into(), vec![]));
        // Order 2 with a polynomial part needs three coefficients; this has two.
        vecs.push((
            format!("DistortionModel:{which}:Global:X:Coefficients"),
            "F64Vector".into(),
            vec![0.001, 0.0],
        ));
        vecs.push((
            format!("DistortionModel:{which}:Global:Y:Coefficients"),
            "F64Vector".into(),
            vec![0.002, 0.0],
        ));
    }
    vecs.push((
        "ProjectiveTransformation:ImageToProjection".into(),
        "F64Matrix".into(),
        identity.clone(),
    ));
    vecs.push(("ProjectiveTransformation:ProjectionToImage".into(), "F64Matrix".into(), identity));

    let solution = read(&props, &vecs).expect("read").expect("a solution");
    assert!(solution.distortion.is_none(), "a malformed model was kept");
    assert_eq!(solution.usable_layer(), 2, "the layers below it should survive");
}

/// Local terms are packed: all of a direction's terms share one node array,
/// one coefficient array and one array of normalizations, divided by a vector
/// of node offsets. Nothing exercised that unpacking -- the only distortion
/// test uses a single Global term with no nodes at all.
///
/// An off-by-one in the coefficient slice (forgetting that each term also
/// carries its own polynomial coefficients, so the stride is `i * Q`) hands
/// every term its neighbour's coefficients. The solution still evaluates and
/// still round-trips; it is simply wrong by whatever the local distortion is.
#[test]
fn local_terms_are_unpacked_by_their_node_offsets() {
    let identity = vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    let (mut props, mut vecs) = layer_one();

    // Three Local terms with different node counts -- 2, 0 and 1 -- so a
    // uniform stride cannot pass. Order 2 means Q = 3 polynomial coefficients
    // each; all radial coefficients are zero and the polynomial constant of
    // term i is i + 1, so each term's residual is a number that names it.
    let offsets: Vec<f64> = vec![0.0, 2.0, 2.0]; // written as I32Vector below
    let nodes: Vec<f64> = vec![0.1, 0.1, 0.2, 0.2, 0.3, 0.3]; // 3 nodes total
    let _ = &offsets;

    for which in ["ImageToProjection", "ProjectionToImage"] {
        props.push((format!("DistortionModel:{which}:BasisFunction"), "ThinPlateSpline".into()));
        props.push((format!("DistortionModel:{which}:Order"), "2".into()));
        props.push((format!("DistortionModel:{which}:Terms"), "Local".into()));

        // Far apart, with small radii, so exactly one term covers each centre.
        vecs.push((
            format!("DistortionModel:{which}:Local:Center"),
            "F64Matrix".into(),
            vec![0.0, 0.0, 1000.0, 0.0, 0.0, 1000.0],
        ));
        vecs.push((
            format!("DistortionModel:{which}:Local:Radius"),
            "F64Vector".into(),
            vec![10.0, 10.0, 10.0],
        ));
        vecs.push((
            format!("DistortionModel:{which}:Local:X:Normalization"),
            "F64Matrix".into(),
            vec![0.0, 0.0, 1.0, 1000.0, 0.0, 1.0, 0.0, 1000.0, 1.0],
        ));
        vecs.push((
            format!("DistortionModel:{which}:Local:X:Nodes"),
            "F64Matrix".into(),
            nodes.clone(),
        ));
        // Coefficients: per term, (radial for its nodes) then (1, 0, 0) scaled
        // so the constant is the term number.
        vecs.push((
            format!("DistortionModel:{which}:Local:X:Coefficients"),
            "F64Vector".into(),
            vec![
                0.0, 0.0, 1.0, 0.0, 0.0, // term 0: 2 nodes + constant 1
                2.0, 0.0, 0.0, // term 1: 0 nodes + constant 2
                0.0, 3.0, 0.0, 0.0, // term 2: 1 node + constant 3
            ],
        ));
        vecs.push((
            format!("DistortionModel:{which}:Local:Y:Coefficients"),
            "F64Vector".into(),
            vec![
                0.0, 0.0, 10.0, 0.0, 0.0, //
                20.0, 0.0, 0.0, //
                0.0, 30.0, 0.0, 0.0,
            ],
        ));
    }
    vecs.push((
        "ProjectiveTransformation:ImageToProjection".into(),
        "F64Matrix".into(),
        identity.clone(),
    ));
    vecs.push(("ProjectiveTransformation:ProjectionToImage".into(), "F64Matrix".into(), identity));

    // The node offsets are an I32Vector, which the array helper cannot build,
    // so this fixture is assembled by hand below.
    let solution = read_with_offsets(&props, &vecs, &[0i32, 2, 2]).expect("a solution");
    let model = solution.distortion.as_ref().expect("the distortion model was dropped");
    let direction = &model.image_to_projection;
    assert_eq!(direction.local.len(), 3, "three Local terms");

    // Each term owns its own node slice...
    assert_eq!(direction.local[0].spline.x.nodes.len(), 2);
    assert_eq!(direction.local[1].spline.x.nodes.len(), 0);
    assert_eq!(direction.local[2].spline.x.nodes.len(), 1);

    // ...and its own coefficients, which the residual at each centre names.
    let mut scratch = Vec::new();
    for (i, expected) in [(0usize, 1.0), (1, 2.0), (2, 3.0)] {
        let centre = direction.local[i].center;
        let residual = direction.residual(centre, &mut scratch);
        assert!(
            (residual[0] - expected).abs() < 1e-9,
            "term {i} at {centre:?} gave {residual:?}, wanted x = {expected}"
        );
        assert!((residual[1] - expected * 10.0).abs() < 1e-9, "term {i} Y component: {residual:?}");
    }
}

/// The Fallback term's weight is `W(s/t0)`, with `s` the summed Local coverage
/// and `t0` the threshold -- so it "contributes nothing" where coverage is
/// good and "takes over where the coverage of the Local terms fails".
///
/// Nothing tested the composition. If the weight is mis-normalized, every
/// point in the well-covered interior gets a blend of the local fit and the
/// coarse global one: a smooth, plausible, systematically biased solution
/// across the whole frame.
#[test]
fn a_fallback_term_contributes_nothing_where_coverage_is_good() {
    use xisf_core::astrometry::{
        BasisFunction, DistortionDirection, FallbackTerm, LocalTerm, Spline, SplinePair, TermKind,
    };

    // A constant residual per term, so the blend is readable directly.
    let constant = |value: f64| Spline {
        normalization: [0.0, 0.0, 1.0],
        nodes: Vec::new(),
        coefficients: vec![value, 0.0, 0.0],
        shape_parameter: None,
    };
    let pair = |x: f64| SplinePair { x: constant(x), y: constant(x) };

    let direction = DistortionDirection {
        basis_function: BasisFunction::ThinPlateSpline,
        order: 2,
        polynomial: true,
        terms: vec![TermKind::Local, TermKind::Fallback],
        global: None,
        local: vec![LocalTerm { center: [0.0, 0.0], radius: 10.0, spline: pair(1.0) }],
        fallback: Some(FallbackTerm { threshold: 0.5, spline: pair(9.0) }),
    };

    let mut scratch = Vec::new();
    // At the centre the Local weight is 1, which is >= the threshold, so the
    // Fallback weight is W(2) = 0 and the Local term answers alone.
    let inside = direction.residual([0.0, 0.0], &mut scratch);
    assert!((inside[0] - 1.0).abs() < 1e-12, "the Fallback biased a covered point: {inside:?}");

    // Far outside every disc the Local coverage is zero, so the Fallback
    // spline is the whole residual field.
    let outside = direction.residual([1000.0, 1000.0], &mut scratch);
    assert!((outside[0] - 9.0).abs() < 1e-12, "the Fallback did not take over: {outside:?}");

    // And in between it is a genuine blend, not a step.
    let between = direction.residual([9.5, 0.0], &mut scratch);
    assert!(between[0] > 1.0 && between[0] < 9.0, "the transition was not continuous: {between:?}");
}

/// "If the sum of weights is zero at a point, which can only happen in a
/// direction without a Fallback term, R_D(p) shall be the value of the Local
/// term with the smallest t, or zero if there are no terms."
#[test]
fn without_a_fallback_the_nearest_local_term_answers_beyond_the_coverage() {
    use xisf_core::astrometry::{
        BasisFunction, DistortionDirection, LocalTerm, Spline, SplinePair, TermKind,
    };

    let constant = |value: f64| Spline {
        normalization: [0.0, 0.0, 1.0],
        nodes: Vec::new(),
        coefficients: vec![value, 0.0, 0.0],
        shape_parameter: None,
    };
    let pair = |x: f64| SplinePair { x: constant(x), y: constant(x) };

    let mut direction = DistortionDirection {
        basis_function: BasisFunction::ThinPlateSpline,
        order: 2,
        polynomial: true,
        terms: vec![TermKind::Local],
        global: None,
        local: vec![
            LocalTerm { center: [0.0, 0.0], radius: 1.0, spline: pair(1.0) },
            LocalTerm { center: [500.0, 0.0], radius: 1.0, spline: pair(2.0) },
        ],
        fallback: None,
    };

    let mut scratch = Vec::new();
    // Beyond every disc: the nearest term by t, which is the first one.
    let far = direction.residual([10.0, 0.0], &mut scratch);
    assert!((far[0] - 1.0).abs() < 1e-12, "the nearest term did not answer: {far:?}");
    // Nearer the second one, it should answer instead.
    let other = direction.residual([490.0, 0.0], &mut scratch);
    assert!((other[0] - 2.0).abs() < 1e-12, "the wrong term answered: {other:?}");

    // "or zero if there are no terms"
    direction.local.clear();
    assert_eq!(direction.residual([0.0, 0.0], &mut scratch), [0.0, 0.0]);
}
