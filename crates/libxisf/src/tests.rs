//! Tests for the C entry points, driven from Rust as a C caller would.

use super::*;
use xisf_core::image::PixelStorage;
use xisf_core::writer::{BlockOptions, PendingImage, Writer};

fn sample_file(format: SampleFormat, channels: u64) -> Vec<u8> {
    let image = Image {
        dimensions: vec![7, 5],
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
    };
    let size = image.data_size().expect("size") as usize;
    let data: Vec<u8> = (0..size).map(|i| (i * 11 + 3) as u8).collect();

    let mut writer = Writer::new();
    writer.add_image(PendingImage::new(image, data, BlockOptions::default())).expect("add_image");
    writer.to_bytes().expect("to_bytes")
}

/// Open a file the way C would, from memory.
fn open(bytes: &[u8]) -> (*mut XisfFile, i32) {
    let mut err = -1;
    let file = unsafe { xisf_open_memory(bytes.as_ptr().cast::<c_void>(), bytes.len(), &mut err) };
    (file, err)
}

#[test]
fn opens_reads_and_closes() {
    let bytes = sample_file(SampleFormat::UInt16, 1);
    let (file, err) = open(&bytes);
    assert!(!file.is_null());
    assert_eq!(err, XisfError::Ok as i32);

    assert_eq!(unsafe { xisf_image_count(file) }, 1);
    let image = unsafe { xisf_image_at(file, 0) };
    assert!(!image.is_null());

    assert_eq!(unsafe { xisf_image_width(image) }, 7);
    assert_eq!(unsafe { xisf_image_height(image) }, 5);
    assert_eq!(unsafe { xisf_image_channels(image) }, 1);
    assert_eq!(unsafe { xisf_image_sample_format(image) }, 1, "UInt16");
    assert_eq!(unsafe { xisf_image_data_size(image) }, 7 * 5 * 2);

    unsafe { xisf_close(file) };
}

/// The same image asked for twice must be the same handle, or the library is
/// allocating one per call and leaking it.
#[test]
fn image_handles_are_borrowed_not_allocated() {
    let bytes = sample_file(SampleFormat::UInt8, 1);
    let (file, _) = open(&bytes);

    let first = unsafe { xisf_image_at(file, 0) };
    let second = unsafe { xisf_image_at(file, 0) };
    assert_eq!(first, second, "each call handed back a different handle");

    unsafe { xisf_close(file) };
}

#[test]
fn reading_into_a_caller_buffer_matches_the_data() {
    let bytes = sample_file(SampleFormat::UInt16, 1);
    let (file, _) = open(&bytes);
    let image = unsafe { xisf_image_at(file, 0) };

    let size = unsafe { xisf_image_data_size(image) } as usize;
    let mut buffer = vec![0u8; size];
    let rc = unsafe { xisf_image_read(image, buffer.as_mut_ptr().cast::<c_void>(), size) };
    assert_eq!(rc, XisfError::Ok as i32);

    let expected: Vec<u8> = (0..size).map(|i| (i * 11 + 3) as u8).collect();
    assert_eq!(buffer, expected);

    unsafe { xisf_close(file) };
}

/// A short buffer must be refused before anything is written, not filled
/// partway and then reported.
#[test]
fn a_short_buffer_is_refused_and_left_untouched() {
    let bytes = sample_file(SampleFormat::UInt16, 1);
    let (file, _) = open(&bytes);
    let image = unsafe { xisf_image_at(file, 0) };

    let mut buffer = [0xAAu8; 8];
    let rc = unsafe { xisf_image_read(image, buffer.as_mut_ptr().cast::<c_void>(), buffer.len()) };
    assert_eq!(rc, XisfError::InvalidArgument as i32);
    assert_eq!(buffer, [0xAAu8; 8], "the buffer was written to despite the failure");

    unsafe { xisf_close(file) };
}

#[test]
fn the_allocating_read_round_trips_through_xisf_free() {
    let bytes = sample_file(SampleFormat::Float32, 1);
    let (file, _) = open(&bytes);
    let image = unsafe { xisf_image_at(file, 0) };

    let mut size = 0usize;
    let mut err = -1;
    let buffer = unsafe { xisf_image_read_alloc(image, &mut size, &mut err) };
    assert!(!buffer.is_null());
    assert_eq!(err, XisfError::Ok as i32);
    assert_eq!(size, 7 * 5 * 4);

    // The pointer is cast to the sample type by any real caller.
    assert_eq!(buffer as usize % align_of::<f32>(), 0, "the buffer is not aligned for f32");
    let seen = unsafe { core::slice::from_raw_parts(buffer.cast::<u8>(), size) };
    let expected: Vec<u8> = (0..size).map(|i| (i * 11 + 3) as u8).collect();
    assert_eq!(seen, &expected[..]);

    unsafe { xisf_free(buffer) };
    unsafe { xisf_close(file) };
}

/// Everything must tolerate a null handle rather than crashing, because a C
/// caller who ignored an error will pass one.
#[test]
fn null_handles_are_refused_rather_than_dereferenced() {
    let null_file: *const XisfFile = core::ptr::null();
    let null_image: *const XisfImage = core::ptr::null();

    assert_eq!(unsafe { xisf_image_count(null_file) }, 0);
    assert!(unsafe { xisf_image_at(null_file, 0) }.is_null());

    assert_eq!(unsafe { xisf_image_width(null_image) }, 0);
    assert_eq!(unsafe { xisf_image_height(null_image) }, 0);
    assert_eq!(unsafe { xisf_image_channels(null_image) }, 0);
    assert_eq!(unsafe { xisf_image_data_size(null_image) }, 0);
    assert_eq!(unsafe { xisf_image_dimension_count(null_image) }, 0);
    assert_eq!(unsafe { xisf_image_dimension(null_image, 0) }, 0);
    assert_eq!(unsafe { xisf_image_fits_keyword_count(null_image) }, 0);
    assert!(unsafe { xisf_image_fits_keyword_name(null_image, 0) }.is_null());

    assert_eq!(
        unsafe { xisf_image_read(null_image, core::ptr::null_mut(), 0) },
        XisfError::InvalidArgument as i32
    );
    assert!(
        unsafe { xisf_image_read_alloc(null_image, core::ptr::null_mut(), core::ptr::null_mut()) }
            .is_null()
    );
    assert_eq!(unsafe { xisf_image_verify(null_image) }, XisfError::InvalidArgument as i32);

    // Both destructors accept null.
    unsafe { xisf_close(core::ptr::null_mut()) };
    unsafe { xisf_free(core::ptr::null_mut()) };
}

#[test]
fn opening_rubbish_reports_why_rather_than_crashing() {
    for (bytes, expected) in [
        (b"not an xisf file".to_vec(), XisfError::NotXisf),
        (b"XISF0100".to_vec(), XisfError::Truncated),
        (Vec::new(), XisfError::Truncated),
    ] {
        let mut err = -1;
        let file =
            unsafe { xisf_open_memory(bytes.as_ptr().cast::<c_void>(), bytes.len(), &mut err) };
        assert!(file.is_null(), "{bytes:?} should not have opened");
        assert_eq!(err, expected as i32, "wrong code for {bytes:?}");
    }

    // A null buffer is an argument error, not a crash.
    let mut err = -1;
    assert!(unsafe { xisf_open_memory(core::ptr::null(), 10, &mut err) }.is_null());
    assert_eq!(err, XisfError::InvalidArgument as i32);
}

/// An out-of-range index must be rejected rather than trusted.
#[test]
fn out_of_range_indices_return_null_or_zero() {
    let bytes = sample_file(SampleFormat::UInt8, 1);
    let (file, _) = open(&bytes);

    assert!(unsafe { xisf_image_at(file, 1) }.is_null());
    assert!(unsafe { xisf_image_at(file, usize::MAX) }.is_null());

    let image = unsafe { xisf_image_at(file, 0) };
    assert_eq!(unsafe { xisf_image_dimension(image, 99) }, 0);
    assert!(unsafe { xisf_image_fits_keyword_name(image, 99) }.is_null());

    unsafe { xisf_close(file) };
}

/// The out-parameters may be null: a caller who does not want the error code
/// or the size should not have to provide somewhere to put them.
#[test]
fn null_out_parameters_are_accepted() {
    let bytes = sample_file(SampleFormat::UInt8, 1);
    let file = unsafe {
        xisf_open_memory(bytes.as_ptr().cast::<c_void>(), bytes.len(), core::ptr::null_mut())
    };
    assert!(!file.is_null());

    let image = unsafe { xisf_image_at(file, 0) };
    let buffer =
        unsafe { xisf_image_read_alloc(image, core::ptr::null_mut(), core::ptr::null_mut()) };
    assert!(!buffer.is_null());

    unsafe { xisf_free(buffer) };
    unsafe { xisf_close(file) };
}

#[test]
fn error_messages_and_names_are_always_printable() {
    // Including codes that are not ours: C can pass any int.
    for code in [-1, 0, 1, 11, 12, 9999, i32::MIN, i32::MAX] {
        let text = unsafe { CStr::from_ptr(xisf_error_message(code)) };
        assert!(!text.to_bytes().is_empty(), "code {code} produced an empty message");
    }
    for format in [-1, 0, 7, 8, i32::MAX] {
        let name = unsafe { CStr::from_ptr(xisf_sample_format_name(format)) };
        assert!(!name.to_bytes().is_empty(), "format {format} produced an empty name");
    }
    assert_eq!(unsafe { CStr::from_ptr(xisf_sample_format_name(4)) }, c"Float32");
    assert_eq!(xisf_sample_format_size(4), 4);
    assert_eq!(xisf_sample_format_size(99), 0, "an unknown format has no size");

    let version = unsafe { CStr::from_ptr(xisf_version()) };
    assert_eq!(version.to_str().unwrap(), env!("CARGO_PKG_VERSION"));
}

/// The error codes are the ABI: the header fixes them, so they must not move.
#[test]
fn error_codes_match_the_header() {
    let header = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("include/xisf.h"),
    )
    .expect("read xisf.h");

    for (name, value) in [
        ("XISF_OK", 0),
        ("XISF_ERR_NOT_XISF", 1),
        ("XISF_ERR_TRUNCATED", 2),
        ("XISF_ERR_BAD_HEADER", 3),
        ("XISF_ERR_BAD_ATTRIBUTE", 4),
        ("XISF_ERR_UNSUPPORTED", 5),
        ("XISF_ERR_CHECKSUM_MISMATCH", 6),
        ("XISF_ERR_COMPRESSION", 7),
        ("XISF_ERR_NOT_FOUND", 8),
        ("XISF_ERR_INVALID_ARGUMENT", 9),
        ("XISF_ERR_IO", 10),
        ("XISF_ERR_OUT_OF_MEMORY", 11),
    ] {
        let expected = format!("{name} = {value}");
        assert!(
            header.contains(&expected),
            "the header does not declare `{expected}`; the Rust side and the ABI have drifted"
        );
    }

    // And the sample formats, which a caller switches on.
    for (name, value) in [
        ("XISF_SAMPLE_UINT8", 0),
        ("XISF_SAMPLE_UINT16", 1),
        ("XISF_SAMPLE_UINT32", 2),
        ("XISF_SAMPLE_UINT64", 3),
        ("XISF_SAMPLE_FLOAT32", 4),
        ("XISF_SAMPLE_FLOAT64", 5),
        ("XISF_SAMPLE_COMPLEX32", 6),
        ("XISF_SAMPLE_COMPLEX64", 7),
    ] {
        assert!(header.contains(&format!("{name} = {value}")), "{name} drifted");
    }
}

#[test]
fn checksums_verify_through_the_c_api() {
    let image = Image {
        dimensions: vec![4, 4],
        channels: 1,
        sample_format: SampleFormat::UInt16,
        color_space: ColorSpace::Gray,
        pixel_storage: PixelStorage::Planar,
        bounds: None,
        id: None,
        uuid: None,
        image_type: None,
        offset: None,
        orientation: None,
    };
    let data = vec![7u8; image.data_size().unwrap() as usize];
    let mut writer = Writer::new();
    writer
        .add_image(PendingImage::new(
            image,
            data,
            BlockOptions {
                compression: None,
                checksum: Some(xisf_core::block::ChecksumAlgorithm::Sha256),
            },
        ))
        .unwrap();
    let bytes = writer.to_bytes().unwrap();

    let (file, _) = open(&bytes);
    let image = unsafe { xisf_image_at(file, 0) };
    assert_eq!(unsafe { xisf_image_verify(image) }, XisfError::Ok as i32);
    unsafe { xisf_close(file) };
}

/// A file with no checksum is not a verification failure: the specification
/// makes them optional.
#[test]
fn an_absent_checksum_is_not_a_failure() {
    let bytes = sample_file(SampleFormat::UInt8, 1);
    let (file, _) = open(&bytes);
    let image = unsafe { xisf_image_at(file, 0) };
    assert_eq!(unsafe { xisf_image_verify(image) }, XisfError::Ok as i32);
    unsafe { xisf_close(file) };
}
