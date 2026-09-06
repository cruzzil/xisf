//! Keeping Rust panics from crossing the C boundary.
//!
//! Unwinding out of an `extern "C"` function into C is undefined behaviour, so
//! every entry point runs its body inside [`guard`], which catches any panic,
//! reports it once, and returns the caller's fallback instead.
//!
//! This is a backstop rather than a design: a panic reaching here is a bug in
//! the engine, which returns `Result` on every error path it knows about. It
//! is reported to stderr on first occurrence so the bug is visible rather than
//! silently swallowed.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};

static REPORTED: AtomicBool = AtomicBool::new(false);

/// Run `body`, returning `fallback` if it panics.
pub(crate) fn guard<T>(what: &'static str, fallback: T, body: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(payload) => {
            report(what, &payload);
            fallback
        }
    }
}

fn report(what: &'static str, payload: &Box<dyn std::any::Any + Send>) {
    // Only the first is reported, so a caller looping over a broken file does
    // not flood stderr.
    if REPORTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic with no message".to_string());
    eprintln!("libxisf: BUG: {what} panicked: {message}");
}
