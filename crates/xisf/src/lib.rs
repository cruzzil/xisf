//! Reading and writing XISF files.
//!
//! XISF is the Extensible Image Serialization Format: an XML header naming
//! images and properties, followed by the binary blocks that hold them.
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use xisf::XisfFile;
//!
//! let file = XisfFile::open("image.xisf")?;
//! for image in file.images() {
//!     println!("{:?} {}", image.geometry(), image.sample_format().name());
//!     let pixels: Vec<u16> = image.read()?;
//!     println!("  {} samples", pixels.len());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! This crate is the ergonomic face of [`xisf_core`], which does the work. It
//! is written from the published XISF specification; nothing here derives from
//! libXISF or from the PixInsight Class Library.

#![warn(missing_docs)]

// `alloc` is not linked automatically even in a crate that has `std`, so it
// is named here to make `alloc::` paths resolve. Reaching for the narrowest
// crate that has an item -- `core` where no allocator is needed, `alloc`
// where one is but an operating system is not -- keeps the door open to a
// `no_std` build and marks which parts genuinely need the platform.
extern crate alloc;

use alloc::borrow::Cow;
use std::path::Path;

// Everything a caller can be handed by this crate's own API must be nameable
// through this crate. A method returning a type only `xisf-core` exports
// forces every consumer to depend on the engine directly, which defeats the
// point of a facade -- and it is not hypothetical: `xisftool` did exactly
// that, because `verify()` returns a `ChecksumStatus` that was not re-exported
// and so could not be matched on.
pub use xisf_core::Reader;
pub use xisf_core::block::{ByteOrder, ChecksumAlgorithm, Codec, Compression, Location};
pub use xisf_core::error::{Error, ErrorKind, Result};
pub use xisf_core::header::DataRef;
pub use xisf_core::header::{Element, Header};
pub use xisf_core::image::{
    Bounds, CfaElement, ColorFilterArray, ColorSpace, DisplayFunction, Gamma, Image, Orientation,
    PixelStorage, Resolution, ResolutionUnit, RgbWorkingSpace, SampleFormat,
};
pub use xisf_core::property::{Property, PropertyType, Scalar, ScalarValue, Shape};
pub use xisf_core::reader::ChecksumStatus;
pub use xisf_core::table::{Cell, Field, Structure, Table};
pub use xisf_core::writer::{
    BlockOptions, Codec2 as WriteCodec, CompressionRequest, DistributedUnit, FitsKeyword,
    PendingImage, PendingThumbnail, Writer,
};

/// An open XISF file.
#[derive(Debug)]
pub struct XisfFile {
    reader: Reader,
}

impl XisfFile {
    /// Open a file from disk. Its data blocks are memory-mapped, so nothing
    /// is read until it is asked for.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self { reader: Reader::open(path)? })
    }

    /// Read a file already in memory.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        Ok(Self { reader: Reader::from_bytes(bytes)? })
    }

    /// The images the file holds, in the order the header names them.
    pub fn images(&self) -> Vec<ImageRef<'_>> {
        self.reader
            .header()
            .images()
            .into_iter()
            .filter_map(|element| {
                Image::parse(element).ok().map(|image| ImageRef { file: self, element, image })
            })
            .collect()
    }

    /// The file's properties, wherever in the header they appear.
    ///
    /// Includes those under `<Metadata>`, those attached to an image, and any
    /// standalone ones, because the distinction matters to a writer far more
    /// than to a caller asking what a file contains.
    pub fn properties(&self) -> Vec<PropertyRef<'_>> {
        self.reader
            .header()
            .root
            .descendants()
            .into_iter()
            .filter(|e| e.name == "Property")
            .filter_map(|element| {
                Property::parse(element).ok().map(|property| PropertyRef {
                    file: self,
                    element,
                    property,
                })
            })
            .collect()
    }

    /// The file's table properties, wherever in the header they appear.
    ///
    /// A table is a property whose value cannot be written as a `<Property>`
    /// element, so it is reached separately rather than through
    /// [`XisfFile::properties`].
    pub fn tables(&self) -> Vec<Table> {
        let header = self.reader.header();
        header
            .root
            .descendants()
            .into_iter()
            .filter(|e| e.name == "Table")
            .filter_map(|element| Table::parse(element, header).ok())
            .collect()
    }

    /// The table property with a given identifier, if the file has one.
    pub fn table(&self, id: &str) -> Option<Table> {
        self.tables().into_iter().find(|t| t.id == id)
    }

    /// The property with a given identifier, if the file has one.
    pub fn property(&self, id: &str) -> Option<PropertyRef<'_>> {
        self.properties().into_iter().find(|p| p.id() == id)
    }

    /// The file's properties as plain text, for the ones that have a textual
    /// form. A vector or matrix has none and is skipped.
    pub fn metadata(&self) -> Vec<(String, String)> {
        self.reader
            .header()
            .root
            .descendants()
            .into_iter()
            .filter(|e| e.name == "Property")
            .filter_map(|e| {
                let id = e.attr("id")?.to_string();
                let value = e.attr("value").map(str::to_owned).or_else(|| e.data.text.clone())?;
                Some((id, value))
            })
            .collect()
    }

    /// The underlying engine reader, for anything this API does not cover.
    pub fn reader(&self) -> &Reader {
        &self.reader
    }
}

/// One image in a file.
#[derive(Debug)]
pub struct ImageRef<'a> {
    file: &'a XisfFile,
    element: &'a Element,
    image: Image,
}

impl<'a> ImageRef<'a> {
    /// The image's dimensions, fastest-varying first, without the channel
    /// count.
    pub fn geometry(&self) -> &[u64] {
        &self.image.dimensions
    }

    /// How many channels the image has.
    pub fn channels(&self) -> u64 {
        self.image.channels
    }

    /// The type of one sample.
    pub fn sample_format(&self) -> SampleFormat {
        self.image.sample_format
    }

    /// The colour space the channels are in.
    pub fn color_space(&self) -> ColorSpace {
        self.image.color_space
    }

    /// How channels are interleaved.
    pub fn pixel_storage(&self) -> PixelStorage {
        self.image.pixel_storage
    }

    /// The range floating-point samples are scaled to, if declared.
    pub fn bounds(&self) -> Option<Bounds> {
        self.image.bounds
    }

    /// The parsed `<Image>` attributes.
    pub fn attributes(&self) -> &Image {
        &self.image
    }

    /// The byte order the samples are stored in.
    pub fn byte_order(&self) -> ByteOrder {
        self.element.data.byte_order
    }

    /// Whether the pixel data is compressed, and how.
    pub fn is_compressed(&self) -> bool {
        self.element.data.compression.is_some()
    }

    /// The image's pixel data, decompressed, in the order it was stored.
    ///
    /// Borrowed straight from the mapping when the block is attached and
    /// uncompressed, so this costs nothing in the common case.
    pub fn bytes(&self) -> Result<Cow<'a, [u8]>> {
        self.file.reader.block(&self.element.data)
    }

    /// Check the recorded checksum, if the image has one.
    pub fn verify(&self) -> Result<ChecksumStatus> {
        self.file.reader.verify(&self.element.data)
    }

    /// The image's FITS keywords, as `(name, value, comment)`.
    pub fn fits_keywords(&self) -> Vec<(String, String, String)> {
        self.element
            .children_named("FITSKeyword")
            .map(|k| {
                (
                    k.attr("name").unwrap_or_default().to_string(),
                    k.attr("value").unwrap_or_default().to_string(),
                    k.attr("comment").unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    /// Read the pixel data as `T`.
    ///
    /// `T` must be the image's own sample type; this converts byte order, not
    /// sample formats. Ask a `Float32` image for `u16` and it says so rather
    /// than quietly reinterpreting the bytes.
    ///
    /// When the stored order already matches the machine's, the samples are
    /// bulk-copied rather than decoded one at a time -- for a large image
    /// that is the difference between a memory copy and a loop over tens of
    /// millions of values.
    pub fn read<T: Sample>(&self) -> Result<Vec<T>> {
        if self.image.sample_format != T::FORMAT {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                format!(
                    "this image holds {} samples; asked for {}",
                    self.image.sample_format.name(),
                    T::FORMAT.name()
                ),
            ));
        }

        let bytes = self.bytes()?;
        let expected = self.image.data_size().ok_or_else(|| {
            Error::new(ErrorKind::Unsupported, "the image's geometry overflows this platform")
        })?;
        if bytes.len() as u64 != expected {
            return Err(Error::new(
                ErrorKind::Truncated,
                format!("the image declares {expected} bytes of pixel data, found {}", bytes.len()),
            ));
        }

        Ok(T::decode(&bytes, self.byte_order()))
    }

    /// The image's samples, always channel by channel.
    ///
    /// [`ImageRef::read`] returns the samples as the file stores them, which
    /// is planar for most files and interleaved for the rest. The
    /// specification requires a conforming decoder to read both, and the two
    /// are indistinguishable once the samples are in a `Vec` -- so a caller
    /// who forgets to check [`ImageRef::pixel_storage`] gets an image with
    /// its colour channels shuffled and no indication that anything happened.
    ///
    /// This returns planar order whatever the file used, which is the layout
    /// most image code expects. For a planar file it is exactly
    /// [`ImageRef::read`] and costs nothing extra.
    pub fn read_planar<T: Sample>(&self) -> Result<Vec<T>> {
        let samples = self.read::<T>()?;
        if self.image.pixel_storage == PixelStorage::Planar {
            return Ok(samples);
        }

        let channels = self.image.channels as usize;
        if channels <= 1 {
            return Ok(samples);
        }
        let pixels = samples.len() / channels;

        // Interleaved to planar: pixel `p`'s channel `c` sits at `p * n + c`
        // as stored and belongs at `c * pixels + p`.
        let mut out = Vec::with_capacity(samples.len());
        for c in 0..channels {
            out.extend((0..pixels).map(|p| samples[p * channels + c]));
        }
        // Any remainder past the last whole pixel is carried over, so this
        // cannot silently shorten an image whose length does not divide.
        out.extend_from_slice(&samples[pixels * channels..]);
        Ok(out)
    }

    /// Where the image's data lives, for callers that care.
    pub fn location(&self) -> Option<&Location> {
        self.element.data.location.as_ref()
    }

    /// The image's declared resolution, if it states one.
    pub fn resolution(&self) -> Option<Resolution> {
        self.associated("Resolution").into_iter().find_map(|e| Resolution::parse(e).ok())
    }

    /// Elements of `name` belonging to this image, whether they sit inside it
    /// or are shared through a `<Reference>`.
    fn associated(&self, name: &'static str) -> Vec<&'a xisf_core::header::Element> {
        self.file.reader.header().associated(self.element, name)
    }

    /// The image's embedded ICC colour profile, if it has one.
    ///
    /// Returned as raw bytes because that is what an ICC profile is: a
    /// self-describing structure this crate has no business interpreting.
    /// Note that it is **big-endian** by the ICC specification, which is the
    /// one place XISF departs from its own little-endian default -- and why
    /// the format forbids a `byteOrder` attribute here. Nothing is swapped;
    /// the bytes are handed over exactly as stored, which is what an ICC
    /// library expects.
    pub fn icc_profile(&self) -> Option<Result<Cow<'a, [u8]>>> {
        let element = *self.associated("ICCProfile").first()?;
        Some(self.file.reader.block(&element.data))
    }

    /// The image's RGB working space.
    ///
    /// `None` means the file declared none, in which case the space is sRGB
    /// by the spec's default -- see [`RgbWorkingSpace::srgb`]. The two are
    /// kept apart so a writer can round-trip a file that said nothing.
    pub fn rgb_working_space(&self) -> Option<RgbWorkingSpace> {
        self.associated("RGBWorkingSpace").into_iter().find_map(|e| RgbWorkingSpace::parse(e).ok())
    }

    /// The image's display function, which changes how it is shown rather
    /// than what it contains.
    ///
    /// `None` means the file declared none, and the identity applies.
    pub fn display_function(&self) -> Option<DisplayFunction> {
        self.associated("DisplayFunction").into_iter().find_map(|e| DisplayFunction::parse(e).ok())
    }

    /// Table properties attached to this image.
    pub fn tables(&self) -> Vec<Table> {
        let header = self.file.reader.header();
        self.associated("Table").into_iter().filter_map(|e| Table::parse(e, header).ok()).collect()
    }

    /// The image's colour filter array, if it is a mosaiced sensor image.
    ///
    /// Its presence is what says the pixels are a mosaic and need
    /// demosaicing; an encoder is forbidden from attaching one to an image
    /// that is not mosaiced, so this is a reliable signal rather than a hint.
    pub fn color_filter_array(&self) -> Option<ColorFilterArray> {
        self.associated("ColorFilterArray")
            .into_iter()
            .find_map(|e| ColorFilterArray::parse(e).ok())
    }

    /// The image's thumbnail, if it carries one.
    pub fn thumbnail(&self) -> Option<ThumbnailRef<'a>> {
        let element = *self.associated("Thumbnail").first()?;
        let image = xisf_core::image::parse_thumbnail(element).ok()?;
        Some(ThumbnailRef { file: self.file, element, image })
    }
}

/// An image's thumbnail: a small preview, which is an image in its own right.
#[derive(Debug)]
pub struct ThumbnailRef<'a> {
    file: &'a XisfFile,
    element: &'a Element,
    image: Image,
}

impl<'a> ThumbnailRef<'a> {
    /// The thumbnail's dimensions, without the channel count.
    pub fn geometry(&self) -> &[u64] {
        &self.image.dimensions
    }

    /// How many channels the thumbnail has.
    pub fn channels(&self) -> u64 {
        self.image.channels
    }

    /// The type of one sample. Thumbnails are conventionally `UInt8`, which
    /// is what makes them cheap to display whatever the image's own format.
    pub fn sample_format(&self) -> SampleFormat {
        self.image.sample_format
    }

    /// The colour space the thumbnail's channels are in.
    pub fn color_space(&self) -> ColorSpace {
        self.image.color_space
    }

    /// The parsed attributes.
    pub fn attributes(&self) -> &Image {
        &self.image
    }

    /// The thumbnail's pixel data, decompressed.
    pub fn bytes(&self) -> Result<Cow<'a, [u8]>> {
        self.file.reader.block(&self.element.data)
    }
}

/// One property in a file.
#[derive(Debug)]
pub struct PropertyRef<'a> {
    file: &'a XisfFile,
    element: &'a Element,
    property: Property,
}

impl<'a> PropertyRef<'a> {
    /// The property's identifier, possibly namespaced.
    pub fn id(&self) -> &str {
        &self.property.id
    }

    /// The declared type.
    pub fn kind(&self) -> PropertyType {
        self.property.kind
    }

    /// The parsed attributes.
    pub fn attributes(&self) -> &Property {
        &self.property
    }

    /// The value as text, for a scalar, string or `TimePoint`.
    ///
    /// The property's value, decoded according to its declared type.
    ///
    /// Prefer this to parsing [`PropertyRef::as_str`] yourself: the format
    /// permits binary, octal and hexadecimal integer literals, which Rust's
    /// own `parse` rejects, and the declared type decides whether a literal
    /// that fills its width is a large positive number or a negative one.
    pub fn value(&self) -> Option<ScalarValue> {
        self.property.value()
    }

    /// The property's value as it is written in the header.
    ///
    /// `None` for a vector, matrix or table, whose value is binary and has no
    /// textual form -- use [`bytes`](PropertyRef::bytes) or
    /// [`read`](PropertyRef::read) for those.
    pub fn as_str(&self) -> Option<&str> {
        match self.property.kind.shape {
            Shape::Scalar | Shape::TimePoint => self.property.value.as_deref(),
            Shape::String => {
                // A string is usually character data, but a long one may be
                // held in a data block instead, in which case there is no
                // text here to hand back.
                self.property.value.as_deref().or(self.property.text.as_deref())
            }
            _ => None,
        }
    }

    /// The property's data block, decompressed, if it has one.
    pub fn bytes(&self) -> Result<Cow<'a, [u8]>> {
        if self.element.data.location.is_none() {
            return Err(Error::new(
                ErrorKind::NotFound,
                format!("property {:?} has no data block", self.property.id),
            ));
        }
        self.file.reader.block(&self.element.data)
    }

    /// Read a vector or matrix property's components as `T`.
    ///
    /// `T` must be the property's own element type; this converts byte order,
    /// not element types, for the same reason [`ImageRef::read`] does not.
    pub fn read<T: Sample>(&self) -> Result<Vec<T>> {
        let element = self.property.kind.element.ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidArgument,
                format!("property {:?} has no element type to read", self.property.id),
            )
        })?;
        if element.name() != T::FORMAT.name() {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                format!(
                    "property {:?} holds {} components; asked for {}",
                    self.property.id,
                    element.name(),
                    T::FORMAT.name()
                ),
            ));
        }

        let expected = self.property.data_size().ok_or_else(|| {
            Error::new(
                ErrorKind::Unsupported,
                format!(
                    "property {:?} declares a shape this platform cannot hold",
                    self.property.id
                ),
            )
        })?;
        let bytes = self.bytes()?;
        if bytes.len() as u64 != expected {
            return Err(Error::new(
                ErrorKind::Truncated,
                format!(
                    "property {:?} declares {expected} bytes but its block holds {}",
                    self.property.id,
                    bytes.len()
                ),
            ));
        }

        Ok(T::decode(&bytes, self.element.data.byte_order))
    }
}

/// A pixel sample type that can be read in bulk.
///
/// Sealed: the set is fixed by the format's `sampleFormat` values, and adding
/// to it would mean adding to the format.
pub trait Sample: sealed::Sealed + Copy {
    /// The `sampleFormat` this type corresponds to.
    const FORMAT: SampleFormat;

    /// Decode a whole buffer of samples stored in `order`.
    #[doc(hidden)]
    fn decode(bytes: &[u8], order: ByteOrder) -> Vec<Self>;
}

mod sealed {
    pub trait Sealed {}
}

macro_rules! sample {
    ($ty:ty, $format:ident) => {
        impl sealed::Sealed for $ty {}
        impl Sample for $ty {
            const FORMAT: SampleFormat = SampleFormat::$format;

            fn decode(bytes: &[u8], order: ByteOrder) -> Vec<Self> {
                let (chunks, _) = bytes.as_chunks::<{ size_of::<$ty>() }>();
                match order {
                    ByteOrder::Big => chunks.iter().map(|c| <$ty>::from_be_bytes(*c)).collect(),
                    ByteOrder::Little => chunks.iter().map(|c| <$ty>::from_le_bytes(*c)).collect(),
                }
            }
        }
    };
}

sample!(u8, UInt8);
sample!(u16, UInt16);
sample!(u32, UInt32);
sample!(u64, UInt64);
sample!(f32, Float32);
sample!(f64, Float64);

#[cfg(test)]
mod tests {
    use super::*;
    use xisf_core::writer::{PendingImage, Writer};

    fn write(image: Image, data: Vec<u8>) -> XisfFile {
        let mut writer = Writer::new();
        writer
            .add_image(PendingImage::new(image, data, BlockOptions::default()))
            .expect("add_image");
        XisfFile::from_bytes(writer.to_bytes().expect("to_bytes")).expect("read back")
    }

    fn image(format: SampleFormat, channels: u64) -> Image {
        Image {
            dimensions: vec![5, 4],
            channels,
            sample_format: format,
            color_space: if channels >= 3 { ColorSpace::Rgb } else { ColorSpace::Gray },
            pixel_storage: PixelStorage::Planar,
            bounds: None,
            id: None,
            uuid: None,
            image_type: None,
            offset: None,
            orientation: None,
        }
    }

    #[test]
    fn reads_samples_of_the_right_type() {
        let values: Vec<u16> = (0..20u16).map(|i| i * 1000).collect();
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let file = write(image(SampleFormat::UInt16, 1), bytes);

        let read: Vec<u16> = file.images()[0].read().expect("read");
        assert_eq!(read, values);
    }

    /// Asking for the wrong type is an error, not a reinterpretation. Silently
    /// treating float bits as integers is the kind of thing that produces
    /// plausible-looking nonsense.
    #[test]
    fn the_wrong_sample_type_is_refused() {
        let file = write(image(SampleFormat::Float32, 1), vec![0; 80]);
        let err = file.images()[0].read::<u16>().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidArgument);
        assert!(err.message().contains("Float32"), "the message should name both types");
    }

    /// The stored order is the format's, not the machine's: a big-endian
    /// block reads the same on any host.
    #[test]
    fn big_endian_blocks_are_byte_swapped() {
        let values: Vec<u32> = vec![1, 0x0102_0304, u32::MAX, 42];
        let big: Vec<u8> = values.iter().flat_map(|v| v.to_be_bytes()).collect();

        let xml = format!(
            r#"<xisf version="1.0"><Image geometry="4:1:1" sampleFormat="UInt32" \
byteOrder="big" location="inline:base64">{}</Image></xisf>"#,
            base64_encode(&big)
        );
        let file = XisfFile::from_bytes(monolithic(&xml)).expect("read");
        let read: Vec<u32> = file.images()[0].read().expect("read");
        assert_eq!(read, values, "a big-endian block must not depend on the host");
        assert_eq!(file.images()[0].byte_order(), ByteOrder::Big);
    }

    /// And with no attribute at all the block is little-endian, per the spec,
    /// whatever the host happens to be.
    #[test]
    fn an_absent_byte_order_means_little_endian() {
        let values: Vec<u32> = vec![0x0a0b_0c0d, 7];
        let little: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let xml = format!(
            r#"<xisf version="1.0"><Image geometry="2:1:1" sampleFormat="UInt32" \
location="inline:base64">{}</Image></xisf>"#,
            base64_encode(&little)
        );
        let file = XisfFile::from_bytes(monolithic(&xml)).expect("read");
        assert_eq!(file.images()[0].byte_order(), ByteOrder::Little);
        assert_eq!(file.images()[0].read::<u32>().unwrap(), values);
    }

    #[test]
    fn geometry_and_metadata_come_through() {
        let file = write(image(SampleFormat::UInt8, 3), vec![0; 60]);
        let images = file.images();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].geometry(), &[5, 4]);
        assert_eq!(images[0].channels(), 3);
        assert_eq!(images[0].color_space(), ColorSpace::Rgb);
    }

    // --- helpers -----------------------------------------------------

    fn monolithic(xml: &str) -> Vec<u8> {
        let xml = xml.replace("\\\n", "");
        let mut out = Vec::new();
        out.extend_from_slice(b"XISF0100");
        out.extend_from_slice(&(xml.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(xml.as_bytes());
        out
    }

    fn base64_encode(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
            let digits = [n >> 18, (n >> 12) & 63, (n >> 6) & 63, n & 63];
            for (i, d) in digits.iter().enumerate() {
                if i <= chunk.len() {
                    out.push(TABLE[*d as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }
}
