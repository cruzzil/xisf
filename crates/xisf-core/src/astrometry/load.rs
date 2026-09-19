//! Loading the bulk arrays of a distortion model out of their data blocks.
//!
//! The spline records are packed: all the Local terms of a direction share one
//! node array, one coefficient array and one array of normalizations, divided
//! between the terms by a vector of node offsets. Unpacking that is what this
//! module does, so that evaluation sees ordinary per-term splines and does not
//! have to carry the packing around.
//!
//! The Y component of a term "may share the nodes, the normalization and the
//! shape parameter of the X component, in which case only its coefficients are
//! stored". Sharing is resolved here for the same reason.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use super::spline::polynomial_terms;
use super::{DistortionDirection, FallbackTerm, LocalTerm, Spline, SplinePair};
use crate::err;
use crate::error::Result;
use crate::header::Element;

/// Fetches a solution's array properties from their data blocks.
pub(super) struct Blocks<'a> {
    pub reader: &'a crate::Reader,
    pub elements: BTreeMap<String, &'a Element>,
    pub scalars: &'a BTreeMap<String, String>,
}

impl Blocks<'_> {
    /// The raw bytes of a property's block, checked against its declared shape.
    fn bytes(&self, key: &str) -> Result<Option<(Vec<u8>, bool)>> {
        let Some(element) = self.elements.get(key) else { return Ok(None) };
        let property = crate::property::Property::parse(element)?;
        let data = self.reader.block(&element.data)?;
        let expected = property.data_size().unwrap_or(0);
        if data.len() as u64 != expected {
            return Err(err!(
                Truncated,
                "AstrometricSolution:{key} declares {expected} bytes but its block holds {}",
                data.len()
            ));
        }
        Ok(Some((data.into_owned(), element.data.byte_order == crate::block::ByteOrder::Big)))
    }

    /// An `F64Vector` or `F64Matrix` property as a flat run of doubles.
    pub fn f64s(&self, key: &str) -> Result<Option<Vec<f64>>> {
        let Some((bytes, big)) = self.bytes(key)? else { return Ok(None) };
        let (chunks, _) = bytes.as_chunks::<8>();
        Ok(Some(
            chunks
                .iter()
                .map(|c| if big { f64::from_be_bytes(*c) } else { f64::from_le_bytes(*c) })
                .collect(),
        ))
    }

    /// An `I32Vector` property, which the node offsets use.
    pub fn i32s(&self, key: &str) -> Result<Option<Vec<i32>>> {
        let Some((bytes, big)) = self.bytes(key)? else { return Ok(None) };
        let (chunks, _) = bytes.as_chunks::<4>();
        Ok(Some(
            chunks
                .iter()
                .map(|c| if big { i32::from_be_bytes(*c) } else { i32::from_le_bytes(*c) })
                .collect(),
        ))
    }

    /// A scalar `Float64` property, which lives in a `value` attribute rather
    /// than in a block.
    pub fn scalar(&self, key: &str) -> Result<Option<f64>> {
        let Some(text) = self.scalars.get(key) else { return Ok(None) };
        text.trim()
            .parse::<f64>()
            .map(Some)
            .map_err(|_| err!(BadAttribute, "AstrometricSolution:{key} is not a number"))
    }

    fn pairs(&self, key: &str) -> Result<Option<Vec<[f64; 2]>>> {
        Ok(self.f64s(key)?.map(|v| v.chunks_exact(2).map(|c| [c[0], c[1]]).collect()))
    }
}

/// Attach the spline records of one direction to its already-parsed metadata.
pub(super) fn load(
    direction: &mut DistortionDirection,
    blocks: &Blocks<'_>,
    which: &str,
) -> Result<()> {
    let prefix = |leaf: &str| alloc::format!("DistortionModel:{which}:{leaf}");
    let has_shape = direction.basis_function.has_shape_parameter();
    let order = direction.order;
    let polynomial = direction.polynomial;

    if direction.terms.contains(&super::TermKind::Global) {
        direction.global =
            Some(single_term(blocks, &prefix("Global"), has_shape, order, polynomial)?);
    }

    if direction.terms.contains(&super::TermKind::Local) {
        direction.local = local_terms(blocks, &prefix("Local"), has_shape, order, polynomial)?;
    }

    if direction.terms.contains(&super::TermKind::Fallback) {
        let threshold = blocks
            .scalar(&prefix("Fallback:Threshold"))?
            .ok_or_else(|| err!(BadHeader, "a Fallback term has no Threshold"))?;
        // Finite and positive. Spelled out rather than as a negated
        // comparison so that NaN is visibly rejected rather than incidentally.
        if !threshold.is_finite() || threshold <= 0.0 {
            return Err(err!(
                BadHeader,
                "a Fallback coverage threshold must be greater than zero, got {threshold}"
            ));
        }
        direction.fallback = Some(FallbackTerm {
            threshold,
            spline: single_term(blocks, &prefix("Fallback"), has_shape, order, polynomial)?,
        });
    }

    Ok(())
}

/// A Global or Fallback term: one spline record, with the Y component
/// optionally sharing the X component's nodes.
fn single_term(
    blocks: &Blocks<'_>,
    prefix: &str,
    has_shape: bool,
    order: i32,
    polynomial: bool,
) -> Result<SplinePair> {
    let key = |leaf: &str| alloc::format!("{prefix}:{leaf}");

    let normalization = blocks
        .f64s(&key("X:Normalization"))?
        .ok_or_else(|| err!(BadHeader, "a distortion term has no X:Normalization"))?;
    if normalization.len() != 3 {
        return Err(err!(BadHeader, "a spline normalization needs 3 values"));
    }
    let nodes = blocks
        .pairs(&key("X:Nodes"))?
        .ok_or_else(|| err!(BadHeader, "a distortion term has no X:Nodes"))?;
    let coefficients = blocks
        .f64s(&key("X:Coefficients"))?
        .ok_or_else(|| err!(BadHeader, "a distortion term has no X:Coefficients"))?;
    let shape = if has_shape { blocks.scalar(&key("X:ShapeParameter"))? } else { None };

    let x = Spline {
        normalization: [normalization[0], normalization[1], normalization[2]],
        nodes,
        coefficients,
        shape_parameter: shape,
    };
    check_length(&x, order, polynomial, prefix, "X")?;

    // The Y component shares everything but its coefficients unless it states
    // otherwise. "When Y:Nodes is specified, Y:Normalization must also be
    // specified, and Y:ShapeParameter must be specified if and only if the
    // basis function has a shape parameter."
    let y_coefficients = blocks
        .f64s(&key("Y:Coefficients"))?
        .ok_or_else(|| err!(BadHeader, "a distortion term has no Y:Coefficients"))?;
    let y = match blocks.pairs(&key("Y:Nodes"))? {
        None => Spline { coefficients: y_coefficients, ..x.clone() },
        Some(nodes) => {
            let normalization = blocks
                .f64s(&key("Y:Normalization"))?
                .ok_or_else(|| err!(BadHeader, "Y:Nodes was given without Y:Normalization"))?;
            if normalization.len() != 3 {
                return Err(err!(BadHeader, "a spline normalization needs 3 values"));
            }
            Spline {
                normalization: [normalization[0], normalization[1], normalization[2]],
                nodes,
                coefficients: y_coefficients,
                shape_parameter: if has_shape {
                    blocks.scalar(&key("Y:ShapeParameter"))?
                } else {
                    None
                },
            }
        }
    };
    check_length(&y, order, polynomial, prefix, "Y")?;

    Ok(SplinePair { x, y })
}

/// The Local terms, unpacked from the shared arrays by their node offsets.
fn local_terms(
    blocks: &Blocks<'_>,
    prefix: &str,
    has_shape: bool,
    order: i32,
    polynomial: bool,
) -> Result<Vec<LocalTerm>> {
    let key = |leaf: &str| alloc::format!("{prefix}:{leaf}");

    let centers = blocks
        .pairs(&key("Center"))?
        .ok_or_else(|| err!(BadHeader, "Local terms have no Center"))?;
    let radii = blocks
        .f64s(&key("Radius"))?
        .ok_or_else(|| err!(BadHeader, "Local terms have no Radius"))?;
    if centers.len() != radii.len() {
        return Err(err!(
            BadHeader,
            "Local terms declare {} centres and {} radii",
            centers.len(),
            radii.len()
        ));
    }
    let count = centers.len();

    let x = packed(blocks, &key("X"), count, has_shape)?;
    // The Y components share the X packing unless they state their own.
    let y = match blocks.pairs(&key("Y:Nodes"))? {
        None => Packed {
            coefficients: blocks
                .f64s(&key("Y:Coefficients"))?
                .ok_or_else(|| err!(BadHeader, "Local terms have no Y:Coefficients"))?,
            ..x.clone()
        },
        Some(_) => packed(blocks, &key("Y"), count, has_shape)?,
    };

    let mut terms = Vec::with_capacity(count);
    for i in 0..count {
        if !radii[i].is_finite() || radii[i] <= 0.0 {
            return Err(err!(
                BadHeader,
                "a Local term's support radius must be greater than zero, got {}",
                radii[i]
            ));
        }
        let xs = x.term(i, order, polynomial, count)?;
        let ys = y.term(i, order, polynomial, count)?;
        terms.push(LocalTerm {
            center: centers[i],
            radius: radii[i],
            spline: SplinePair { x: xs, y: ys },
        });
    }
    Ok(terms)
}

/// The packed arrays of one component across every Local term.
#[derive(Clone)]
struct Packed {
    normalizations: Vec<f64>,
    offsets: Vec<i32>,
    nodes: Vec<[f64; 2]>,
    coefficients: Vec<f64>,
    shapes: Vec<f64>,
}

impl Packed {
    /// The spline of term `i`, cut out of the packed arrays.
    fn term(&self, i: usize, order: i32, polynomial: bool, count: usize) -> Result<Spline> {
        // "the i-th Local term owns the node rows in the range
        // [offset_i, offset_{i+1})", with the last running to the end.
        let start = usize::try_from(self.offsets[i])
            .map_err(|_| err!(BadHeader, "a negative node offset"))?;
        let end = if i + 1 < count {
            usize::try_from(self.offsets[i + 1])
                .map_err(|_| err!(BadHeader, "a negative node offset"))?
        } else {
            self.nodes.len()
        };
        if start > end || end > self.nodes.len() {
            return Err(err!(
                BadHeader,
                "Local term {i} claims nodes {start}..{end} of {}",
                self.nodes.len()
            ));
        }

        let nodes = self.nodes[start..end].to_vec();
        // Coefficients are packed over the same offsets, but each term also
        // carries its own polynomial coefficients, so its slice is longer than
        // its node count by exactly Q.
        let q = polynomial_terms(order, polynomial);
        let c_start = start + i * q;
        let c_end = end + (i + 1) * q;
        if c_end > self.coefficients.len() {
            return Err(err!(
                BadHeader,
                "Local term {i} claims coefficients {c_start}..{c_end} of {}",
                self.coefficients.len()
            ));
        }

        if self.normalizations.len() < (i + 1) * 3 {
            return Err(err!(BadHeader, "Local term {i} has no normalization"));
        }
        Ok(Spline {
            normalization: [
                self.normalizations[i * 3],
                self.normalizations[i * 3 + 1],
                self.normalizations[i * 3 + 2],
            ],
            nodes,
            coefficients: self.coefficients[c_start..c_end].to_vec(),
            shape_parameter: self.shapes.get(i).copied(),
        })
    }
}

fn packed(blocks: &Blocks<'_>, prefix: &str, count: usize, has_shape: bool) -> Result<Packed> {
    let key = |leaf: &str| alloc::format!("{prefix}:{leaf}");
    let normalizations = blocks
        .f64s(&key("Normalization"))?
        .ok_or_else(|| err!(BadHeader, "Local terms have no Normalization"))?;
    let offsets = blocks
        .i32s(&key("NodeOffsets"))?
        .ok_or_else(|| err!(BadHeader, "Local terms have no NodeOffsets"))?;
    if offsets.len() != count {
        return Err(err!(
            BadHeader,
            "Local terms declare {count} terms but {} node offsets",
            offsets.len()
        ));
    }
    Ok(Packed {
        normalizations,
        offsets,
        nodes: blocks
            .pairs(&key("Nodes"))?
            .ok_or_else(|| err!(BadHeader, "Local terms have no Nodes"))?,
        coefficients: blocks
            .f64s(&key("Coefficients"))?
            .ok_or_else(|| err!(BadHeader, "Local terms have no Coefficients"))?,
        shapes: if has_shape {
            blocks.f64s(&key("ShapeParameter"))?.unwrap_or_default()
        } else {
            Vec::new()
        },
    })
}

/// A spline's coefficient vector must hold one value per node plus the
/// polynomial ones. A mismatch means the model would be evaluated with
/// coefficients belonging to something else.
fn check_length(
    spline: &Spline,
    order: i32,
    polynomial: bool,
    prefix: &str,
    component: &str,
) -> Result<()> {
    let expected = spline.expected_coefficients(order, polynomial);
    if spline.coefficients.len() != expected {
        return Err(err!(
            BadHeader,
            "{prefix}:{component} has {} nodes and needs {expected} coefficients, but has {}",
            spline.nodes.len(),
            spline.coefficients.len()
        ));
    }
    Ok(())
}
