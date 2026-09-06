/*
 * xisf.h -- a C API for the Extensible Image Serialization Format.
 *
 * Copyright (c) 2026 The xisf-rs Developers. MIT licensed; see LICENSE.
 *
 * ---------------------------------------------------------------------------
 *
 * THIS IS NOT libXISF.
 *
 * There is an unrelated C++ library also called libXISF, by Dusan Poizl, and
 * this shares its name and nothing else.  That library exposes C++ classes in
 * namespace LibXISF -- no `extern "C"` anywhere -- so its every symbol is
 * mangled and takes std::string, std::filesystem::path or its own classes.
 * None of it is reachable from C, and none of it is what this declares.
 *
 * The two symbol sets are therefore disjoint: everything here is a plain C
 * name, everything there is mangled.  A program built against that library and
 * linked against this one fails at link time with undefined symbols, which is
 * a loud failure rather than a quiet one.  That is the only reason sharing the
 * name is tolerable.
 *
 * ---------------------------------------------------------------------------
 *
 * CONVENTIONS
 *
 * Ownership.  Anything this library allocates for you is released with
 * xisf_free().  Do not call the C library's free() on it: that would fix the
 * allocator into the ABI, and on Windows a DLL and its caller can link
 * different C runtimes with separate heaps, where freeing across the boundary
 * corrupts one of them.  Handles have their own destructors -- xisf_close()
 * for a file -- and must not be passed to xisf_free().
 *
 * Errors.  Functions that can fail take a trailing `xisf_error_t *err`, which
 * may be NULL if you do not want it.  It is set to XISF_OK on success and left
 * meaningful only when the call fails.  xisf_error_message() names any code.
 *
 * Null.  Every pointer parameter may be NULL unless stated otherwise; a NULL
 * handle produces XISF_ERR_INVALID_ARGUMENT rather than a crash.
 *
 * Threads.  A handle may be used from several threads as long as none of them
 * closes it.  Handles are read-only once opened.
 */

#ifndef XISF_H
#define XISF_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ---- Version ------------------------------------------------------- */

#define XISF_VERSION_MAJOR 0
#define XISF_VERSION_MINOR 1
#define XISF_VERSION_PATCH 0

/* The library's version string, e.g. "0.1.0". Never NULL. */
const char *xisf_version(void);

/* ---- Errors -------------------------------------------------------- */

typedef enum {
    XISF_OK = 0,
    /* The file does not begin with the XISF signature. */
    XISF_ERR_NOT_XISF = 1,
    /* The file is shorter than its own structure claims. */
    XISF_ERR_TRUNCATED = 2,
    /* The XML header could not be parsed. */
    XISF_ERR_BAD_HEADER = 3,
    /* An attribute does not match the grammar the specification gives it. */
    XISF_ERR_BAD_ATTRIBUTE = 4,
    /* Well-formed, but not something this build supports -- a codec left out
     * at compile time, or a feature of a later specification. */
    XISF_ERR_UNSUPPORTED = 5,
    /* A recorded checksum did not match the stored bytes. */
    XISF_ERR_CHECKSUM_MISMATCH = 6,
    /* Decompression failed. */
    XISF_ERR_COMPRESSION = 7,
    /* No such image, property or block. */
    XISF_ERR_NOT_FOUND = 8,
    /* A NULL handle, an out-of-range index, or a buffer too small. */
    XISF_ERR_INVALID_ARGUMENT = 9,
    /* An operating system error. */
    XISF_ERR_IO = 10,
    /* Allocation failed. */
    XISF_ERR_OUT_OF_MEMORY = 11
} xisf_error_t;

/*
 * A human-readable description of an error code.
 *
 * The returned string is static and must not be freed.  An unrecognised code
 * yields "unknown error" rather than NULL, so the result is always printable.
 */
const char *xisf_error_message(xisf_error_t error);

/* ---- Sample formats ------------------------------------------------ */

typedef enum {
    XISF_SAMPLE_UINT8 = 0,
    XISF_SAMPLE_UINT16 = 1,
    XISF_SAMPLE_UINT32 = 2,
    XISF_SAMPLE_UINT64 = 3,
    XISF_SAMPLE_FLOAT32 = 4,
    XISF_SAMPLE_FLOAT64 = 5,
    XISF_SAMPLE_COMPLEX32 = 6,
    XISF_SAMPLE_COMPLEX64 = 7
} xisf_sample_format_t;

/* Bytes per sample. Zero for an unrecognised format. */
size_t xisf_sample_format_size(xisf_sample_format_t format);

/* The specification's name for a format, e.g. "Float32". Never NULL. */
const char *xisf_sample_format_name(xisf_sample_format_t format);

typedef enum {
    XISF_COLOR_GRAY = 0,
    XISF_COLOR_RGB = 1,
    XISF_COLOR_CIELAB = 2
} xisf_color_space_t;

typedef enum {
    XISF_BYTE_ORDER_LITTLE = 0,
    XISF_BYTE_ORDER_BIG = 1
} xisf_byte_order_t;

/* ---- Files --------------------------------------------------------- */

/* An open XISF file. Opaque; obtain one with xisf_open(). */
typedef struct xisf_file xisf_file_t;

/* One image within a file. Opaque, borrowed, and valid only while the file
 * that produced it is open. Never freed directly. */
typedef struct xisf_image xisf_image_t;

/*
 * Open a monolithic XISF file.
 *
 * The file is memory-mapped, so its pixel data costs nothing until read.
 * Returns NULL on failure, setting *err.
 */
xisf_file_t *xisf_open(const char *path, xisf_error_t *err);

/*
 * Open an XISF file already in memory.
 *
 * The bytes are copied, so `data` need not outlive the call.
 */
xisf_file_t *xisf_open_memory(const void *data, size_t size, xisf_error_t *err);

/* Close a file and release everything it owns. NULL is accepted and ignored.
 * Every xisf_image_t obtained from it becomes invalid. */
void xisf_close(xisf_file_t *file);

/* How many images the file holds. Zero for a NULL handle. */
size_t xisf_image_count(const xisf_file_t *file);

/*
 * Borrow an image by index.
 *
 * The result belongs to the file and must not be freed.  Returns NULL if the
 * handle is NULL or the index is out of range.
 */
const xisf_image_t *xisf_image_at(const xisf_file_t *file, size_t index);

/* ---- Image geometry ------------------------------------------------ */

/* How many dimensions the image has, not counting channels. Usually 2. */
size_t xisf_image_dimension_count(const xisf_image_t *image);

/* One dimension, fastest-varying first. Zero if the index is out of range. */
uint64_t xisf_image_dimension(const xisf_image_t *image, size_t index);

/* Convenience for the common two-dimensional case. Zero if absent. */
uint64_t xisf_image_width(const xisf_image_t *image);
uint64_t xisf_image_height(const xisf_image_t *image);

/* The number of channels. */
uint64_t xisf_image_channels(const xisf_image_t *image);

xisf_sample_format_t xisf_image_sample_format(const xisf_image_t *image);
xisf_color_space_t xisf_image_color_space(const xisf_image_t *image);

/* The byte order the samples are stored in. Little-endian when the file does
 * not say, which is the format's default rather than the host's order. */
xisf_byte_order_t xisf_image_byte_order(const xisf_image_t *image);

/* Whether the pixel data is stored compressed. */
int xisf_image_is_compressed(const xisf_image_t *image);

/* The size of the image's pixel data in bytes. Zero if the geometry overflows. */
uint64_t xisf_image_data_size(const xisf_image_t *image);

/* ---- Reading pixels ------------------------------------------------ */

/*
 * Copy the image's pixel data into a buffer you provide.
 *
 * `size` is the buffer's capacity; it must be at least
 * xisf_image_data_size(), or XISF_ERR_INVALID_ARGUMENT is returned and
 * nothing is written.  Samples arrive in the byte order the file stored them
 * in -- see xisf_image_byte_order() -- and are not converted.
 */
xisf_error_t xisf_image_read(const xisf_image_t *image, void *buffer, size_t size);

/*
 * Read the image's pixel data into a buffer this library allocates.
 *
 * On success writes the size to *size and returns the buffer, which you
 * release with xisf_free().  Returns NULL on failure, setting *err.  `size`
 * may be NULL if you already know how big it will be.
 */
void *xisf_image_read_alloc(const xisf_image_t *image, size_t *size, xisf_error_t *err);

/*
 * Verify the image's recorded checksum.
 *
 * Returns XISF_OK if it matches or if the file records none -- the
 * specification makes checksums optional, so their absence is not a failure.
 * Returns XISF_ERR_CHECKSUM_MISMATCH if the stored bytes do not match.
 */
xisf_error_t xisf_image_verify(const xisf_image_t *image);

/* ---- Metadata ------------------------------------------------------ */

/* How many FITS keywords the image carries. */
size_t xisf_image_fits_keyword_count(const xisf_image_t *image);

/*
 * Borrow one FITS keyword's name, value or comment.
 *
 * The strings belong to the file, are NUL-terminated, and stay valid until it
 * is closed.  Returns NULL if the index is out of range.
 */
const char *xisf_image_fits_keyword_name(const xisf_image_t *image, size_t index);
const char *xisf_image_fits_keyword_value(const xisf_image_t *image, size_t index);
const char *xisf_image_fits_keyword_comment(const xisf_image_t *image, size_t index);

/* ---- Memory -------------------------------------------------------- */

/*
 * Release a buffer this library allocated for you.
 *
 * Use this for the result of xisf_image_read_alloc().  NULL is accepted and
 * ignored.  Do not pass handles here -- a file is closed with xisf_close().
 *
 * This exists so the allocator is not part of the ABI.  A library that tells
 * you to call free() can never change how it allocates, and cannot be used
 * across a Windows DLL boundary where the two sides link different C runtimes.
 */
void xisf_free(void *pointer);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* XISF_H */
