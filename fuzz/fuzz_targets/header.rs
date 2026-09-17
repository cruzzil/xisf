//! The XML header parser, over arbitrary text.
//!
//! The narrowest target of the four: no I/O, no codecs, no block addressing,
//! so anything it finds is in the parser itself. It is also the fastest, which
//! matters because the structural limits -- depth, elements, attributes -- are
//! only reachable by a fuzzer that gets to run a great many iterations.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = core::str::from_utf8(data) else { return };
    let Ok(header) = xisf_core::header::parse(text) else { return };

    // Parsing is half of it. A header that parses is then walked, and the
    // walks have their own arithmetic: `descendants` is iterative over a tree
    // whose depth the parser bounded, and `associated` follows uid references.
    for element in header.root.descendants() {
        let _ = element.attr("uid");
        let _ = header.associated(element, "Resolution");
        let _ = header.associated(element, "Thumbnail");
    }
    for image in header.images() {
        let _ = xisf_core::image::Image::parse(image);
    }
});
