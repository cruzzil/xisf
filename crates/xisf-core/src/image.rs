//! The `<Image>` core element: what the pixels are and how they are laid out.

use crate::err;
use crate::error::Result;
use crate::header::Element;

/// How pixel samples are stored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SampleFormat {
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float32,
    Float64,
    Complex32,
    Complex64,
}

impl SampleFormat {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "UInt8" => SampleFormat::UInt8,
            "UInt16" => SampleFormat::UInt16,
            "UInt32" => SampleFormat::UInt32,
            "UInt64" => SampleFormat::UInt64,
            "Float32" => SampleFormat::Float32,
            "Float64" => SampleFormat::Float64,
            "Complex32" => SampleFormat::Complex32,
            "Complex64" => SampleFormat::Complex64,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            SampleFormat::UInt8 => "UInt8",
            SampleFormat::UInt16 => "UInt16",
            SampleFormat::UInt32 => "UInt32",
            SampleFormat::UInt64 => "UInt64",
            SampleFormat::Float32 => "Float32",
            SampleFormat::Float64 => "Float64",
            SampleFormat::Complex32 => "Complex32",
            SampleFormat::Complex64 => "Complex64",
        }
    }

    /// Bytes per sample. A complex sample is two components wide.
    pub fn size(self) -> usize {
        match self {
            SampleFormat::UInt8 => 1,
            SampleFormat::UInt16 => 2,
            SampleFormat::UInt32 | SampleFormat::Float32 => 4,
            SampleFormat::UInt64 | SampleFormat::Float64 | SampleFormat::Complex32 => 8,
            SampleFormat::Complex64 => 16,
        }
    }

    /// Whether samples are floating point, which decides how `bounds` applies.
    pub fn is_float(self) -> bool {
        matches!(
            self,
            SampleFormat::Float32
                | SampleFormat::Float64
                | SampleFormat::Complex32
                | SampleFormat::Complex64
        )
    }
}

/// The colour space the channels are in.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ColorSpace {
    #[default]
    Gray,
    Rgb,
    CieLab,
}

impl ColorSpace {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "Gray" => ColorSpace::Gray,
            "RGB" => ColorSpace::Rgb,
            "CIELab" => ColorSpace::CieLab,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            ColorSpace::Gray => "Gray",
            ColorSpace::Rgb => "RGB",
            ColorSpace::CieLab => "CIELab",
        }
    }

    /// How many channels this space needs, before any alpha channels.
    pub fn nominal_channels(self) -> u64 {
        match self {
            ColorSpace::Gray => 1,
            ColorSpace::Rgb | ColorSpace::CieLab => 3,
        }
    }
}

/// How samples of different channels are interleaved.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PixelStorage {
    /// Each channel stored whole, one after another. The spec's default.
    #[default]
    Planar,
    /// Channels interleaved per pixel.
    Normal,
}

impl PixelStorage {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "Planar" => PixelStorage::Planar,
            "Normal" => PixelStorage::Normal,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            PixelStorage::Planar => "Planar",
            PixelStorage::Normal => "Normal",
        }
    }
}

/// The `bounds` attribute: the range floating-point samples are scaled to.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Bounds {
    pub low: f64,
    pub high: f64,
}

/// An `<Image>` element's attributes, parsed.
#[derive(Clone, PartialEq, Debug)]
pub struct Image {
    /// Dimensions, fastest-varying first, then the channel count.
    ///
    /// The spec writes `geometry="width:height:channels"` for a 2-D image,
    /// and allows more dimensions before the channel count.
    pub dimensions: Vec<u64>,
    pub channels: u64,
    pub sample_format: SampleFormat,
    pub color_space: ColorSpace,
    pub pixel_storage: PixelStorage,
    /// Present for floating-point images; the spec's default is 0:1.
    pub bounds: Option<Bounds>,
    pub id: Option<String>,
    pub uuid: Option<String>,
    pub image_type: Option<String>,
    /// A pedestal added to every sample, which must be subtracted to get
    /// zero-based values. The spec's default is zero.
    ///
    /// This is not cosmetic: calibration and integration subtract it, so an
    /// image read without it has the wrong zero point.
    pub offset: Option<f64>,
    /// How the image should be reoriented for display. `None` is the spec's
    /// default of no transformation.
    ///
    /// A decoder must *not* apply this for processing that depends on the
    /// physical layout of the pixels -- calibration frames would no longer
    /// line up -- so it is kept as a declaration rather than applied here.
    pub orientation: Option<Orientation>,
}

/// The `orientation` attribute: a rotation, and optionally a horizontal flip.
///
/// The rotation is counter-clockwise in degrees, and the flip is applied
/// after it, which is the order the spec's `90;flip` spelling implies.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Orientation {
    /// One of 0, 90, 180 or -90.
    pub rotation: i16,
    pub flip_horizontal: bool,
}

impl Orientation {
    /// Parse an `orientation` attribute.
    pub fn parse(text: &str) -> Result<Self> {
        let (rotation, flip_horizontal) = match text.strip_suffix(";flip") {
            Some(rest) => (rest, true),
            None => match text {
                "flip" => ("0", true),
                rest => (rest, false),
            },
        };
        let rotation = match rotation {
            "0" => 0,
            "90" => 90,
            "180" => 180,
            "-90" => -90,
            other => {
                return Err(err!(BadAttribute, "unknown orientation rotation {other:?}"));
            }
        };
        Ok(Orientation { rotation, flip_horizontal })
    }

    /// The attribute text for this orientation.
    pub fn to_attribute(self) -> String {
        match (self.rotation, self.flip_horizontal) {
            (0, false) => "0".into(),
            (0, true) => "flip".into(),
            (rotation, false) => rotation.to_string(),
            (rotation, true) => format!("{rotation};flip"),
        }
    }

    /// Whether this leaves the image as stored.
    pub fn is_identity(self) -> bool {
        self.rotation == 0 && !self.flip_horizontal
    }
}

impl Image {
    /// Parse an `<Image>` element's attributes.
    pub fn parse(element: &Element) -> Result<Self> {
        if element.name != "Image" {
            return Err(err!(InvalidArgument, "expected <Image>, got <{}>", element.name));
        }

        let geometry = element
            .attr("geometry")
            .ok_or_else(|| err!(BadHeader, "<Image> has no geometry attribute"))?;
        let (dimensions, channels) = parse_geometry(geometry)?;

        let sample_format = element
            .attr("sampleFormat")
            .ok_or_else(|| err!(BadHeader, "<Image> has no sampleFormat attribute"))?;
        let sample_format = SampleFormat::parse(sample_format)
            .ok_or_else(|| err!(Unsupported, "unknown sampleFormat {sample_format:?}"))?;

        // `colorSpace` and `pixelStorage` are optional and default per the
        // spec; an unrecognised *value* is still an error, since guessing
        // would misread the pixels.
        let color_space = match element.attr("colorSpace") {
            None => ColorSpace::default(),
            Some(name) => ColorSpace::parse(name)
                .ok_or_else(|| err!(Unsupported, "unknown colorSpace {name:?}"))?,
        };
        let pixel_storage = match element.attr("pixelStorage") {
            None => PixelStorage::default(),
            Some(name) => PixelStorage::parse(name)
                .ok_or_else(|| err!(Unsupported, "unknown pixelStorage {name:?}"))?,
        };

        let bounds = match element.attr("bounds") {
            None => None,
            Some(text) => Some(parse_bounds(text)?),
        };

        Ok(Image {
            dimensions,
            channels,
            sample_format,
            color_space,
            pixel_storage,
            bounds,
            id: element.attr("id").map(str::to_owned),
            uuid: element.attr("uuid").map(str::to_owned),
            image_type: element.attr("imageType").map(str::to_owned),
            offset: match element.attr("offset") {
                None => None,
                Some(text) => Some(parse_offset(text)?),
            },
            orientation: match element.attr("orientation") {
                None => None,
                Some(text) => Some(Orientation::parse(text)?),
            },
        })
    }

    /// Total number of samples: every dimension times the channel count.
    pub fn sample_count(&self) -> Option<u64> {
        self.dimensions
            .iter()
            .try_fold(1u64, |acc, d| acc.checked_mul(*d))?
            .checked_mul(self.channels)
    }

    /// The size the pixel data should be, in bytes.
    pub fn data_size(&self) -> Option<u64> {
        self.sample_count()?.checked_mul(self.sample_format.size() as u64)
    }

    /// Whether the channel count covers what the colour space needs.
    ///
    /// More channels than nominal is legal -- they are alpha channels -- but
    /// fewer means the image cannot be interpreted in the space it declares.
    pub fn channels_suffice(&self) -> bool {
        self.channels >= self.color_space.nominal_channels()
    }
}

/// Parse `geometry="d1:d2:...:channels"`.
///
/// The last field is always the channel count, and at least one dimension has
/// to precede it, so `"10"` alone is not a geometry.
fn parse_geometry(text: &str) -> Result<(Vec<u64>, u64)> {
    let fields: Vec<&str> = text.split(':').collect();
    if fields.len() < 2 {
        return Err(err!(
            BadAttribute,
            "geometry needs at least one dimension and a channel count, got {text:?}"
        ));
    }

    let mut values = Vec::with_capacity(fields.len());
    for field in &fields {
        let value = field
            .parse::<u64>()
            .map_err(|_| err!(BadAttribute, "geometry field {field:?} is not a decimal integer"))?;
        values.push(value);
    }

    let channels = values.pop().expect("checked non-empty above");
    if channels == 0 {
        return Err(err!(BadAttribute, "an image with zero channels has no pixels"));
    }
    if values.contains(&0) {
        return Err(err!(BadAttribute, "geometry {text:?} has a zero-length dimension"));
    }
    Ok((values, channels))
}

/// Parse `bounds="low:high"`.
fn parse_bounds(text: &str) -> Result<Bounds> {
    let (low, high) = text
        .split_once(':')
        .ok_or_else(|| err!(BadAttribute, "bounds needs low:high, got {text:?}"))?;
    let low = low
        .trim()
        .parse::<f64>()
        .map_err(|_| err!(BadAttribute, "bounds low {low:?} is not a number"))?;
    let high = high
        .trim()
        .parse::<f64>()
        .map_err(|_| err!(BadAttribute, "bounds high {high:?} is not a number"))?;
    if !(low.is_finite() && high.is_finite()) {
        return Err(err!(BadAttribute, "bounds must be finite, got {text:?}"));
    }
    if low >= high {
        return Err(err!(BadAttribute, "bounds low must be below high, got {text:?}"));
    }
    Ok(Bounds { low, high })
}

/// The unit an image's resolution is measured in.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ResolutionUnit {
    /// Pixels per inch, and the spec's default when no `unit` is given.
    #[default]
    Inch,
    /// Pixels per centimetre.
    Centimetre,
}

impl ResolutionUnit {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "inch" => ResolutionUnit::Inch,
            "cm" => ResolutionUnit::Centimetre,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            ResolutionUnit::Inch => "inch",
            ResolutionUnit::Centimetre => "cm",
        }
    }
}

/// A `<Resolution>` element: how many pixels there are per unit of length.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Resolution {
    pub horizontal: f64,
    pub vertical: f64,
    pub unit: ResolutionUnit,
}

impl Resolution {
    /// Parse a `<Resolution>` element.
    pub fn parse(element: &Element) -> Result<Self> {
        if element.name != "Resolution" {
            return Err(err!(InvalidArgument, "expected <Resolution>, got <{}>", element.name));
        }
        let number = |name: &str| -> Result<f64> {
            let text =
                element.attr(name).ok_or_else(|| err!(BadHeader, "<Resolution> has no {name}"))?;
            let value = text
                .trim()
                .parse::<f64>()
                .map_err(|_| err!(BadAttribute, "<Resolution> {name}={text:?} is not a number"))?;
            if !value.is_finite() || value <= 0.0 {
                return Err(err!(
                    BadAttribute,
                    "<Resolution> {name} must be positive, got {value}"
                ));
            }
            Ok(value)
        };

        let unit = match element.attr("unit") {
            None => ResolutionUnit::default(),
            Some(name) => ResolutionUnit::parse(name)
                .ok_or_else(|| err!(Unsupported, "unknown resolution unit {name:?}"))?,
        };
        Ok(Resolution { horizontal: number("horizontal")?, vertical: number("vertical")?, unit })
    }

    /// The resolution in pixels per inch, whatever unit it was written in.
    pub fn per_inch(&self) -> (f64, f64) {
        match self.unit {
            ResolutionUnit::Inch => (self.horizontal, self.vertical),
            ResolutionUnit::Centimetre => (self.horizontal * 2.54, self.vertical * 2.54),
        }
    }
}

/// Parse an `offset` attribute: a pedestal, which cannot be negative.
fn parse_offset(text: &str) -> Result<f64> {
    let value = text
        .trim()
        .parse::<f64>()
        .map_err(|_| err!(BadAttribute, "<Image> offset={text:?} is not a number"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(err!(BadAttribute, "<Image> offset must be zero or positive, got {value}"));
    }
    Ok(value)
}

/// Split a colon-separated list of `n` floating point numbers.
fn colon_numbers<const N: usize>(element: &str, name: &str, text: &str) -> Result<[f64; N]> {
    let mut out = [0.0; N];
    let mut parts = text.split(':');
    for slot in out.iter_mut() {
        let part = parts
            .next()
            .ok_or_else(|| err!(BadAttribute, "<{element}> {name}={text:?} needs {N} values"))?;
        *slot = part.trim().parse::<f64>().map_err(|_| {
            err!(BadAttribute, "<{element}> {name}={text:?} has a non-numeric component")
        })?;
        if !slot.is_finite() {
            return Err(err!(BadAttribute, "<{element}> {name}={text:?} is not finite"));
        }
    }
    if parts.next().is_some() {
        return Err(err!(BadAttribute, "<{element}> {name}={text:?} has more than {N} values"));
    }
    Ok(out)
}

/// The transfer curve of an RGB working space.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Gamma {
    /// A fixed exponent, always greater than zero.
    Exponent(f64),
    /// The sRGB piecewise transfer function, which is not a pure exponent.
    Srgb,
}

/// An `<RGBWorkingSpace>` element: the colour space an image's RGB values
/// are expressed in.
///
/// Every triple is in red, green, blue order. Absent, the spec's default is
/// sRGB, which is what [`RgbWorkingSpace::srgb`] returns -- so a caller can
/// treat "no element" and "sRGB" alike without special-casing either.
#[derive(Clone, PartialEq, Debug)]
pub struct RgbWorkingSpace {
    pub gamma: Gamma,
    /// Chromaticity x of the three primaries.
    pub x: [f64; 3],
    /// Chromaticity y of the three primaries.
    pub y: [f64; 3],
    /// Luminance coefficients of the three primaries.
    pub luminance: [f64; 3],
    pub name: Option<String>,
}

impl RgbWorkingSpace {
    /// The sRGB space, which applies when an image declares none.
    pub fn srgb() -> Self {
        RgbWorkingSpace {
            gamma: Gamma::Srgb,
            x: [0.648431, 0.321152, 0.155886],
            y: [0.330856, 0.597871, 0.066044],
            luminance: [0.222491, 0.716888, 0.060621],
            name: Some("sRGB IEC61966-2.1".into()),
        }
    }

    /// Parse an `<RGBWorkingSpace>` element.
    pub fn parse(element: &Element) -> Result<Self> {
        if element.name != "RGBWorkingSpace" {
            return Err(err!(
                InvalidArgument,
                "expected <RGBWorkingSpace>, got <{}>",
                element.name
            ));
        }
        let triple = |name: &str| -> Result<[f64; 3]> {
            let text = element
                .attr(name)
                .ok_or_else(|| err!(BadHeader, "<RGBWorkingSpace> has no {name}"))?;
            colon_numbers::<3>("RGBWorkingSpace", name, text)
        };

        let text = element
            .attr("gamma")
            .ok_or_else(|| err!(BadHeader, "<RGBWorkingSpace> has no gamma"))?;
        let gamma = if text.trim().eq_ignore_ascii_case("sRGB") {
            Gamma::Srgb
        } else {
            let exponent = text
                .trim()
                .parse::<f64>()
                .map_err(|_| err!(BadAttribute, "gamma={text:?} is neither sRGB nor a number"))?;
            if !exponent.is_finite() || exponent <= 0.0 {
                return Err(err!(BadAttribute, "gamma must be positive, got {exponent}"));
            }
            Gamma::Exponent(exponent)
        };

        Ok(RgbWorkingSpace {
            gamma,
            x: triple("x")?,
            y: triple("y")?,
            luminance: triple("Y")?,
            name: element.attr("name").map(str::to_owned),
        })
    }
}

/// A `<DisplayFunction>` element: a screen transfer function, which changes
/// how an image is *shown* without changing the data.
///
/// Each parameter has four components, for the red (or grey), green, blue and
/// lightness channels in that order. Absent, the identity function applies,
/// which is what [`DisplayFunction::identity`] returns.
#[derive(Clone, PartialEq, Debug)]
pub struct DisplayFunction {
    /// Midtones balance.
    pub midtones: [f64; 4],
    /// Shadows clipping point.
    pub shadows: [f64; 4],
    /// Highlights clipping point.
    pub highlights: [f64; 4],
    /// Shadows dynamic range expansion.
    pub low_range: [f64; 4],
    /// Highlights dynamic range expansion.
    pub high_range: [f64; 4],
    pub name: Option<String>,
}

impl DisplayFunction {
    /// The identity function, which applies when an image declares none.
    pub fn identity() -> Self {
        DisplayFunction {
            midtones: [0.5; 4],
            shadows: [0.0; 4],
            highlights: [1.0; 4],
            low_range: [0.0; 4],
            high_range: [1.0; 4],
            name: None,
        }
    }

    /// Whether this is the identity, and so can be skipped when displaying.
    pub fn is_identity(&self) -> bool {
        *self == DisplayFunction { name: self.name.clone(), ..DisplayFunction::identity() }
    }

    /// Parse a `<DisplayFunction>` element.
    pub fn parse(element: &Element) -> Result<Self> {
        if element.name != "DisplayFunction" {
            return Err(err!(
                InvalidArgument,
                "expected <DisplayFunction>, got <{}>",
                element.name
            ));
        }
        let quad = |name: &str| -> Result<[f64; 4]> {
            let text = element
                .attr(name)
                .ok_or_else(|| err!(BadHeader, "<DisplayFunction> has no {name}"))?;
            colon_numbers::<4>("DisplayFunction", name, text)
        };
        Ok(DisplayFunction {
            midtones: quad("m")?,
            shadows: quad("s")?,
            highlights: quad("h")?,
            low_range: quad("l")?,
            high_range: quad("r")?,
            name: element.attr("name").map(str::to_owned),
        })
    }
}

/// One element of a colour filter array pattern.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CfaElement {
    /// `0`: a nonexistent or undefined element.
    Undefined,
    Red,
    Green,
    Blue,
    /// White or panchromatic.
    White,
    Cyan,
    Magenta,
    Yellow,
}

impl CfaElement {
    fn parse(c: char) -> Option<Self> {
        Some(match c {
            '0' => CfaElement::Undefined,
            'R' => CfaElement::Red,
            'G' => CfaElement::Green,
            'B' => CfaElement::Blue,
            'W' => CfaElement::White,
            'C' => CfaElement::Cyan,
            'M' => CfaElement::Magenta,
            'Y' => CfaElement::Yellow,
            _ => return None,
        })
    }

    /// The character the specification writes this element as.
    pub fn as_char(self) -> char {
        match self {
            CfaElement::Undefined => '0',
            CfaElement::Red => 'R',
            CfaElement::Green => 'G',
            CfaElement::Blue => 'B',
            CfaElement::White => 'W',
            CfaElement::Cyan => 'C',
            CfaElement::Magenta => 'M',
            CfaElement::Yellow => 'Y',
        }
    }
}

/// A `<ColorFilterArray>`: the mosaic pattern a sensor captured through.
///
/// A Bayer filter is the familiar case, but the format admits any rectangular
/// pattern over eight element kinds. The pattern is ordered as it lies on the
/// image, left to right then top to bottom, so `pattern[y * width + x]` is the
/// filter over pixel `(x, y)` modulo the matrix size.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ColorFilterArray {
    pub pattern: Vec<CfaElement>,
    pub width: u64,
    pub height: u64,
    /// An optional human-readable name, e.g. "RGGB".
    pub name: Option<String>,
}

impl ColorFilterArray {
    /// Parse a `<ColorFilterArray>` element.
    pub fn parse(element: &Element) -> Result<Self> {
        if element.name != "ColorFilterArray" {
            return Err(err!(
                InvalidArgument,
                "expected <ColorFilterArray>, got <{}>",
                element.name
            ));
        }

        let text = element
            .attr("pattern")
            .ok_or_else(|| err!(BadHeader, "<ColorFilterArray> has no pattern"))?;
        let pattern = text
            .trim()
            .chars()
            .map(|c| {
                CfaElement::parse(c)
                    .ok_or_else(|| err!(BadAttribute, "{c:?} is not a CFA pattern element"))
            })
            .collect::<Result<Vec<_>>>()?;

        let dimension = |name: &str| -> Result<u64> {
            let text = element
                .attr(name)
                .ok_or_else(|| err!(BadHeader, "<ColorFilterArray> has no {name}"))?;
            let value = text.trim().parse::<u64>().map_err(|_| {
                err!(BadAttribute, "<ColorFilterArray> {name}={text:?} is not an integer")
            })?;
            if value == 0 {
                return Err(err!(BadAttribute, "<ColorFilterArray> {name} must be above zero"));
            }
            Ok(value)
        };
        let width = dimension("width")?;
        let height = dimension("height")?;

        // The pattern length is the matrix, so a mismatch means the two
        // disagree about the sensor -- and a decoder that trusted the
        // dimensions would demosaic with a pattern shifted by however much.
        let expected = width
            .checked_mul(height)
            .ok_or_else(|| err!(BadAttribute, "<ColorFilterArray> dimensions overflow"))?;
        if pattern.len() as u64 != expected {
            return Err(err!(
                BadAttribute,
                "<ColorFilterArray> is {width}x{height} but its pattern has {} elements",
                pattern.len()
            ));
        }

        Ok(ColorFilterArray {
            pattern,
            width,
            height,
            name: element.attr("name").map(str::to_owned),
        })
    }

    /// The filter over pixel `(x, y)`, which repeats across the image.
    pub fn element_at(&self, x: u64, y: u64) -> CfaElement {
        let index = (y % self.height) * self.width + (x % self.width);
        self.pattern[index as usize]
    }

    /// The pattern as the specification writes it.
    pub fn pattern_string(&self) -> String {
        self.pattern.iter().map(|e| e.as_char()).collect()
    }
}

/// A `<Thumbnail>`: a small preview, which is an image in its own right.
///
/// Parsed with the same code as an `<Image>`, because it *is* one -- same
/// geometry, sample format, colour space and data block. Only the element
/// name and its role differ.
pub fn parse_thumbnail(element: &Element) -> Result<Image> {
    if element.name != "Thumbnail" {
        return Err(err!(InvalidArgument, "expected <Thumbnail>, got <{}>", element.name));
    }
    // `Image::parse` checks the element name, so it is parsed from a copy
    // renamed to match rather than by duplicating every attribute rule.
    let mut as_image = element.clone();
    as_image.name = "Image".to_string();
    Image::parse(&as_image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header;

    fn image_of(attrs: &str) -> Result<Image> {
        let xml = format!(r#"<xisf version="1.0"><Image {attrs}/></xisf>"#);
        let parsed = header::parse(&xml)?;
        Image::parse(parsed.images()[0])
    }

    #[test]
    fn parses_a_typical_image() {
        let image =
            image_of(r#"geometry="256:256:3" sampleFormat="Float32" colorSpace="RGB""#).unwrap();
        assert_eq!(image.dimensions, vec![256, 256]);
        assert_eq!(image.channels, 3);
        assert_eq!(image.sample_format, SampleFormat::Float32);
        assert_eq!(image.color_space, ColorSpace::Rgb);
        assert_eq!(image.data_size(), Some(256 * 256 * 3 * 4));
        assert!(image.channels_suffice());
    }

    #[test]
    fn omitted_attributes_take_the_specs_defaults() {
        let image = image_of(r#"geometry="4:4:1" sampleFormat="UInt8""#).unwrap();
        assert_eq!(image.color_space, ColorSpace::Gray);
        assert_eq!(image.pixel_storage, PixelStorage::Planar);
        assert_eq!(image.bounds, None);
    }

    #[test]
    fn more_than_two_dimensions_are_allowed() {
        let image = image_of(r#"geometry="8:8:8:2" sampleFormat="UInt16""#).unwrap();
        assert_eq!(image.dimensions, vec![8, 8, 8]);
        assert_eq!(image.channels, 2);
        assert_eq!(image.data_size(), Some(8 * 8 * 8 * 2 * 2));
    }

    #[test]
    fn bounds_parse_and_are_ordered() {
        let image = image_of(r#"geometry="2:2:1" sampleFormat="Float32" bounds="0:1""#).unwrap();
        assert_eq!(image.bounds, Some(Bounds { low: 0.0, high: 1.0 }));

        for bad in ["1:0", "0:0", "nope", "0", "0:inf"] {
            let attrs = format!(r#"geometry="2:2:1" sampleFormat="Float32" bounds="{bad}""#);
            assert!(image_of(&attrs).is_err(), "bounds={bad:?} should be refused");
        }
    }

    /// Zero dimensions and unknown enum values are refused rather than
    /// defaulted, because both would silently misread the pixel data.
    #[test]
    fn resolution_defaults_to_pixels_per_inch() {
        let xml = r#"<xisf version="1.0"><Resolution horizontal="120" vertical="96"/></xisf>"#;
        let header = header::parse(xml).unwrap();
        let r = Resolution::parse(&header.root.children[0]).unwrap();
        assert_eq!(r.unit, ResolutionUnit::Inch);
        assert_eq!(r.per_inch(), (120.0, 96.0));
    }

    #[test]
    fn centimetre_resolutions_convert() {
        let xml =
            r#"<xisf version="1.0"><Resolution horizontal="100" vertical="100" unit="cm"/></xisf>"#;
        let header = header::parse(xml).unwrap();
        let r = Resolution::parse(&header.root.children[0]).unwrap();
        assert_eq!(r.unit, ResolutionUnit::Centimetre);
        assert_eq!(r.per_inch(), (254.0, 254.0));
    }

    #[test]
    fn a_nonsensical_resolution_is_refused() {
        for attrs in [
            r#"horizontal="0" vertical="1""#,
            r#"horizontal="-5" vertical="1""#,
            r#"horizontal="1""#,
            r#"horizontal="x" vertical="1""#,
            r#"horizontal="1" vertical="1" unit="furlong""#,
        ] {
            let xml = format!(r#"<xisf version="1.0"><Resolution {attrs}/></xisf>"#);
            let header = header::parse(&xml).unwrap();
            assert!(
                Resolution::parse(&header.root.children[0]).is_err(),
                "{attrs} should have been refused"
            );
        }
    }

    /// A thumbnail is an image, and is parsed by the same rules rather than a
    /// second implementation of them.
    #[test]
    fn a_thumbnail_parses_as_an_image() {
        let xml = r#"<xisf version="1.0"><Thumbnail geometry="400:300:3" sampleFormat="UInt8"
                     colorSpace="RGB" location="attachment:8192:360000"/></xisf>"#;
        let header = header::parse(xml).unwrap();
        let thumbnail = parse_thumbnail(&header.root.children[0]).unwrap();
        assert_eq!(thumbnail.dimensions, vec![400, 300]);
        assert_eq!(thumbnail.channels, 3);
        assert_eq!(thumbnail.sample_format, SampleFormat::UInt8);
        assert_eq!(thumbnail.data_size(), Some(400 * 300 * 3));
    }

    fn cfa_of(attrs: &str) -> Result<ColorFilterArray> {
        let xml = format!(r#"<xisf version="1.0"><ColorFilterArray {attrs}/></xisf>"#);
        let header = header::parse(&xml)?;
        ColorFilterArray::parse(&header.root.children[0])
    }

    #[test]
    fn a_bayer_pattern_parses_and_repeats() {
        let cfa = cfa_of(r#"pattern="RGGB" width="2" height="2" name="RGGB""#).unwrap();
        assert_eq!(cfa.pattern_string(), "RGGB");
        assert_eq!(cfa.name.as_deref(), Some("RGGB"));

        // Ordered left to right, then top to bottom.
        assert_eq!(cfa.element_at(0, 0), CfaElement::Red);
        assert_eq!(cfa.element_at(1, 0), CfaElement::Green);
        assert_eq!(cfa.element_at(0, 1), CfaElement::Green);
        assert_eq!(cfa.element_at(1, 1), CfaElement::Blue);

        // And it tiles across the sensor.
        assert_eq!(cfa.element_at(2, 2), CfaElement::Red);
        assert_eq!(cfa.element_at(101, 100), CfaElement::Green);
    }

    #[test]
    fn every_pattern_element_the_spec_names_is_understood() {
        let cfa = cfa_of(r#"pattern="0RGBWCMY" width="8" height="1""#).unwrap();
        assert_eq!(cfa.pattern[0], CfaElement::Undefined);
        assert_eq!(cfa.pattern[4], CfaElement::White);
        assert_eq!(cfa.pattern[7], CfaElement::Yellow);
        assert_eq!(cfa.pattern_string(), "0RGBWCMY");
    }

    /// A pattern that does not fill its declared matrix means the two
    /// disagree about the sensor, and demosaicing on the dimensions alone
    /// would use a pattern shifted by however much.
    #[test]
    fn a_pattern_that_does_not_match_its_dimensions_is_refused() {
        assert!(cfa_of(r#"pattern="RGGB" width="3" height="2""#).is_err());
        assert!(cfa_of(r#"pattern="RGG" width="2" height="2""#).is_err());
        assert!(cfa_of(r#"pattern="RGGB" width="0" height="2""#).is_err());
        assert!(cfa_of(r#"pattern="RGXB" width="2" height="2""#).is_err(), "X is not an element");
        assert!(cfa_of(r#"width="2" height="2""#).is_err());
        assert!(cfa_of(r#"pattern="RGGB" height="2""#).is_err());
    }

    #[test]
    fn nonsense_geometry_and_unknown_values_are_refused() {
        for attrs in [
            r#"geometry="0:0:1" sampleFormat="UInt8""#,
            r#"geometry="10:0:1" sampleFormat="UInt8""#,
            r#"geometry="10" sampleFormat="UInt8""#,
            r#"geometry="10:10:0" sampleFormat="UInt8""#,
            r#"geometry="a:b:c" sampleFormat="UInt8""#,
            r#"geometry="4:4:1" sampleFormat="Float16""#,
            r#"geometry="4:4:1" sampleFormat="UInt8" colorSpace="CMYK""#,
            r#"geometry="4:4:1" sampleFormat="UInt8" pixelStorage="Sideways""#,
            r#"sampleFormat="UInt8""#,
            r#"geometry="4:4:1""#,
        ] {
            assert!(image_of(attrs).is_err(), "{attrs} should have been refused");
        }
    }

    #[test]
    fn an_rgb_image_needs_three_channels() {
        let short = image_of(r#"geometry="4:4:1" sampleFormat="UInt8" colorSpace="RGB""#).unwrap();
        assert!(!short.channels_suffice());
        // A fourth channel is an alpha channel, which is allowed.
        let alpha = image_of(r#"geometry="4:4:4" sampleFormat="UInt8" colorSpace="RGB""#).unwrap();
        assert!(alpha.channels_suffice());
    }

    #[test]
    fn an_absurd_geometry_does_not_overflow() {
        let image =
            image_of(&format!(r#"geometry="{0}:{0}:{0}" sampleFormat="Float64""#, u64::MAX))
                .unwrap();
        assert_eq!(image.sample_count(), None, "should report overflow, not wrap");
        assert_eq!(image.data_size(), None);
    }
}
