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
fn with_solution(properties: &[(&str, &str)], vectors: &[(&str, &str, &[f64])]) -> Vec<u8> {
    let mut body = String::new();
    for (id, value) in properties {
        body.push_str(&format!(
            r#"<Property id="AstrometricSolution:{id}" type="String">{value}</Property>"#
        ));
    }
    for (id, kind, values) in vectors {
        let mut raw = Vec::new();
        for v in *values {
            raw.extend_from_slice(&v.to_le_bytes());
        }
        let shape = if *kind == "F64Matrix" {
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
type Scalars = Vec<(&'static str, &'static str)>;
/// A solution's array properties: identifier, XISF type, and values.
type Arrays = Vec<(&'static str, &'static str, Vec<f64>)>;

/// The minimum a valid solution needs: layer 1 and nothing else.
fn layer_one() -> (Scalars, Arrays) {
    (
        vec![("Version", "1.0"), ("ProjectionSystem", "Gnomonic")],
        vec![
            ("ReferenceCelestialCoordinates", "F64Vector", vec![10.684, 41.269]),
            ("ReferenceImageCoordinates", "F64Vector", vec![8.0, 8.0]),
            ("LinearTransformationMatrix", "F64Matrix", vec![-0.0005, 0.0, 0.0, 0.0005]),
        ],
    )
}

fn read(
    properties: &[(&str, &str)],
    vectors: &[(&str, &str, Vec<f64>)],
) -> xisf_core::Result<Option<astrometry::Solution>> {
    let vectors: Vec<(&str, &str, &[f64])> =
        vectors.iter().map(|(a, b, c)| (*a, *b, c.as_slice())).collect();
    let bytes = with_solution(properties, &vectors);
    let reader = Reader::from_bytes(bytes).expect("header");
    let image = reader.header().images()[0];
    astrometry::read(&reader, image)
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
    props[1] = ("ProjectionSystem", "Fisheye");
    let err = read(&props, &vecs).expect_err("an unknown projection was accepted");
    assert_eq!(err.kind(), xisf_core::ErrorKind::Unsupported);
    assert!(err.message().contains("whole solution unavailable"), "{}", err.message());
}

/// "A decoder that does not support the major revision of a solution shall not
/// interpret any part of the solution, not even its first layer."
#[test]
fn an_unsupported_major_revision_is_not_interpreted_at_all() {
    let (mut props, vecs) = layer_one();
    props[0] = ("Version", "2.0");
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
    props.push(("DistortionModel:ImageToProjection:BasisFunction", "MysteryKernel"));
    props.push(("DistortionModel:ImageToProjection:Order", "2"));
    props.push(("DistortionModel:ImageToProjection:Terms", "Global"));
    props.push(("DistortionModel:ProjectionToImage:BasisFunction", "ThinPlateSpline"));
    props.push(("DistortionModel:ProjectionToImage:Order", "2"));
    props.push(("DistortionModel:ProjectionToImage:Terms", "Global"));
    vecs.push((
        "ProjectiveTransformation:ImageToProjection",
        "F64Matrix",
        vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    ));
    vecs.push((
        "ProjectiveTransformation:ProjectionToImage",
        "F64Matrix",
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
        "ProjectiveTransformation:ImageToProjection",
        "F64Matrix",
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
    props.push(("CelestialReferenceSystem", "FK5"));
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
    props.push(("Catalog", "Gaia DR3"));
    props.push(("CreatorApplication", "SomeSolver 1.2"));
    props.push(("CreatorOS", "Linux"));
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
        props[1] = ("ProjectionSystem", name);
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
        props.push((
            Box::leak(format!("DistortionModel:{which}:BasisFunction").into_boxed_str()),
            "ThinPlateSpline",
        ));
        props.push((Box::leak(format!("DistortionModel:{which}:Order").into_boxed_str()), "2"));
        props
            .push((Box::leak(format!("DistortionModel:{which}:Terms").into_boxed_str()), "Global"));
        // A Global term with no nodes and a first-degree polynomial: the
        // residual is the constant 0.001 in u and 0.002 in v, which is small
        // enough to be a plausible distortion and large enough to see.
        let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
        vecs.push((
            leak(format!("DistortionModel:{which}:Global:X:Normalization")),
            "F64Vector",
            vec![0.0, 0.0, 1.0],
        ));
        vecs.push((leak(format!("DistortionModel:{which}:Global:X:Nodes")), "F64Matrix", vec![]));
        vecs.push((
            leak(format!("DistortionModel:{which}:Global:X:Coefficients")),
            "F64Vector",
            vec![0.001, 0.0, 0.0],
        ));
        vecs.push((
            leak(format!("DistortionModel:{which}:Global:Y:Coefficients")),
            "F64Vector",
            vec![0.002, 0.0, 0.0],
        ));
    }
    vecs.push(("ProjectiveTransformation:ImageToProjection", "F64Matrix", identity.clone()));
    vecs.push(("ProjectiveTransformation:ProjectionToImage", "F64Matrix", identity));

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
        let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
        props.push((leak(format!("DistortionModel:{which}:BasisFunction")), "ThinPlateSpline"));
        props.push((leak(format!("DistortionModel:{which}:Order")), "2"));
        props.push((leak(format!("DistortionModel:{which}:Terms")), "Global"));
        vecs.push((
            leak(format!("DistortionModel:{which}:Global:X:Normalization")),
            "F64Vector",
            vec![0.0, 0.0, 1.0],
        ));
        vecs.push((leak(format!("DistortionModel:{which}:Global:X:Nodes")), "F64Matrix", vec![]));
        // Order 2 with a polynomial part needs three coefficients; this has two.
        vecs.push((
            leak(format!("DistortionModel:{which}:Global:X:Coefficients")),
            "F64Vector",
            vec![0.001, 0.0],
        ));
        vecs.push((
            leak(format!("DistortionModel:{which}:Global:Y:Coefficients")),
            "F64Vector",
            vec![0.002, 0.0],
        ));
    }
    vecs.push(("ProjectiveTransformation:ImageToProjection", "F64Matrix", identity.clone()));
    vecs.push(("ProjectiveTransformation:ProjectionToImage", "F64Matrix", identity));

    let solution = read(&props, &vecs).expect("read").expect("a solution");
    assert!(solution.distortion.is_none(), "a malformed model was kept");
    assert_eq!(solution.usable_layer(), 2, "the layers below it should survive");
}
