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

/// A `<Property>` element, parsed.
///
/// The value itself is deliberately *not* decoded here. A scalar lives in the
/// `value` attribute and a string in the character data, but a vector or
/// matrix lives in a data block that only a reader can fetch -- so this
/// records what the property is and where its value is, and leaves getting it
/// to whoever holds the file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Property {
    /// The `id` attribute: the property's name, possibly namespaced.
    pub id: String,
    pub kind: PropertyType,
    /// The `value` attribute, for scalar and `TimePoint` properties.
    pub value: Option<String>,
    /// Character data, for `String` properties held inline.
    pub text: Option<String>,
    /// A vector's component count, from the `length` attribute.
    pub length: Option<u64>,
    /// A matrix's dimensions, from `rows` and `columns`.
    pub rows: Option<u64>,
    pub columns: Option<u64>,
    /// The `format` specifier, which affects only how a value is *printed*.
    pub format: Option<String>,
    pub comment: Option<String>,
}

impl Property {
    /// Parse a `<Property>` element's attributes.
    pub fn parse(element: &crate::header::Element) -> Result<Self> {
        if element.name != "Property" {
            return Err(err!(InvalidArgument, "expected <Property>, got <{}>", element.name));
        }
        let id = element
            .attr("id")
            .ok_or_else(|| err!(BadHeader, "a <Property> has no id"))?
            .to_string();
        let kind = element
            .attr("type")
            .ok_or_else(|| err!(BadHeader, "<Property id={id:?}> has no type"))
            .and_then(PropertyType::parse)?;

        let number = |name: &str| -> Result<Option<u64>> {
            match element.attr(name) {
                None => Ok(None),
                Some(text) => text.trim().parse::<u64>().map(Some).map_err(|_| {
                    err!(BadAttribute, "a <Property> has a non-numeric {name}: {text:?}")
                }),
            }
        };

        let property = Property {
            id,
            kind,
            value: element.attr("value").map(str::to_owned),
            text: element.data.text.clone(),
            length: number("length")?,
            rows: number("rows")?,
            columns: number("columns")?,
            format: element.attr("format").map(str::to_owned),
            comment: element.attr("comment").map(str::to_owned),
        };
        property.check_shape()?;
        Ok(property)
    }

    /// How many components the value has, where that is knowable from the
    /// header alone.
    ///
    /// `None` for scalars and strings, whose length is not a separate
    /// attribute.
    pub fn component_count(&self) -> Option<u64> {
        match self.kind.shape {
            Shape::Vector => self.length,
            Shape::Matrix => self.rows.zip(self.columns).and_then(|(r, c)| r.checked_mul(c)),
            _ => None,
        }
    }

    /// The size the value's data block should be, in bytes.
    pub fn data_size(&self) -> Option<u64> {
        let count = self.component_count()?;
        let width = self.kind.element?.size()? as u64;
        count.checked_mul(width)
    }

    /// A vector must state its length and a matrix its dimensions; without
    /// them the block cannot be interpreted, and guessing from the block's
    /// size would silently accept a truncated file.
    fn check_shape(&self) -> Result<()> {
        match self.kind.shape {
            Shape::Vector if self.length.is_none() => {
                Err(err!(BadHeader, "<Property id={:?}> is a vector with no length", self.id))
            }
            Shape::Matrix if self.rows.is_none() || self.columns.is_none() => Err(err!(
                BadHeader,
                "<Property id={:?}> is a matrix without both rows and columns",
                self.id
            )),
            _ => Ok(()),
        }
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

    fn property_of(attrs: &str, text: &str) -> Result<Property> {
        let xml = format!(r#"<xisf version="1.0"><Property {attrs}>{text}</Property></xisf>"#);
        let header = crate::header::parse(&xml)?;
        Property::parse(&header.root.children[0])
    }

    #[test]
    fn a_scalar_property_carries_its_value_in_an_attribute() {
        let p = property_of(r#"id="FocalDistance" type="UInt32" value="2540""#, "").unwrap();
        assert_eq!(p.id, "FocalDistance");
        assert_eq!(p.kind.shape, Shape::Scalar);
        assert_eq!(p.kind.element, Some(Scalar::UInt32));
        assert_eq!(p.value.as_deref(), Some("2540"));
        assert_eq!(p.component_count(), None, "a scalar has no component count");
    }

    #[test]
    fn a_string_property_carries_its_value_as_character_data() {
        let p = property_of(r#"id="Instrument:Name" type="String""#, "SBIG STF-8300M").unwrap();
        assert_eq!(p.text.as_deref(), Some("SBIG STF-8300M"));
        assert_eq!(p.kind.shape, Shape::String);
    }

    #[test]
    fn a_vector_states_its_length_and_a_matrix_its_dimensions() {
        let v = property_of(r#"id="v" type="F64Vector" length="1000""#, "").unwrap();
        assert_eq!(v.component_count(), Some(1000));
        assert_eq!(v.data_size(), Some(8000));

        let m = property_of(r#"id="m" type="F32Matrix" rows="100" columns="25""#, "").unwrap();
        assert_eq!(m.component_count(), Some(2500));
        assert_eq!(m.data_size(), Some(10_000));
    }

    /// Without the shape attributes the block cannot be interpreted, and
    /// inferring it from the block's size would quietly accept a truncated
    /// file as a shorter vector.
    #[test]
    fn an_aggregate_without_its_shape_is_refused() {
        assert!(property_of(r#"id="v" type="F64Vector""#, "").is_err());
        assert!(property_of(r#"id="m" type="F32Matrix" rows="10""#, "").is_err());
        assert!(property_of(r#"id="m" type="F32Matrix" columns="10""#, "").is_err());
        assert!(property_of(r#"id="v" type="F64Vector" length="wat""#, "").is_err());
    }

    #[test]
    fn a_property_needs_an_id_and_a_type() {
        assert!(property_of(r#"type="UInt32" value="1""#, "").is_err());
        assert!(property_of(r#"id="x" value="1""#, "").is_err());
        assert!(property_of(r#"id="x" type="Nonsense""#, "").is_err());
    }

    /// A huge declared shape must report overflow rather than wrapping to a
    /// small size that then gets used as an allocation.
    #[test]
    fn an_absurd_shape_does_not_overflow() {
        let m = property_of(
            &format!(r#"id="m" type="F64Matrix" rows="{0}" columns="{0}""#, u64::MAX),
            "",
        )
        .unwrap();
        assert_eq!(m.component_count(), None);
        assert_eq!(m.data_size(), None);
    }

    #[test]
    fn unknown_types_are_rejected_rather_than_guessed() {
        for name in ["", "Nope", "F99Vector", "Int32Vector", "VectorMatrix", "Float16"] {
            assert!(PropertyType::parse(name).is_err(), "{name:?} should not parse");
        }
    }
}

/// A scalar property's value, decoded according to its declared type.
///
/// Kept as an enum rather than one number because the declared type decides
/// how the text is read: the specification's own example, `0x80E950AB`, is
/// 2162774187 as a `UInt32` and -2132193109 as an `Int32`, and nothing but
/// the `type` attribute says which was meant.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ScalarValue {
    Bool(bool),
    Signed(i128),
    Unsigned(u128),
    Float(f64),
    /// Real and imaginary parts.
    Complex(f64, f64),
}

impl ScalarValue {
    /// The value as an `f64`, for callers that want one number.
    ///
    /// Large 64- and 128-bit integers lose precision, as they would in any
    /// conversion to a double; a complex value yields its real part.
    pub fn as_f64(&self) -> f64 {
        match *self {
            ScalarValue::Bool(b) => f64::from(u8::from(b)),
            ScalarValue::Signed(v) => v as f64,
            ScalarValue::Unsigned(v) => v as f64,
            ScalarValue::Float(v) => v,
            ScalarValue::Complex(re, _) => re,
        }
    }
}

/// Parse an integer the way the specification serialises one.
///
/// Decimal is the ordinary case, but binary, octal and hexadecimal are all
/// legal with `0b`, `0o` and `0x` prefixes in either case -- and Rust's own
/// `from_str` rejects every one of those, so a caller who reached for
/// `.parse()` would fail on values this format explicitly permits.
///
/// The result is the bit pattern the digits denote. Whether it is read as
/// signed or unsigned is the declared type's business, not the literal's:
/// `0x80E950AB` denotes the same thirty-two bits either way.
fn parse_radix(text: &str) -> Option<(u128, bool)> {
    let text = text.trim();
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };

    let (radix, digits) = match digits.get(..2) {
        Some(prefix) if prefix.eq_ignore_ascii_case("0b") => (2, &digits[2..]),
        Some(prefix) if prefix.eq_ignore_ascii_case("0o") => (8, &digits[2..]),
        Some(prefix) if prefix.eq_ignore_ascii_case("0x") => (16, &digits[2..]),
        _ => (10, digits),
    };
    if digits.is_empty() {
        return None;
    }
    u128::from_str_radix(digits, radix).ok().map(|value| (value, negative))
}

/// Parse a floating point value the way the specification serialises one.
///
/// `NaN`, `+Inf` and `-Inf` are the spelled-out forms the spec names, and a
/// leading decimal point (`.123`) is legal. Rust's parser accepts all of
/// these, so this exists to be the one place the grammar is stated rather
/// than to correct it.
fn parse_float(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok()
}

impl Property {
    /// The property's value, decoded according to its declared type.
    ///
    /// `None` when the property has no scalar value in the header -- a
    /// string, a time point, or an aggregate whose value lives in a data
    /// block -- or when the text does not match the type it claims.
    pub fn value(&self) -> Option<ScalarValue> {
        if self.kind.shape != Shape::Scalar {
            return None;
        }
        let text = self.value.as_deref()?.trim();
        let scalar = self.kind.element?;

        Some(match scalar {
            // The spec allows either the words or the integers 0 and 1.
            Scalar::Boolean => match text {
                "true" | "True" | "TRUE" | "1" => ScalarValue::Bool(true),
                "false" | "False" | "FALSE" | "0" => ScalarValue::Bool(false),
                _ => return None,
            },

            Scalar::Float32 | Scalar::Float64 | Scalar::Float128 => {
                ScalarValue::Float(parse_float(text)?)
            }

            Scalar::Complex32 | Scalar::Complex64 | Scalar::Complex128 => {
                // `(re,im)`, as the spec's `(0.123,-0.735e-02)` example.
                let inner = text.strip_prefix('(')?.strip_suffix(')')?;
                let (re, im) = inner.split_once(',')?;
                ScalarValue::Complex(parse_float(re)?, parse_float(im)?)
            }

            Scalar::Int8 | Scalar::Int16 | Scalar::Int32 | Scalar::Int64 | Scalar::Int128 => {
                let (magnitude, negative) = parse_radix(text)?;
                let width = scalar.size()? * 8;
                let signed = if negative {
                    i128::try_from(magnitude).ok()?.checked_neg()?
                } else if width < 128 && magnitude >= (1u128 << (width - 1)) {
                    // A literal that fills the width, like the spec's own
                    // `0x80E950AB` as an Int32, denotes a negative number in
                    // two's complement rather than being out of range.
                    (magnitude as i128) - (1i128 << width)
                } else {
                    i128::try_from(magnitude).ok()?
                };
                ScalarValue::Signed(signed)
            }

            Scalar::UInt8 | Scalar::UInt16 | Scalar::UInt32 | Scalar::UInt64 | Scalar::UInt128 => {
                let (magnitude, negative) = parse_radix(text)?;
                if negative {
                    return None;
                }
                ScalarValue::Unsigned(magnitude)
            }
        })
    }
}
