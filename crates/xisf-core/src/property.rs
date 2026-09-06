//! XISF's property type system.
//!
//! A `<Property>` carries a typed value. Scalars and strings live in the
//! header itself; vectors, matrices and tables have data blocks, because a
//! matrix of a million doubles has no business being XML text.
//!
//! Two things about the type names are easy to get wrong, and both are in the
//! spec's own tables:
//!
//! - **Most types have two spellings.** `UInt8` is also `Byte`, `Int32` is
//!   also `Int`, `F64Matrix` is also `Matrix`. A reader that knows only the
//!   canonical name fails on perfectly valid files, so both are accepted and
//!   the canonical one is what gets written.
//! - **The type list runs wider than Rust's.** `Int128`, `UInt128`,
//!   `Float128` and the `Complex128` family are all in the spec. The 128-bit
//!   integers map to Rust's; `Float128` has no stable Rust type, so it is
//!   recognised and carried as raw bytes rather than silently narrowed to
//!   `f64`.

use crate::err;
use crate::error::Result;

/// The element type of a scalar, vector, matrix or table component.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scalar {
    Int8,
    UInt8,
    Int16,
    UInt16,
    Int32,
    UInt32,
    Int64,
    UInt64,
    Int128,
    UInt128,
    Float32,
    Float64,
    /// IEEE 754 binary128. No stable Rust type corresponds, so values are
    /// carried as their 16 bytes rather than converted.
    Float128,
    Complex32,
    Complex64,
    Complex128,
    Boolean,
}

impl Scalar {
    /// Size in bytes of one value, or `None` for `Boolean`, which the spec
    /// serialises as text rather than as a fixed-width datum.
    pub fn size(self) -> Option<usize> {
        Some(match self {
            Scalar::Int8 | Scalar::UInt8 => 1,
            Scalar::Int16 | Scalar::UInt16 => 2,
            Scalar::Int32 | Scalar::UInt32 | Scalar::Float32 => 4,
            Scalar::Int64 | Scalar::UInt64 | Scalar::Float64 | Scalar::Complex32 => 8,
            Scalar::Int128 | Scalar::UInt128 | Scalar::Float128 | Scalar::Complex64 => 16,
            Scalar::Complex128 => 32,
            Scalar::Boolean => return None,
        })
    }

    /// The canonical name, which is what an encoder writes.
    pub fn name(self) -> &'static str {
        match self {
            Scalar::Int8 => "Int8",
            Scalar::UInt8 => "UInt8",
            Scalar::Int16 => "Int16",
            Scalar::UInt16 => "UInt16",
            Scalar::Int32 => "Int32",
            Scalar::UInt32 => "UInt32",
            Scalar::Int64 => "Int64",
            Scalar::UInt64 => "UInt64",
            Scalar::Int128 => "Int128",
            Scalar::UInt128 => "UInt128",
            Scalar::Float32 => "Float32",
            Scalar::Float64 => "Float64",
            Scalar::Float128 => "Float128",
            Scalar::Complex32 => "Complex32",
            Scalar::Complex64 => "Complex64",
            Scalar::Complex128 => "Complex128",
            Scalar::Boolean => "Boolean",
        }
    }

    /// Parse a scalar type name, canonical or alternate.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "Int8" => Scalar::Int8,
            "UInt8" | "Byte" => Scalar::UInt8,
            "Int16" | "Short" => Scalar::Int16,
            "UInt16" | "UShort" => Scalar::UInt16,
            "Int32" | "Int" => Scalar::Int32,
            "UInt32" | "UInt" => Scalar::UInt32,
            "Int64" => Scalar::Int64,
            "UInt64" => Scalar::UInt64,
            "Int128" => Scalar::Int128,
            "UInt128" => Scalar::UInt128,
            "Float32" | "Float" => Scalar::Float32,
            "Float64" | "Double" => Scalar::Float64,
            "Float128" | "Quad" => Scalar::Float128,
            "Complex32" => Scalar::Complex32,
            "Complex64" | "Complex" => Scalar::Complex64,
            "Complex128" => Scalar::Complex128,
            "Boolean" | "Bool" => Scalar::Boolean,
            _ => return None,
        })
    }
}

/// What shape a property's value has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// A single value, in the `value` attribute.
    Scalar,
    /// UTF-8 text, in the element's character data.
    String,
    /// An ISO 8601 instant, in the `value` attribute.
    TimePoint,
    /// A one-dimensional array, in a data block, with a `length` attribute.
    Vector,
    /// A two-dimensional array, in a data block, with `rows` and `columns`.
    Matrix,
    /// A heterogeneous table.
    Table,
}

/// A property's declared type: its shape, and the element type where it has one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PropertyType {
    pub shape: Shape,
    /// `None` for `String`, `TimePoint` and `Table`, which have no single
    /// element type.
    pub element: Option<Scalar>,
}

impl PropertyType {
    /// Parse a `type` attribute.
    ///
    /// Vector and matrix names are built from an element prefix and a suffix
    /// -- `I32Vector`, `F64Matrix` -- so they are decomposed rather than
    /// listed one by one, which is also how the alternate spellings stay in
    /// one place.
    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "String" => return Ok(Self { shape: Shape::String, element: None }),
            "TimePoint" => return Ok(Self { shape: Shape::TimePoint, element: None }),
            "Table" => return Ok(Self { shape: Shape::Table, element: None }),
            _ => {}
        }

        if let Some(scalar) = Scalar::parse(name) {
            return Ok(Self { shape: Shape::Scalar, element: Some(scalar) });
        }

        // Aggregate spellings whose whole name is an alternate: `ByteArray`
        // is a UInt8 vector, `Vector` a Float64 one, `Matrix` a Float64
        // matrix, and the `IVector` family follows the scalar aliases.
        let aggregate = match name {
            "ByteArray" => Some((Shape::Vector, Scalar::UInt8)),
            "IVector" => Some((Shape::Vector, Scalar::Int32)),
            "UIVector" => Some((Shape::Vector, Scalar::UInt32)),
            "Vector" => Some((Shape::Vector, Scalar::Float64)),
            "Matrix" => Some((Shape::Matrix, Scalar::Float64)),
            _ => None,
        };
        if let Some((shape, element)) = aggregate {
            return Ok(Self { shape, element: Some(element) });
        }

        let (prefix, shape) = if let Some(prefix) = name.strip_suffix("Vector") {
            (prefix, Shape::Vector)
        } else if let Some(prefix) = name.strip_suffix("Matrix") {
            (prefix, Shape::Matrix)
        } else {
            return Err(err!(Unsupported, "unknown property type {name:?}"));
        };

        let element = element_from_prefix(prefix)
            .ok_or_else(|| err!(Unsupported, "unknown element prefix {prefix:?} in {name:?}"))?;
        Ok(Self { shape, element: Some(element) })
    }

    /// The canonical name for this type.
    pub fn name(&self) -> String {
        match (self.shape, self.element) {
            (Shape::String, _) => "String".into(),
            (Shape::TimePoint, _) => "TimePoint".into(),
            (Shape::Table, _) => "Table".into(),
            (Shape::Scalar, Some(scalar)) => scalar.name().into(),
            (Shape::Vector, Some(scalar)) => format!("{}Vector", prefix_for(scalar)),
            (Shape::Matrix, Some(scalar)) => format!("{}Matrix", prefix_for(scalar)),
            // Only reachable if an element type is dropped from an aggregate,
            // which the constructors do not allow.
            (shape, None) => format!("{shape:?}"),
        }
    }
}

/// The prefix an aggregate type name uses for an element type: `I32Vector`
/// carries `Int32`, `UI8Vector` carries `UInt8`.
fn element_from_prefix(prefix: &str) -> Option<Scalar> {
    Some(match prefix {
        "I8" => Scalar::Int8,
        "UI8" => Scalar::UInt8,
        "I16" => Scalar::Int16,
        "UI16" => Scalar::UInt16,
        "I32" => Scalar::Int32,
        "UI32" => Scalar::UInt32,
        "I64" => Scalar::Int64,
        "UI64" => Scalar::UInt64,
        "I128" => Scalar::Int128,
        "UI128" => Scalar::UInt128,
        "F32" => Scalar::Float32,
        "F64" => Scalar::Float64,
        "F128" => Scalar::Float128,
        "C32" => Scalar::Complex32,
        "C64" => Scalar::Complex64,
        "C128" => Scalar::Complex128,
        _ => return None,
    })
}

fn prefix_for(scalar: Scalar) -> &'static str {
    match scalar {
        Scalar::Int8 => "I8",
        Scalar::UInt8 => "UI8",
        Scalar::Int16 => "I16",
        Scalar::UInt16 => "UI16",
        Scalar::Int32 => "I32",
        Scalar::UInt32 => "UI32",
        Scalar::Int64 => "I64",
        Scalar::UInt64 => "UI64",
        Scalar::Int128 => "I128",
        Scalar::UInt128 => "UI128",
        Scalar::Float32 => "F32",
        Scalar::Float64 => "F64",
        Scalar::Float128 => "F128",
        Scalar::Complex32 => "C32",
        Scalar::Complex64 => "C64",
        Scalar::Complex128 => "C128",
        Scalar::Boolean => "Boolean",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_names_and_their_alternates_agree() {
        for (canonical, alternate) in [
            ("UInt8", "Byte"),
            ("Int16", "Short"),
            ("UInt16", "UShort"),
            ("Int32", "Int"),
            ("UInt32", "UInt"),
            ("Float32", "Float"),
            ("Float64", "Double"),
            ("Float128", "Quad"),
        ] {
            assert_eq!(
                Scalar::parse(canonical),
                Scalar::parse(alternate),
                "{canonical} and {alternate} name the same type"
            );
            assert_eq!(Scalar::parse(canonical).unwrap().name(), canonical);
        }
    }

    #[test]
    fn aggregate_names_decompose() {
        let v = PropertyType::parse("I32Vector").unwrap();
        assert_eq!(v.shape, Shape::Vector);
        assert_eq!(v.element, Some(Scalar::Int32));

        let m = PropertyType::parse("F64Matrix").unwrap();
        assert_eq!(m.shape, Shape::Matrix);
        assert_eq!(m.element, Some(Scalar::Float64));

        let c = PropertyType::parse("C128Vector").unwrap();
        assert_eq!(c.element, Some(Scalar::Complex128));
    }

    #[test]
    fn whole_name_aliases_resolve() {
        // These are alternates for an entire aggregate type, not prefixes.
        assert_eq!(
            PropertyType::parse("ByteArray").unwrap(),
            PropertyType::parse("UI8Vector").unwrap()
        );
        assert_eq!(
            PropertyType::parse("Vector").unwrap(),
            PropertyType::parse("F64Vector").unwrap()
        );
        assert_eq!(
            PropertyType::parse("Matrix").unwrap(),
            PropertyType::parse("F64Matrix").unwrap()
        );
        assert_eq!(
            PropertyType::parse("IVector").unwrap(),
            PropertyType::parse("I32Vector").unwrap()
        );
    }

    #[test]
    fn every_type_round_trips_through_its_canonical_name() {
        for name in [
            "Int8",
            "UInt8",
            "Int16",
            "UInt16",
            "Int32",
            "UInt32",
            "Int64",
            "UInt64",
            "Int128",
            "UInt128",
            "Float32",
            "Float64",
            "Float128",
            "Complex32",
            "Complex64",
            "Complex128",
            "Boolean",
            "String",
            "TimePoint",
            "Table",
            "I8Vector",
            "UI128Vector",
            "F32Vector",
            "C64Vector",
            "F64Matrix",
            "I16Matrix",
        ] {
            let parsed = PropertyType::parse(name).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(parsed.name(), name, "{name} did not round-trip");
            assert_eq!(PropertyType::parse(&parsed.name()).unwrap(), parsed);
        }
    }

    #[test]
    fn element_sizes_are_the_widths_the_spec_gives() {
        assert_eq!(Scalar::Int8.size(), Some(1));
        assert_eq!(Scalar::Float64.size(), Some(8));
        assert_eq!(Scalar::Complex32.size(), Some(8), "two Float32 components");
        assert_eq!(Scalar::Complex64.size(), Some(16), "two Float64 components");
        assert_eq!(Scalar::Complex128.size(), Some(32), "two Float128 components");
        assert_eq!(Scalar::Float128.size(), Some(16));
        assert_eq!(Scalar::Boolean.size(), None, "serialised as text, not a fixed width");
    }

    #[test]
    fn unknown_types_are_rejected_rather_than_guessed() {
        for name in ["", "Nope", "F99Vector", "Int32Vector", "VectorMatrix", "Float16"] {
            assert!(PropertyType::parse(name).is_err(), "{name:?} should not parse");
        }
    }
}
