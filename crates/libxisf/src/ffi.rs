//! The handful of operations that genuinely need a raw pointer.
//!
//! A C ABI is a pointer ABI, so unsafe code cannot leave this crate -- but it
//! can be concentrated. Each judgement is made once here, and the entry points
//! are ordinary safe Rust over `Option`.
//!
//! # Out-parameters use `ptr::write`, not `Option<&mut T>`
//!
//! `Option<&T>` and `Option<&mut T>` are ABI-identical to `*const T` and
//! `*mut T`, so they look like the obvious signature, and for *inputs* they
//! are. Out-parameters are different: C calls these with an uninitialised
//! destination --
//!
//! ```c
//! xisf_error_t err;              /* uninitialised */
//! xisf_file_t *f = xisf_open("x.xisf", &err);
//! ```
//!
//! -- and a `&mut xisf_error_t` pointing at uninitialised memory is undefined
//! behaviour before anything is written through it, because a reference must
//! always point at a valid value of its type. [`std::ptr::write`] requires the
//! destination to be writable and aligned but *not* initialised, which is
//! exactly what a C out-parameter is.

use std::ffi::{CStr, c_char, c_void};

/// Write `value` through a C out-parameter, doing nothing if it is null.
///
/// # Safety
/// `out` must be null, or writable and aligned for a `T`. It need not be
/// initialised.
pub(crate) unsafe fn write_out<T>(out: *mut T, value: T) {
    if !out.is_null() {
        // SAFETY: non-null, and the caller guarantees it is writable and
        // aligned. `write` neither reads nor drops the previous contents,
        // which is what makes it correct over uninitialised storage.
        unsafe { out.write(value) };
    }
}

/// Borrow a handle, or `None` if it is null.
///
/// # Safety
/// `ptr` must be null or point at a live `T` that is not mutated for `'a`.
pub(crate) unsafe fn as_ref<'a, T>(ptr: *const T) -> Option<&'a T> {
    // SAFETY: `as_ref` is null-checked; the caller guarantees the rest.
    unsafe { ptr.as_ref() }
}

/// Borrow a C string argument, or `None` if it is null.
///
/// # Safety
/// `ptr` must be null or point at a NUL-terminated string that stays alive
/// and unmodified for `'a`.
pub(crate) unsafe fn c_str<'a>(ptr: *const c_char) -> Option<&'a CStr> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: non-null, and the caller guarantees termination and lifetime.
    Some(unsafe { CStr::from_ptr(ptr) })
}

/// A buffer handed to a C caller, released with `xisf_free`.
///
/// Rust's allocator needs the layout back at deallocation time, which C does
/// not have. Storing the length in a header immediately before the bytes the
/// caller sees keeps `xisf_free` a one-argument function, which is the shape
/// C expects.
///
/// The header is `usize`-aligned and the payload follows it, so the pointer
/// handed out is aligned for any type up to `align_of::<usize>()`. That is
/// enough for every XISF sample format: the widest is `Complex64`, two
/// `f64`s, whose alignment requirement is 8.
pub(crate) mod buffer {
    use std::alloc::{Layout, alloc, dealloc};

    /// Bytes reserved before the payload for its length.
    const HEADER: usize = size_of::<usize>();

    fn layout_for(len: usize) -> Option<Layout> {
        Layout::from_size_align(HEADER.checked_add(len)?, align_of::<usize>()).ok()
    }

    /// Allocate a buffer holding `bytes`, ready to hand to C.
    ///
    /// Returns null if allocation fails or the size is unrepresentable.
    pub(crate) fn allocate(bytes: &[u8]) -> *mut u8 {
        let Some(layout) = layout_for(bytes.len()) else {
            return std::ptr::null_mut();
        };

        // SAFETY: the layout has a non-zero size, since HEADER is non-zero.
        let base = unsafe { alloc(layout) };
        if base.is_null() {
            return std::ptr::null_mut();
        }

        // SAFETY: `base` is a fresh allocation of at least HEADER + len bytes,
        // aligned for `usize`, so the header write and the copy are both in
        // bounds and correctly aligned.
        unsafe {
            base.cast::<usize>().write(bytes.len());
            let payload = base.add(HEADER);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), payload, bytes.len());
            payload
        }
    }

    /// Release a buffer from [`allocate`].
    ///
    /// # Safety
    /// `payload` must be null, or a pointer returned by [`allocate`] and not
    /// yet released.
    pub(crate) unsafe fn release(payload: *mut u8) {
        if payload.is_null() {
            return;
        }
        // SAFETY: the caller guarantees this came from `allocate`, so the
        // header sits immediately before it and records the payload length.
        unsafe {
            let base = payload.sub(HEADER);
            let len = base.cast::<usize>().read();
            if let Some(layout) = layout_for(len) {
                dealloc(base, layout);
            }
        }
    }
}

/// Free a buffer previously handed to a caller.
///
/// # Safety
/// As [`buffer::release`].
pub(crate) unsafe fn free_buffer(pointer: *mut c_void) {
    unsafe { buffer::release(pointer.cast::<u8>()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_out_ignores_a_null_destination() {
        unsafe { write_out(std::ptr::null_mut::<u32>(), 7) };
    }

    #[test]
    fn write_out_does_not_read_the_previous_contents() {
        // The point of `ptr::write`: the destination starts uninitialised,
        // exactly as a C caller's `xisf_error_t err;` does.
        let mut slot = std::mem::MaybeUninit::<u32>::uninit();
        unsafe { write_out(slot.as_mut_ptr(), 42) };
        assert_eq!(unsafe { slot.assume_init() }, 42);
    }

    #[test]
    fn buffers_round_trip_and_keep_their_contents() {
        for len in [0usize, 1, 7, 8, 4096] {
            let data: Vec<u8> = (0..len).map(|i| (i * 13 + 1) as u8).collect();
            let ptr = buffer::allocate(&data);
            assert!(!ptr.is_null(), "allocation of {len} bytes failed");

            let seen = unsafe { std::slice::from_raw_parts(ptr, len) };
            assert_eq!(seen, &data[..], "the buffer's contents changed");

            unsafe { buffer::release(ptr) };
        }
    }

    /// The pointer handed to C is cast to the sample type and dereferenced,
    /// so it must be aligned for one. `Complex64` is the widest at 8 bytes.
    #[test]
    fn buffers_are_aligned_for_any_sample_type() {
        for len in [1usize, 3, 5, 9, 17] {
            let ptr = buffer::allocate(&vec![0u8; len]);
            assert!(!ptr.is_null());
            assert_eq!(ptr as usize % align_of::<u64>(), 0, "a {len}-byte buffer was misaligned");
            unsafe { buffer::release(ptr) };
        }
    }

    #[test]
    fn releasing_null_is_a_no_op() {
        unsafe { buffer::release(std::ptr::null_mut()) };
        unsafe { free_buffer(std::ptr::null_mut()) };
    }

    #[test]
    fn c_str_rejects_null() {
        assert!(unsafe { c_str(std::ptr::null()) }.is_none());
        assert_eq!(unsafe { c_str(c"hello".as_ptr()) }, Some(c"hello"));
    }
}
