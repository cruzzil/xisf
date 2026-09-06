//! A C API for XISF, built on [`xisf_core`].
//!
//! The contract is `include/xisf.h`; this implements it. The header is the
//! documentation a C caller reads, so the reasoning about ownership and
//! nullability lives there, and this file says only what a Rust reader needs.
//!
//! # Not libXISF
//!
//! There is an unrelated C++ library of the same name. It exposes C++ classes
//! and no `extern "C"` at all, so its symbols and these share nothing but the
//! filename: a program built against that one and linked against this fails at
//! link time with undefined symbols, which is a loud failure rather than a
//! quiet one.
//!
//! # Conventions, and why
//!
//! - **Enums crossing the boundary are returned, never taken.** C may pass any
//!   integer, and holding an out-of-range value in a `#[repr]` Rust enum is
//!   undefined behaviour. Where a value comes *in* it is a `c_int` converted
//!   by a checked lookup.
//! - **No panic may cross the boundary.** Unwinding out of an `extern "C"`
//!   function is undefined behaviour, so every entry point runs inside
//!   `panic::guard`, which catches, reports once, and returns a fallback.
//! - **Raw pointers go through the `ffi` module.** One place makes each
//!   judgement, rather than the same one being re-made at every call site.

use std::ffi::{CStr, CString, c_char, c_void};

use xisf_core::block::ByteOrder;
use xisf_core::image::{ColorSpace, Image, SampleFormat};
use xisf_core::reader::ChecksumStatus;
use xisf_core::{ErrorKind, Reader};

mod ffi;
mod panic;

use ffi::{as_ref, c_str, write_out};
use panic::guard;

// ---- Error codes ----------------------------------------------------

/// Mirrors `xisf_error_t`. The discriminants are the ABI and must not move.
#[repr(i32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum XisfError {
    Ok = 0,
    NotXisf = 1,
    Truncated = 2,
    BadHeader = 3,
    BadAttribute = 4,
    Unsupported = 5,
    ChecksumMismatch = 6,
    Compression = 7,
    NotFound = 8,
    InvalidArgument = 9,
    Io = 10,
    OutOfMemory = 11,
}

impl From<ErrorKind> for XisfError {
    fn from(kind: ErrorKind) -> Self {
        match kind {
            ErrorKind::NotXisf => XisfError::NotXisf,
            ErrorKind::Truncated => XisfError::Truncated,
            ErrorKind::BadHeader => XisfError::BadHeader,
            ErrorKind::BadAttribute => XisfError::BadAttribute,
            ErrorKind::Unsupported => XisfError::Unsupported,
            ErrorKind::ChecksumMismatch => XisfError::ChecksumMismatch,
            ErrorKind::Compression => XisfError::Compression,
            ErrorKind::NotFound => XisfError::NotFound,
            ErrorKind::InvalidArgument => XisfError::InvalidArgument,
            ErrorKind::Io => XisfError::Io,
        }
    }
}

/// A human-readable description of an error code.
///
/// # Safety
/// Always safe; the returned pointer is `'static`.
#[unsafe(no_mangle)]
pub extern "C" fn xisf_error_message(error: i32) -> *const c_char {
    let text: &CStr = match error {
        0 => c"no error",
        1 => c"not an XISF file",
        2 => c"the file is truncated",
        3 => c"the XISF header could not be parsed",
        4 => c"an attribute is malformed",
        5 => c"unsupported by this build",
        6 => c"a checksum did not match",
        7 => c"decompression failed",
        8 => c"not found",
        9 => c"invalid argument",
        10 => c"an I/O error occurred",
        11 => c"out of memory",
        _ => c"unknown error",
    };
    text.as_ptr()
}

/// The library's version string.
#[unsafe(no_mangle)]
pub extern "C" fn xisf_version() -> *const c_char {
    // A byte string with an explicit NUL, so the pointer is valid for C.
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast::<c_char>()
}

// ---- Sample formats -------------------------------------------------

fn sample_format_from_i32(value: i32) -> Option<SampleFormat> {
    Some(match value {
        0 => SampleFormat::UInt8,
        1 => SampleFormat::UInt16,
        2 => SampleFormat::UInt32,
        3 => SampleFormat::UInt64,
        4 => SampleFormat::Float32,
        5 => SampleFormat::Float64,
        6 => SampleFormat::Complex32,
        7 => SampleFormat::Complex64,
        _ => return None,
    })
}

fn sample_format_to_i32(format: SampleFormat) -> i32 {
    match format {
        SampleFormat::UInt8 => 0,
        SampleFormat::UInt16 => 1,
        SampleFormat::UInt32 => 2,
        SampleFormat::UInt64 => 3,
        SampleFormat::Float32 => 4,
        SampleFormat::Float64 => 5,
        SampleFormat::Complex32 => 6,
        SampleFormat::Complex64 => 7,
    }
}

/// Bytes per sample, or zero for an unrecognised format.
///
/// Taken as a plain `int`: C may pass anything, and materialising an
/// out-of-range value in a `#[repr]` Rust enum would be undefined behaviour.
#[unsafe(no_mangle)]
pub extern "C" fn xisf_sample_format_size(format: i32) -> usize {
    sample_format_from_i32(format).map_or(0, SampleFormat::size)
}

/// The specification's name for a sample format.
#[unsafe(no_mangle)]
pub extern "C" fn xisf_sample_format_name(format: i32) -> *const c_char {
    match sample_format_from_i32(format) {
        None => c"Unknown".as_ptr(),
        Some(SampleFormat::UInt8) => c"UInt8".as_ptr(),
        Some(SampleFormat::UInt16) => c"UInt16".as_ptr(),
        Some(SampleFormat::UInt32) => c"UInt32".as_ptr(),
        Some(SampleFormat::UInt64) => c"UInt64".as_ptr(),
        Some(SampleFormat::Float32) => c"Float32".as_ptr(),
        Some(SampleFormat::Float64) => c"Float64".as_ptr(),
        Some(SampleFormat::Complex32) => c"Complex32".as_ptr(),
        Some(SampleFormat::Complex64) => c"Complex64".as_ptr(),
    }
}

// ---- Handles --------------------------------------------------------

/// One image, resolved once at open time.
///
/// Everything a caller can ask for is precomputed, including the interned
/// FITS keyword strings, so every accessor is infallible and returns a
/// pointer that stays valid until the file closes.
#[derive(Debug)]
struct ImageEntry {
    image: Image,
    /// Cloned rather than looked up: resolving the element for every read
    /// meant rebuilding a vector of every image in the header each time.
    data: xisf_core::header::DataRef,
    byte_order: ByteOrder,
    compressed: bool,
    fits: Vec<(CString, CString, CString)>,
}

/// The `xisf_file_t` a caller holds.
#[derive(Debug)]
pub struct XisfFile {
    reader: Reader,
    images: Vec<ImageEntry>,
    /// Borrowed handles, built once on first use.
    ///
    /// A handle names its file by address, which is not known until the file
    /// itself has been boxed -- hence the deferral. Handing out a fresh
    /// allocation per call would leak, since the header promises these belong
    /// to the file and are never freed by the caller.
    handles: std::sync::OnceLock<Vec<XisfImage>>,
}

/// The `xisf_image_t` a caller borrows.
///
/// A file and an index rather than a pointer into the file's vector: an
/// accessor can then re-resolve safely, and no image outlives its file
/// without that being detectable.
#[derive(Debug)]
pub struct XisfImage {
    file: *const XisfFile,
    entry: usize,
}

// SAFETY: a handle is immutable once built and only ever read. Dereferencing
// `file` is sound under the contract the header states -- the file must be
// open for as long as the handle is used -- which is the caller's to keep and
// is no different across threads than within one.
unsafe impl Send for XisfImage {}
unsafe impl Sync for XisfImage {}

impl XisfFile {
    fn build(reader: Reader) -> Self {
        let images = reader
            .header()
            .images()
            .into_iter()
            .filter_map(|element| {
                let image = Image::parse(element).ok()?;
                let fits = element
                    .children_named("FITSKeyword")
                    .map(|k| {
                        (
                            intern(k.attr("name").unwrap_or_default()),
                            intern(k.attr("value").unwrap_or_default()),
                            intern(k.attr("comment").unwrap_or_default()),
                        )
                    })
                    .collect();
                Some(ImageEntry {
                    image,
                    data: element.data.clone(),
                    byte_order: element.data.byte_order,
                    compressed: element.data.compression.is_some(),
                    fits,
                })
            })
            .collect();
        Self { reader, images, handles: std::sync::OnceLock::new() }
    }
}

/// A C string that never fails: an interior NUL is replaced rather than
/// rejected, since a metadata value should not make an accessor unusable.
fn intern(text: &str) -> CString {
    CString::new(text.replace('\0', " ")).unwrap_or_default()
}

/// Resolve a borrowed image handle back to its file and entry.
///
/// # Safety
/// `image` must be null or a handle this library produced, whose file is open.
unsafe fn resolve<'a>(image: *const XisfImage) -> Option<(&'a XisfFile, &'a ImageEntry)> {
    let handle = unsafe { as_ref(image) }?;
    let file = unsafe { as_ref(handle.file) }?;
    let entry = file.images.get(handle.entry)?;
    Some((file, entry))
}

// ---- Opening and closing --------------------------------------------

/// Open a monolithic XISF file.
///
/// # Safety
/// `path` must be null or a valid NUL-terminated string; `err` null or
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xisf_open(path: *const c_char, err: *mut i32) -> *mut XisfFile {
    guard("xisf_open", std::ptr::null_mut(), || {
        let Some(path) = (unsafe { c_str(path) }) else {
            unsafe { write_out(err, XisfError::InvalidArgument as i32) };
            return std::ptr::null_mut();
        };
        let Ok(path) = path.to_str() else {
            unsafe { write_out(err, XisfError::InvalidArgument as i32) };
            return std::ptr::null_mut();
        };

        match Reader::open(path) {
            Ok(reader) => {
                unsafe { write_out(err, XisfError::Ok as i32) };
                Box::into_raw(Box::new(XisfFile::build(reader)))
            }
            Err(e) => {
                unsafe { write_out(err, XisfError::from(e.kind()) as i32) };
                std::ptr::null_mut()
            }
        }
    })
}

/// Open an XISF file already in memory. The bytes are copied.
///
/// # Safety
/// `data` must be null or point at `size` readable bytes; `err` null or
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xisf_open_memory(
    data: *const c_void,
    size: usize,
    err: *mut i32,
) -> *mut XisfFile {
    guard("xisf_open_memory", std::ptr::null_mut(), || {
        if data.is_null() {
            unsafe { write_out(err, XisfError::InvalidArgument as i32) };
            return std::ptr::null_mut();
        }
        // SAFETY: the caller guarantees `size` readable bytes at `data`.
        let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size) }.to_vec();

        match Reader::from_bytes(bytes) {
            Ok(reader) => {
                unsafe { write_out(err, XisfError::Ok as i32) };
                Box::into_raw(Box::new(XisfFile::build(reader)))
            }
            Err(e) => {
                unsafe { write_out(err, XisfError::from(e.kind()) as i32) };
                std::ptr::null_mut()
            }
        }
    })
}

/// Close a file. Null is accepted and ignored.
///
/// # Safety
/// `file` must be null or a handle from `xisf_open`, not already closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xisf_close(file: *mut XisfFile) {
    guard("xisf_close", (), || {
        if !file.is_null() {
            drop(unsafe { Box::from_raw(file) });
        }
    })
}

/// How many images the file holds.
///
/// # Safety
/// `file` must be null or a valid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xisf_image_count(file: *const XisfFile) -> usize {
    guard("xisf_image_count", 0, || unsafe { as_ref(file) }.map_or(0, |f| f.images.len()))
}

/// Borrow an image by index. The result belongs to the file.
///
/// # Safety
/// `file` must be null or a valid handle, open for as long as the result is
/// used.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xisf_image_at(file: *const XisfFile, index: usize) -> *const XisfImage {
    guard("xisf_image_at", std::ptr::null(), || {
        let Some(handle) = (unsafe { as_ref(file) }) else {
            return std::ptr::null();
        };
        let handles = handle.handles.get_or_init(|| {
            (0..handle.images.len()).map(|entry| XisfImage { file, entry }).collect()
        });
        handles.get(index).map_or(std::ptr::null(), |h| h as *const XisfImage)
    })
}

// ---- Geometry -------------------------------------------------------

macro_rules! image_accessor {
    ($(#[$meta:meta])* $name:ident -> $ret:ty, $fallback:expr, |$entry:ident| $body:expr) => {
        $(#[$meta])*
        ///
        /// # Safety
        /// `image` must be null or a handle from `xisf_image_at` whose file is
        /// still open.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(image: *const XisfImage) -> $ret {
            guard(stringify!($name), $fallback, || {
                match unsafe { resolve(image) } {
                    None => $fallback,
                    Some((_, $entry)) => $body,
                }
            })
        }
    };
}

image_accessor!(
    /// How many dimensions the image has, not counting channels.
    xisf_image_dimension_count -> usize, 0, |e| e.image.dimensions.len()
);
image_accessor!(
    /// The number of channels.
    xisf_image_channels -> u64, 0, |e| e.image.channels
);
image_accessor!(
    /// The image's width, or zero if it has no dimensions.
    xisf_image_width -> u64, 0, |e| e.image.dimensions.first().copied().unwrap_or(0)
);
image_accessor!(
    /// The image's height, or zero if it is one-dimensional.
    xisf_image_height -> u64, 0, |e| e.image.dimensions.get(1).copied().unwrap_or(0)
);
image_accessor!(
    /// The sample format, as an `xisf_sample_format_t`.
    xisf_image_sample_format -> i32, 0, |e| sample_format_to_i32(e.image.sample_format)
);
image_accessor!(
    /// The colour space, as an `xisf_color_space_t`.
    xisf_image_color_space -> i32, 0, |e| match e.image.color_space {
        ColorSpace::Gray => 0,
        ColorSpace::Rgb => 1,
        ColorSpace::CieLab => 2,
    }
);
image_accessor!(
    /// The stored byte order, as an `xisf_byte_order_t`.
    xisf_image_byte_order -> i32, 0, |e| match e.byte_order {
        ByteOrder::Little => 0,
        ByteOrder::Big => 1,
    }
);
image_accessor!(
    /// Whether the pixel data is stored compressed.
    xisf_image_is_compressed -> i32, 0, |e| i32::from(e.compressed)
);
image_accessor!(
    /// The size of the pixel data in bytes, or zero if the geometry overflows.
    xisf_image_data_size -> u64, 0, |e| e.image.data_size().unwrap_or(0)
);
image_accessor!(
    /// How many FITS keywords the image carries.
    xisf_image_fits_keyword_count -> usize, 0, |e| e.fits.len()
);

/// One dimension of the image, fastest-varying first.
///
/// # Safety
/// As the other image accessors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xisf_image_dimension(image: *const XisfImage, index: usize) -> u64 {
    guard("xisf_image_dimension", 0, || match unsafe { resolve(image) } {
        None => 0,
        Some((_, entry)) => entry.image.dimensions.get(index).copied().unwrap_or(0),
    })
}

macro_rules! fits_accessor {
    ($(#[$meta:meta])* $name:ident, $field:tt) => {
        $(#[$meta])*
        ///
        /// # Safety
        /// As the other image accessors. The returned pointer belongs to the
        /// file and stays valid until it is closed.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(image: *const XisfImage, index: usize) -> *const c_char {
            guard(stringify!($name), std::ptr::null(), || {
                match unsafe { resolve(image) } {
                    None => std::ptr::null(),
                    Some((_, entry)) => {
                        entry.fits.get(index).map_or(std::ptr::null(), |k| k.$field.as_ptr())
                    }
                }
            })
        }
    };
}

fits_accessor!(
    /// A FITS keyword's name.
    xisf_image_fits_keyword_name, 0
);
fits_accessor!(
    /// A FITS keyword's value.
    xisf_image_fits_keyword_value, 1
);
fits_accessor!(
    /// A FITS keyword's comment.
    xisf_image_fits_keyword_comment, 2
);

// ---- Pixel data -----------------------------------------------------

/// Copy pixel data into a caller-provided buffer.
///
/// # Safety
/// `image` as the other accessors; `buffer` must be null or writable for
/// `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xisf_image_read(
    image: *const XisfImage,
    buffer: *mut c_void,
    size: usize,
) -> i32 {
    guard("xisf_image_read", XisfError::InvalidArgument as i32, || {
        let Some((file, entry)) = (unsafe { resolve(image) }) else {
            return XisfError::InvalidArgument as i32;
        };
        if buffer.is_null() {
            return XisfError::InvalidArgument as i32;
        }
        let data = match file.reader.block(&entry.data) {
            Ok(data) => data,
            Err(e) => return XisfError::from(e.kind()) as i32,
        };
        // Checked before writing anything, so a short buffer leaves it
        // untouched rather than partly filled.
        if size < data.len() {
            return XisfError::InvalidArgument as i32;
        }

        // SAFETY: `buffer` is non-null and the caller guarantees `size`
        // writable bytes, which the check above proves is enough.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), buffer.cast::<u8>(), data.len()) };
        XisfError::Ok as i32
    })
}

/// Read pixel data into a buffer this library allocates.
///
/// # Safety
/// `image` as the other accessors; `size` and `err` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xisf_image_read_alloc(
    image: *const XisfImage,
    size: *mut usize,
    err: *mut i32,
) -> *mut c_void {
    guard("xisf_image_read_alloc", std::ptr::null_mut(), || {
        let fail = |code: XisfError| {
            unsafe { write_out(err, code as i32) };
            unsafe { write_out(size, 0usize) };
            std::ptr::null_mut()
        };

        let Some((file, entry)) = (unsafe { resolve(image) }) else {
            return fail(XisfError::InvalidArgument);
        };
        let data = match file.reader.block(&entry.data) {
            Ok(data) => data,
            Err(e) => return fail(XisfError::from(e.kind())),
        };

        let buffer = ffi::buffer::allocate(&data);
        if buffer.is_null() {
            return fail(XisfError::OutOfMemory);
        }
        unsafe { write_out(size, data.len()) };
        unsafe { write_out(err, XisfError::Ok as i32) };
        buffer.cast::<c_void>()
    })
}

/// Verify the image's recorded checksum, if it has one.
///
/// # Safety
/// As the other image accessors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xisf_image_verify(image: *const XisfImage) -> i32 {
    guard("xisf_image_verify", XisfError::InvalidArgument as i32, || {
        let Some((file, entry)) = (unsafe { resolve(image) }) else {
            return XisfError::InvalidArgument as i32;
        };
        match file.reader.verify(&entry.data) {
            // No checksum is not a failure; the specification makes them
            // optional.
            Ok(ChecksumStatus::Valid | ChecksumStatus::Absent) => XisfError::Ok as i32,
            Ok(ChecksumStatus::Invalid) => XisfError::ChecksumMismatch as i32,
            Err(e) => XisfError::from(e.kind()) as i32,
        }
    })
}

/// Release a buffer from `xisf_image_read_alloc`.
///
/// # Safety
/// `pointer` must be null or a buffer this library returned and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xisf_free(pointer: *mut c_void) {
    guard("xisf_free", (), || unsafe { ffi::free_buffer(pointer) })
}

#[cfg(test)]
mod tests;
