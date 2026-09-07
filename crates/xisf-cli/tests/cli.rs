//! Drive the built binary the way a user would.
//!
//! These run the real executable rather than calling functions, because most
//! of what can go wrong in a command-line tool is in the parts a unit test
//! does not reach: argument handling, exit status, and what actually lands on
//! stdout. `dump` writing to stdout is the clearest case -- nothing but a
//! process check tells you whether the bytes came out intact.

use std::path::PathBuf;
use std::process::{Command, Output};

fn binary() -> PathBuf {
    // The test executable sits in target/<profile>/deps/, the binary beside it.
    let mut path = std::env::current_exe().expect("current exe");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join(if cfg!(windows) { "xisftool.exe" } else { "xisftool" })
}

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus")
}

fn run(args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("running {}: {e}; was the binary built?", binary().display()))
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn info_reports_the_geometry_of_a_real_file() {
    let path = corpus().join("pixinsight/Sample_F32_ZlibCompression_Sha256Security.xisf");
    let output = run(&["info", path.to_str().unwrap()]);
    assert!(output.status.success(), "info failed: {}", String::from_utf8_lossy(&output.stderr));

    let text = stdout(&output);
    assert!(text.contains("1 image(s)"), "{text}");
    assert!(text.contains("50 x 50, 3 channel(s), Float32, RGB"), "{text}");
    assert!(text.contains("compressed"), "the file is zlib-compressed:\n{text}");
    assert!(text.contains("bounds 0:1"), "{text}");
}

/// `--verbose` has to actually show more, or the flag is decoration.
#[test]
fn verbose_shows_fits_keywords_and_properties() {
    let path = corpus().join("pixinsight/Sample_F32_ZlibCompression_Sha256Security.xisf");
    let plain = stdout(&run(&["info", path.to_str().unwrap()]));
    let loud = stdout(&run(&["info", "-v", path.to_str().unwrap()]));

    assert!(loud.len() > plain.len(), "verbose output was no longer than plain");
    assert!(loud.contains("FITS SIMPLE"), "expected FITS keywords:\n{loud}");
    assert!(!plain.contains("FITS SIMPLE"), "plain output should not list keywords");
}

#[test]
fn header_prints_parsable_xml() {
    let path = corpus().join("generated/UInt8_Gray_none.xisf");
    let output = run(&["header", path.to_str().unwrap()]);
    assert!(output.status.success());

    let text = stdout(&output);
    assert!(text.contains("<xisf"), "{text}");
    assert!(text.contains("</xisf>"), "the header was truncated:\n{text}");
    assert!(text.contains("<Image"), "{text}");
}

#[test]
fn dump_writes_exactly_the_pixel_bytes() {
    let path = corpus().join("pixinsight/simple_10by10.xisf");
    let output = run(&["dump", path.to_str().unwrap()]);
    assert!(output.status.success());

    // 10 x 10 pixels, 3 channels, one byte per sample.
    assert_eq!(output.stdout.len(), 300, "dump wrote the wrong number of bytes");

    // And it must match what the library says, not merely be the right size.
    let file = xisf::XisfFile::open(&path).expect("open");
    let expected = file.images()[0].bytes().expect("bytes");
    assert_eq!(output.stdout, expected.to_vec());
}

/// A file with a good checksum passes; the exit status is what a script reads.
#[test]
fn verify_succeeds_on_an_intact_file() {
    let path = corpus().join("pixinsight/Sample_F32_ZlibCompression_Sha256Security.xisf");
    let output = run(&["verify", path.to_str().unwrap()]);
    assert!(output.status.success(), "{}", stdout(&output));
    assert!(stdout(&output).contains("1 verified"), "{}", stdout(&output));
}

/// A corrupted block must fail, and must fail *loudly* -- exit status included,
/// since that is what a caller checks.
#[test]
fn verify_fails_on_a_corrupted_block() {
    let path = corpus().join("pixinsight/Sample_F32_ZlibCompression_Sha256Security.xisf");
    let mut bytes = std::fs::read(&path).expect("read");

    // Flip a byte inside the image's own block, which has to be located
    // rather than guessed: this file carries more data after it, so a byte
    // near the end of the file is not covered by the checksum at all.
    let file = xisf::XisfFile::open(&path).expect("open");
    let images = file.images();
    let Some(xisf_core::block::Location::Attachment { position, size }) = images[0].location()
    else {
        panic!("expected an attached block");
    };
    assert!(*size > 0);
    let target = *position as usize + (*size as usize / 2);
    assert!(target < bytes.len());
    bytes[target] ^= 0xff;
    drop(file);

    let dir = std::env::temp_dir().join(format!("xisf-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let corrupted = dir.join("corrupted.xisf");
    std::fs::write(&corrupted, &bytes).unwrap();

    let output = run(&["verify", corrupted.to_str().unwrap()]);
    assert!(!output.status.success(), "a corrupted file verified successfully");
    assert!(stdout(&output).contains("CHECKSUM MISMATCH"), "{}", stdout(&output));

    let _ = std::fs::remove_dir_all(&dir);
}

/// A file with no checksums is not reported as verified: saying "ok" would
/// imply something had been checked.
#[test]
fn verify_distinguishes_absent_checksums_from_passing_ones() {
    let path = corpus().join("generated/UInt8_Gray_none.xisf");
    let output = run(&["verify", path.to_str().unwrap()]);
    assert!(output.status.success(), "an absent checksum is not a failure");

    let text = stdout(&output);
    assert!(text.contains("0 verified"), "nothing was verified:\n{text}");
    assert!(text.contains("1 without a checksum"), "{text}");
}

#[test]
fn bad_usage_is_reported_rather_than_ignored() {
    // A missing file.
    let missing = run(&["info", "/nonexistent/nope.xisf"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("xisf:"));

    // A command that does not exist.
    let unknown = run(&["frobnicate", "x"]);
    assert!(!unknown.status.success());

    // An option that does not exist.
    let bad_flag = run(&["info", "--nope", "x"]);
    assert!(!bad_flag.status.success());

    // A command with no file.
    let no_file = run(&["info"]);
    assert!(!no_file.status.success());

    // `--image` without a number.
    let no_index = run(&["dump", "--image"]);
    assert!(!no_index.status.success());
}

#[test]
fn help_and_version_work_without_a_file() {
    let help = run(&["--help"]);
    assert!(help.status.success());
    assert!(stdout(&help).contains("USAGE:"));

    let version = run(&["--version"]);
    assert!(version.status.success());
    assert!(stdout(&version).contains(env!("CARGO_PKG_VERSION")));
}

/// Several files at once, which is what `verify` is for.
#[test]
fn commands_accept_several_files() {
    let a = corpus().join("generated/UInt8_Gray_none.xisf");
    let b = corpus().join("generated/UInt16_Gray_zlib.xisf");
    let output = run(&["info", a.to_str().unwrap(), b.to_str().unwrap()]);
    assert!(output.status.success());

    let text = stdout(&output);
    assert!(text.contains("UInt8"), "{text}");
    assert!(text.contains("UInt16"), "{text}");
}
