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
