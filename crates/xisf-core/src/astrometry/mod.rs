//! The `AstrometricSolution` property namespace: where an image is on the sky.
//!
//! A solution is "a transformation between the image coordinates of an XISF
//! image and celestial coordinates on the sky", stored as a model that any
//! decoder can evaluate from the properties alone. It is organized in four
//! layers, and the layering is the part worth understanding, because it
//! decides what a decoder does when it meets something it does not know:
//!
//! | Layer | Contents | Presence |
//! |---|---|---|
//! | 1 | Projection: system, reference point, linear transformation | required |
//! | 2 | Projective transformation: a 3x3 matrix each way | optional |
//! | 3 | Distortion: residual fields as radial basis function splines | optional |
//! | 4 | Provenance: control points, catalog, creator | optional |
//!
//! "Each layer is complete on its own, and each layer is the fallback for the
//! next one": a decoder that cannot use a layer steps down to the previous one
//! and still has a valid, if less accurate, solution. So an unrecognized
//! basis function costs the distortion model and nothing else, while an
//! unrecognized projection system costs everything, since the whole geometry
//! rests on it. Those rules are encoded in [`Availability`] rather than left
//! to each caller to rediscover.
//!
//! # What this module does not do
//!
//! It models and validates a solution; it does not *evaluate* one. Turning a
//! pixel into a right ascension needs the deprojection, spherical rotation and
//! spline arithmetic, and the specification requires two implementations to
//! agree "to within 10^-6 pixels in image coordinates". The formulas for those
//! steps live in Annex A, which the specification marks *informative*, and
//! which restates the WCS paper; the normative reference is the WCS paper
//! itself. Shipping unverified numerics under an astrometric API would be
//! worse than shipping none: a solution that is subtly wrong puts objects in
//! the wrong place silently, which is the one failure this library's users
//! cannot afford to have hidden from them.

use alloc::collections::BTreeMap;

use crate::err;
use crate::error::Result;

mod eval;
mod load;
mod projection;
mod spline;

pub use projection::{Rotation, celestial_to_native, deproject, native_to_celestial, project};
pub use spline::{Spline, monomials, polynomial_terms, wendland};

/// The prefix every property in this namespace carries.
pub const NAMESPACE: &str = "AstrometricSolution";

/// The revision of the astrometric subsection a solution conforms to.
///
/// Numbered independently of the XISF specification document. "Within a major
/// revision, changes are additive: a minor revision may add properties and
/// vocabulary identifiers, but shall not alter the meaning of any existing
/// property or identifier. A major revision may change anything."
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
}

impl Version {
    /// The revision this module implements.
    pub const SUPPORTED: Version = Version { major: 1, minor: 0 };

    pub fn parse(text: &str) -> Result<Self> {
        let (major, minor) = text.trim().split_once('.').ok_or_else(|| {
            err!(BadAttribute, "an astrometric solution version must be major.minor, got {text:?}")
        })?;
        let number = |field: &str, what: &str| -> Result<u32> {
            field.trim().parse::<u32>().map_err(|_| {
                err!(BadAttribute, "the {what} version {field:?} is not an unsigned integer")
            })
        };
        Ok(Version { major: number(major, "major")?, minor: number(minor, "minor")? })
    }

    /// Whether this module may interpret a solution of this revision.
    ///
    /// "A decoder supporting revision X.Y shall accept any revision X.Z,
    /// ignoring the properties it does not recognize. A decoder that does not
    /// support the major revision of a solution shall not interpret any part
    /// of the solution, not even its first layer, and shall preserve all of
    /// its properties unchanged when writing the image."
    ///
    /// A *newer minor* revision is therefore readable, because additions
    /// within a major revision cannot change what is already there. A
    /// different major revision is not readable at all.
    pub fn is_supported(self) -> bool {
        self.major == Self::SUPPORTED.major
    }
}

impl core::fmt::Display for Version {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The geometry family a projection belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProjectionClass {
    Zenithal,
    Cylindrical,
    PseudoCylindrical,
}

/// The projection systems the specification defines.
///
/// "Decoders shall treat an unrecognized identifier as making the whole
/// solution unavailable" -- which is why this has no `Other` variant, unlike
/// the more forgiving vocabularies elsewhere in the format. The entire
/// geometry is defined in terms of the projection, so a decoder that does not
/// know it has nothing it can correctly do with the rest.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProjectionSystem {
    Gnomonic,
    Stereographic,
    ZenithalEqualArea,
    Orthographic,
    PlateCarree,
    Mercator,
    HammerAitoff,
}

impl ProjectionSystem {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.trim() {
            "Gnomonic" => ProjectionSystem::Gnomonic,
            "Stereographic" => ProjectionSystem::Stereographic,
            "ZenithalEqualArea" => ProjectionSystem::ZenithalEqualArea,
            "Orthographic" => ProjectionSystem::Orthographic,
            "PlateCarree" => ProjectionSystem::PlateCarree,
            "Mercator" => ProjectionSystem::Mercator,
            "HammerAitoff" => ProjectionSystem::HammerAitoff,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            ProjectionSystem::Gnomonic => "Gnomonic",
            ProjectionSystem::Stereographic => "Stereographic",
            ProjectionSystem::ZenithalEqualArea => "ZenithalEqualArea",
            ProjectionSystem::Orthographic => "Orthographic",
            ProjectionSystem::PlateCarree => "PlateCarree",
            ProjectionSystem::Mercator => "Mercator",
            ProjectionSystem::HammerAitoff => "HammerAitoff",
        }
    }

    pub fn class(self) -> ProjectionClass {
        match self {
            ProjectionSystem::Gnomonic
            | ProjectionSystem::Stereographic
            | ProjectionSystem::ZenithalEqualArea
            | ProjectionSystem::Orthographic => ProjectionClass::Zenithal,
            ProjectionSystem::PlateCarree | ProjectionSystem::Mercator => {
                ProjectionClass::Cylindrical
            }
            ProjectionSystem::HammerAitoff => ProjectionClass::PseudoCylindrical,
        }
    }

    /// The default native longitude and latitude of the reference point, in
    /// degrees, when `ReferenceNativeCoordinates` is absent.
    ///
    /// "the default values shall be (0, 90) for zenithal projections and
    /// (0, 0) for all other projections, as prescribed by the WCS
    /// formulation."
    pub fn default_reference_native(self) -> [f64; 2] {
        match self.class() {
            ProjectionClass::Zenithal => [0.0, 90.0],
            _ => [0.0, 0.0],
        }
    }
}

/// The celestial reference system a solution's coordinates are referred to.
///
/// Unlike the projection vocabulary this one tolerates the unknown: "An
/// unrecognized identifier does not make any layer unavailable, since the
/// geometric transformation remains fully defined. It only prevents
/// converting the coordinates produced by the solution to a different
/// reference system."
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum CelestialReferenceSystem {
    /// The International Celestial Reference System, and the default when the
    /// property is absent.
    #[default]
    Icrs,
    /// The Geocentric Celestial Reference System.
    Gcrs,
    /// An identifier this revision does not define. The geometry is still
    /// usable; only conversion to another system is not.
    Other(String),
}

impl CelestialReferenceSystem {
    pub fn parse(name: &str) -> Self {
        match name.trim() {
            "ICRS" => CelestialReferenceSystem::Icrs,
            "GCRS" => CelestialReferenceSystem::Gcrs,
            other => CelestialReferenceSystem::Other(other.to_string()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            CelestialReferenceSystem::Icrs => "ICRS",
            CelestialReferenceSystem::Gcrs => "GCRS",
            CelestialReferenceSystem::Other(name) => name,
        }
    }
}

/// The radial basis functions a distortion model may use.
///
/// "Decoders shall treat an unrecognized identifier as making the third layer
/// unavailable", so again there is no `Other`: a spline evaluated with the
/// wrong kernel is not an approximation, it is a different function.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BasisFunction {
    ThinPlateSpline,
    VariableOrder,
    Gaussian,
    Multiquadric,
    InverseMultiquadric,
    InverseQuadratic,
}

impl BasisFunction {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.trim() {
            "ThinPlateSpline" => BasisFunction::ThinPlateSpline,
            "VariableOrder" => BasisFunction::VariableOrder,
            "Gaussian" => BasisFunction::Gaussian,
            "Multiquadric" => BasisFunction::Multiquadric,
            "InverseMultiquadric" => BasisFunction::InverseMultiquadric,
            "InverseQuadratic" => BasisFunction::InverseQuadratic,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            BasisFunction::ThinPlateSpline => "ThinPlateSpline",
            BasisFunction::VariableOrder => "VariableOrder",
            BasisFunction::Gaussian => "Gaussian",
            BasisFunction::Multiquadric => "Multiquadric",
            BasisFunction::InverseMultiquadric => "InverseMultiquadric",
            BasisFunction::InverseQuadratic => "InverseQuadratic",
        }
    }

    /// Whether this kernel takes a shape parameter, which decides whether the
    /// `ShapeParameter` properties must be present -- the specification
    /// requires them "if and only if the basis function has a shape
    /// parameter".
    pub fn has_shape_parameter(self) -> bool {
        matches!(
            self,
            BasisFunction::Gaussian
                | BasisFunction::Multiquadric
                | BasisFunction::InverseMultiquadric
                | BasisFunction::InverseQuadratic
        )
    }

    /// Whether the polynomial part is required rather than optional. "The
    /// value must be true for the ThinPlateSpline and VariableOrder basis
    /// functions."
    pub fn requires_polynomial(self) -> bool {
        matches!(self, BasisFunction::ThinPlateSpline | BasisFunction::VariableOrder)
    }

    /// The smallest legal order.
    ///
    /// Table 17 gives these separately, and they differ: a thin plate spline
    /// is `m >= 2`, while `VariableOrder` is `m >= 3`, because "a kernel of
    /// this family with order 2 is a thin plate spline and shall use the
    /// ThinPlateSpline identifier". The two identifiers therefore partition
    /// the same family rather than overlapping on order 2.
    pub fn minimum_order(self) -> i32 {
        match self {
            BasisFunction::ThinPlateSpline => 2,
            BasisFunction::VariableOrder => 3,
            _ => 0,
        }
    }

    /// The kernel itself: `phi(rho)`, with `rho` the distance in normalized
    /// coordinates and `epsilon` the shape parameter where the kernel takes
    /// one. Table 17.
    ///
    /// The logarithmic kernels are defined to be zero at the origin, which
    /// they do not reach by limit alone -- `rho^2 ln rho` is an indeterminate
    /// form there, and floating point gives `-inf * 0 = NaN` rather than 0.
    pub fn eval(self, rho: f64, order: i32, epsilon: f64) -> f64 {
        match self {
            BasisFunction::ThinPlateSpline => {
                if rho <= 0.0 {
                    0.0
                } else {
                    rho * rho * rho.ln()
                }
            }
            // (rho^2)^(m-1) ln(rho^2). Written on rho^2 as the table does,
            // which also keeps it exact for even powers.
            BasisFunction::VariableOrder => {
                if rho <= 0.0 {
                    0.0
                } else {
                    let r2 = rho * rho;
                    r2.powi(order - 1) * r2.ln()
                }
            }
            BasisFunction::Gaussian => (-(epsilon * rho).powi(2)).exp(),
            BasisFunction::Multiquadric => (1.0 + (epsilon * rho).powi(2)).sqrt(),
            BasisFunction::InverseMultiquadric => 1.0 / (1.0 + (epsilon * rho).powi(2)).sqrt(),
            BasisFunction::InverseQuadratic => 1.0 / (1.0 + (epsilon * rho).powi(2)),
        }
    }
}

/// The kinds of term a distortion model may be built from.
///
/// "Decoders shall treat an unrecognized identifier in this list as making the
/// third layer unavailable, so that a model with terms the decoder cannot
/// evaluate is never evaluated partially."
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum TermKind {
    /// Weight 1 everywhere. At most one per direction.
    Global,
    /// Compactly supported on a disc, with a Wendland weight function.
    Local,
    /// Carries the large-scale shape where Local coverage fails. At most one
    /// per direction, and only together with Local terms.
    Fallback,
}

impl TermKind {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.trim() {
            "Global" => TermKind::Global,
            "Local" => TermKind::Local,
            "Fallback" => TermKind::Fallback,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            TermKind::Global => "Global",
            TermKind::Local => "Local",
            TermKind::Fallback => "Fallback",
        }
    }

    /// Parse the newline-separated `Terms` property.
    ///
    /// An unrecognized identifier is an error rather than a skipped entry,
    /// because the whole point of the rule is that the layer must not be
    /// evaluated partially.
    pub fn parse_list(text: &str) -> Result<Vec<TermKind>> {
        let mut out = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let kind = TermKind::parse(line).ok_or_else(|| {
                err!(Unsupported, "unknown astrometric distortion term kind {line:?}")
            })?;
            if !out.contains(&kind) {
                out.push(kind);
            }
        }
        if out.is_empty() {
            return Err(err!(BadHeader, "a distortion model lists no term kinds"));
        }
        if out.contains(&TermKind::Fallback) && !out.contains(&TermKind::Local) {
            return Err(err!(
                BadHeader,
                "a Fallback term may only exist together with Local terms"
            ));
        }
        Ok(out)
    }
}

/// Which layers of a solution a decoder may use.
///
/// The layering rule in one place: a layer that cannot be used costs that
/// layer and the ones above it, never the ones below.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Availability {
    /// Every layer present in the file is usable.
    Complete,
    /// Usable up to and including this layer; the ones above were dropped for
    /// the stated reason.
    UpTo { layer: u8, because: String },
    /// Nothing may be interpreted, "not even its first layer".
    None { because: String },
}

impl Availability {
    /// The highest layer a decoder may evaluate, or `None` for a solution it
    /// must leave alone entirely.
    pub fn usable_layer(&self) -> Option<u8> {
        match self {
            Availability::Complete => Some(4),
            Availability::UpTo { layer, .. } => Some(*layer),
            Availability::None { .. } => None,
        }
    }

    pub fn is_usable(&self) -> bool {
        self.usable_layer().is_some()
    }
}

/// The properties of one image under the `AstrometricSolution:` prefix.
///
/// Held as the raw name-to-element map the header gave, keyed by the part of
/// the identifier after the prefix, because a solution's vector and matrix
/// values live in data blocks that only a reader can fetch.
#[derive(Clone, Debug, Default)]
pub struct SolutionProperties {
    properties: BTreeMap<String, String>,
}

impl SolutionProperties {
    /// Record a property, by its identifier with the namespace prefix still on.
    pub fn insert(&mut self, id: &str, value: String) {
        if let Some(rest) = id.strip_prefix("AstrometricSolution:") {
            self.properties.insert(rest.to_string(), value);
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.properties.get(key).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.properties.is_empty()
    }

    /// Every recorded key, without the namespace prefix.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.properties.keys().map(String::as_str)
    }
}

/// Layer 1: the projection. Required, and complete on its own -- "a decoder
/// that implements nothing else obtains the linear solution".
#[derive(Clone, PartialEq, Debug)]
pub struct Projection {
    pub system: ProjectionSystem,
    /// Celestial coordinates of the reference point, in degrees.
    pub reference_celestial: [f64; 2],
    /// Image coordinates corresponding to the origin of the projection plane.
    pub reference_image: [f64; 2],
    /// Image to projection plane, in degrees per pixel, row-major.
    pub linear_transformation: [[f64; 2]; 2],
    /// Native spherical coordinates of the reference point. `None` means the
    /// projection's own default -- see
    /// [`ProjectionSystem::default_reference_native`].
    pub reference_native: Option<[f64; 2]>,
    /// Native spherical coordinates of the celestial pole, if stated.
    pub celestial_pole_native: Option<[f64; 2]>,
    pub celestial_reference_system: CelestialReferenceSystem,
}

/// Layer 2: a projective transformation each way, on homogeneous coordinates.
///
/// "Either both properties are present, or neither of them. The two
/// transformations are not required to be exact inverses of each other: each
/// one is the encoder's best fit for its direction." Both directions are
/// stored because "decoders shall not invert a stored transformation
/// numerically".
#[derive(Clone, PartialEq, Debug)]
pub struct ProjectiveTransformation {
    pub image_to_projection: [[f64; 3]; 3],
    pub projection_to_image: [[f64; 3]; 3],
}

/// Which way round an image-plane step goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    ImageToProjection,
    ProjectionToImage,
}

impl Direction {
    /// The token that appears in the property identifiers.
    pub fn name(self) -> &'static str {
        match self {
            Direction::ImageToProjection => "ImageToProjection",
            Direction::ProjectionToImage => "ProjectionToImage",
        }
    }
}

/// The two scalar splines of one term, one per output component.
///
/// "The Y component of a term may share the nodes, the normalization and the
/// shape parameter of the X component, in which case only its coefficients
/// are stored." Sharing is resolved when the model is loaded, so both are
/// complete splines here and evaluation need not know which case it was.
#[derive(Clone, PartialEq, Debug)]
pub struct SplinePair {
    pub x: Spline,
    pub y: Spline,
}

impl SplinePair {
    /// Both components at a point, as a vector-valued spline `S_i(p)`.
    pub fn eval(
        &self,
        point: [f64; 2],
        direction: &DistortionDirection,
        scratch: &mut Vec<f64>,
    ) -> [f64; 2] {
        let basis = direction.basis_function;
        let order = direction.order;
        let polynomial = direction.polynomial;
        [
            self.x.eval(point, basis, order, polynomial, scratch),
            self.y.eval(point, basis, order, polynomial, scratch),
        ]
    }
}

/// A Local term: a spline on the disc with the given centre and radius.
#[derive(Clone, PartialEq, Debug)]
pub struct LocalTerm {
    /// The centre of the support disc, in the direction's source coordinates.
    pub center: [f64; 2],
    /// The radius of the support disc, in source units.
    pub radius: f64,
    pub spline: SplinePair,
}

/// A Fallback term: a spline over the whole field, weighted by how badly the
/// Local terms cover a point.
#[derive(Clone, PartialEq, Debug)]
pub struct FallbackTerm {
    /// The coverage threshold, in units of summed Local weight.
    pub threshold: f64,
    pub spline: SplinePair,
}

/// Layer 3, for one direction.
#[derive(Clone, PartialEq, Debug)]
pub struct DistortionDirection {
    pub basis_function: BasisFunction,
    /// The order: the degree of the polynomial part plus one.
    pub order: i32,
    /// Whether the splines carry a polynomial part. Defaults to true, and
    /// must be true for the kernels that require it.
    pub polynomial: bool,
    pub terms: Vec<TermKind>,
    /// At most one Global term.
    pub global: Option<SplinePair>,
    /// The Local terms, in the order the packed properties store them.
    pub local: Vec<LocalTerm>,
    /// At most one Fallback term, and only alongside Local terms.
    pub fallback: Option<FallbackTerm>,
}

impl DistortionDirection {
    /// Check the combination against the rules the vocabulary imposes.
    pub fn validate(&self) -> Result<()> {
        if self.order < self.basis_function.minimum_order() {
            return Err(err!(
                BadHeader,
                "the {} basis function requires an order of at least {}, got {}",
                self.basis_function.name(),
                self.basis_function.minimum_order(),
                self.order
            ));
        }
        if self.basis_function.requires_polynomial() && !self.polynomial {
            return Err(err!(
                BadHeader,
                "the {} basis function requires a polynomial part",
                self.basis_function.name()
            ));
        }
        Ok(())
    }
}

/// Layer 3: the distortion model. "Both directions are present or neither of
/// them, and the second layer must be present whenever the third layer is."
#[derive(Clone, PartialEq, Debug)]
pub struct DistortionModel {
    pub image_to_projection: DistortionDirection,
    pub projection_to_image: DistortionDirection,
}

/// Layer 4: provenance. "None of these properties is needed to evaluate the
/// transformation, and none of them constrains the contents of the other
/// layers."
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Provenance {
    pub catalog: Option<String>,
    pub creation_time: Option<String>,
    pub creator_application: Option<String>,
    pub creator_module: Option<String>,
    pub creator_os: Option<String>,
}

/// An astrometric solution, as far as it can be interpreted.
#[derive(Clone, PartialEq, Debug)]
pub struct Solution {
    pub version: Version,
    pub projection: Projection,
    pub projective: Option<ProjectiveTransformation>,
    pub distortion: Option<DistortionModel>,
    pub provenance: Provenance,
    /// Why the layers above the ones present were not interpreted, if any
    /// were dropped.
    pub availability: Availability,
}

impl Solution {
    /// The highest layer a decoder may evaluate.
    pub fn usable_layer(&self) -> u8 {
        self.availability.usable_layer().unwrap_or(0)
    }
}

/// Read the `AstrometricSolution` properties of an image.
///
/// `Ok(None)` means the image carries no solution, which is the ordinary case.
/// An `Err` means it carries one that cannot be read at all.
///
/// Layer by layer, the rules the specification sets out are applied here:
/// an unsupported major revision stops everything, an unknown projection
/// system stops everything, and an unknown basis function or term kind stops
/// the distortion model while leaving the two layers below it usable.
pub fn read(reader: &crate::Reader, image: &crate::header::Element) -> Result<Option<Solution>> {
    use crate::property::{Property, Shape};

    // Gather the namespace. A solution is self-sufficient: "a decoder needs
    // nothing outside the namespace to evaluate the solution, and shall not
    // consult other properties or elements of the XISF unit for this purpose,
    // not even as a fallback."
    let mut scalars: BTreeMap<String, String> = BTreeMap::new();
    let mut blocks: BTreeMap<String, &crate::header::Element> = BTreeMap::new();

    for element in image.children_named("Property") {
        let Some(id) = element.attr("id") else { continue };
        let Some(key) = id.strip_prefix("AstrometricSolution:") else { continue };
        if element.data.location.is_some() {
            blocks.insert(key.to_string(), element);
        } else if let Some(value) = element.attr("value") {
            scalars.insert(key.to_string(), value.to_string());
        } else if let Some(text) = element.data.text.as_deref() {
            scalars.insert(key.to_string(), text.trim().to_string());
        }
    }
    if scalars.is_empty() && blocks.is_empty() {
        return Ok(None);
    }

    // Reading f64s out of a vector or matrix property means going to its data
    // block, which is what makes this need a reader rather than a header.
    let numbers = |key: &str| -> Result<Option<Vec<f64>>> {
        let Some(element) = blocks.get(key) else { return Ok(None) };
        let property = Property::parse(element)?;
        if !matches!(property.kind.shape, Shape::Vector | Shape::Matrix) {
            return Err(err!(
                BadHeader,
                "AstrometricSolution:{key} must be a vector or matrix property"
            ));
        }
        let bytes = reader.block(&element.data)?;
        let expected = property.data_size().unwrap_or(0);
        if bytes.len() as u64 != expected {
            return Err(err!(
                Truncated,
                "AstrometricSolution:{key} declares {expected} bytes but its block holds {}",
                bytes.len()
            ));
        }
        let (chunks, _) = bytes.as_chunks::<8>();
        let big = element.data.byte_order == crate::block::ByteOrder::Big;
        Ok(Some(
            chunks
                .iter()
                .map(|c| if big { f64::from_be_bytes(*c) } else { f64::from_le_bytes(*c) })
                .collect(),
        ))
    };

    let required_numbers = |key: &str, n: usize| -> Result<Vec<f64>> {
        let values = numbers(key)?.ok_or_else(|| {
            err!(BadHeader, "an astrometric solution has no AstrometricSolution:{key}")
        })?;
        if values.len() != n {
            return Err(err!(
                BadHeader,
                "AstrometricSolution:{key} needs {n} values, got {}",
                values.len()
            ));
        }
        Ok(values)
    };

    // The version gates everything, including whether layer 1 may be touched.
    let version_text = scalars.get("Version").ok_or_else(|| {
        err!(BadHeader, "an astrometric solution has no AstrometricSolution:Version")
    })?;
    let version = Version::parse(version_text)?;

    if !version.is_supported() {
        // "shall not interpret any part of the solution, not even its first
        // layer, and shall preserve all of its properties unchanged when
        // writing the image." Preserving them is automatic here, since
        // nothing in this crate rewrites properties it did not parse.
        return Err(err!(
            Unsupported,
            "astrometric solution revision {version} is not supported by this decoder, \
             which implements {}; its properties are preserved but not interpreted",
            Version::SUPPORTED
        ));
    }

    let system_text = scalars.get("ProjectionSystem").ok_or_else(|| {
        err!(BadHeader, "an astrometric solution has no AstrometricSolution:ProjectionSystem")
    })?;
    let Some(system) = ProjectionSystem::parse(system_text) else {
        // "Decoders shall treat an unrecognized identifier as making the
        // whole solution unavailable."
        return Err(err!(
            Unsupported,
            "unknown astrometric projection system {system_text:?}, \
             which makes the whole solution unavailable"
        ));
    };

    let pair = |v: Vec<f64>| -> [f64; 2] { [v[0], v[1]] };
    let linear = required_numbers("LinearTransformationMatrix", 4)?;

    let projection = Projection {
        system,
        reference_celestial: pair(required_numbers("ReferenceCelestialCoordinates", 2)?),
        reference_image: pair(required_numbers("ReferenceImageCoordinates", 2)?),
        linear_transformation: [[linear[0], linear[1]], [linear[2], linear[3]]],
        reference_native: numbers("ReferenceNativeCoordinates")?.map(pair),
        celestial_pole_native: numbers("CelestialPoleNativeCoordinates")?.map(pair),
        celestial_reference_system: scalars
            .get("CelestialReferenceSystem")
            .map(|s| CelestialReferenceSystem::parse(s))
            .unwrap_or_default(),
    };

    // Layer 2.
    let to_3x3 = |v: Vec<f64>| -> [[f64; 3]; 3] {
        [[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]]
    };
    let forward = numbers("ProjectiveTransformation:ImageToProjection")?;
    let inverse = numbers("ProjectiveTransformation:ProjectionToImage")?;
    let projective = match (forward, inverse) {
        (Some(f), Some(i)) if f.len() == 9 && i.len() == 9 => Some(ProjectiveTransformation {
            image_to_projection: to_3x3(f),
            projection_to_image: to_3x3(i),
        }),
        (None, None) => None,
        // "Either both properties are present, or neither of them."
        _ => {
            return Err(err!(
                BadHeader,
                "an astrometric solution has only one direction of its projective \
                 transformation, or one of the wrong size; both are required together"
            ));
        }
    };

    // Layer 3. An unknown basis function or term kind costs this layer alone.
    let mut availability = Availability::Complete;
    let mut distortion = None;
    let has_distortion = scalars.keys().any(|k| k.starts_with("DistortionModel:"));
    if has_distortion {
        let fetch = load::Blocks { reader, elements: blocks.clone(), scalars: &scalars };
        match read_distortion(&scalars).and_then(|mut model| {
            // The metadata parses from the header alone; the spline records
            // live in data blocks. Both have to succeed for the layer to be
            // usable, and both fail the same way -- by dropping this layer and
            // leaving the ones below it -- so they are attempted together.
            load::load(&mut model.image_to_projection, &fetch, "ImageToProjection")?;
            load::load(&mut model.projection_to_image, &fetch, "ProjectionToImage")?;
            Ok(model)
        }) {
            Ok(model) => {
                if projective.is_none() {
                    return Err(err!(
                        BadHeader,
                        "an astrometric solution has a distortion model but no projective \
                         transformation, which must be present whenever the third layer is"
                    ));
                }
                distortion = Some(model);
            }
            Err(e) => {
                availability = Availability::UpTo {
                    layer: if projective.is_some() { 2 } else { 1 },
                    because: e.message().to_string(),
                };
            }
        }
    }

    let provenance = Provenance {
        catalog: scalars.get("Catalog").cloned(),
        creation_time: scalars.get("CreationTime").cloned(),
        creator_application: scalars.get("CreatorApplication").cloned(),
        creator_module: scalars.get("CreatorModule").cloned(),
        creator_os: scalars.get("CreatorOS").cloned(),
    };

    Ok(Some(Solution { version, projection, projective, distortion, provenance, availability }))
}

fn read_distortion(scalars: &BTreeMap<String, String>) -> Result<DistortionModel> {
    let direction = |which: &str| -> Result<DistortionDirection> {
        let key = |leaf: &str| format!("DistortionModel:{which}:{leaf}");
        let text = |leaf: &str| scalars.get(&key(leaf)).map(String::as_str);

        let basis_text = text("BasisFunction")
            .ok_or_else(|| err!(BadHeader, "a distortion model direction has no BasisFunction"))?;
        let basis_function = BasisFunction::parse(basis_text).ok_or_else(|| {
            err!(
                Unsupported,
                "unknown astrometric basis function {basis_text:?}, \
                 which makes the distortion model unavailable"
            )
        })?;

        let order_text =
            text("Order").ok_or_else(|| err!(BadHeader, "a distortion model has no Order"))?;
        let order = order_text.trim().parse::<i32>().map_err(|_| {
            err!(BadAttribute, "a distortion model Order {order_text:?} is not an integer")
        })?;

        // Defaults to true when absent.
        let polynomial = match text("Polynomial") {
            None => true,
            Some(value) => match value.trim() {
                "true" | "1" => true,
                "false" | "0" => false,
                other => {
                    return Err(err!(BadAttribute, "Polynomial must be a Boolean, got {other:?}"));
                }
            },
        };

        let terms_text =
            text("Terms").ok_or_else(|| err!(BadHeader, "a distortion model has no Terms"))?;
        let terms = TermKind::parse_list(terms_text)?;

        let direction = DistortionDirection {
            basis_function,
            order,
            polynomial,
            terms,
            // The spline arrays live in data blocks, which this function does
            // not have a reader for; they are attached by `load_distortion`.
            global: None,
            local: Vec::new(),
            fallback: None,
        };
        direction.validate()?;
        Ok(direction)
    };

    Ok(DistortionModel {
        image_to_projection: direction("ImageToProjection")?,
        projection_to_image: direction("ProjectionToImage")?,
    })
}
