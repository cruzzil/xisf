//! Real C, compiled against the header and linked against the built library.
//!
//! The Rust-side tests call the entry points directly, which proves they
//! behave -- but not that the header *describes* them. A C compiler reading
//! `xisf.h` is the only thing that checks the declarations match the
//! definitions, that the enums have the values the header says, and that a
//! caller who follows the documented ownership rules does not leak or crash.
//!
//! Skips with a note when no C compiler is available, so a bare checkout stays
//! green; CI says which it got.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A C compiler whose ABI matches the one Rust is building for.
///
/// This has to match, not merely exist. On Windows the runners carry MinGW
/// `gcc` while Rust targets MSVC, and linking an MSVC `.lib` with MinGW's `ld`
/// fails on `__chkstk` and every Windows import symbol -- the two runtimes do
/// not mix. Picking the wrong one produces a wall of undefined references that
/// looks like a bug in the library and is not.
fn compiler() -> Option<String> {
    let candidates: &[&str] =
        if cfg!(target_env = "msvc") { &["cl"] } else { &["cc", "gcc", "clang"] };

    for candidate in candidates {
        // `cl` prints its banner to stderr and exits non-zero with no input,
        // so its presence is what is checked rather than its exit status.
        let ran = Command::new(candidate).arg("--version").output();
        if let Ok(output) = ran
            && (output.status.success() || cfg!(target_env = "msvc"))
        {
            return Some((*candidate).to_string());
        }
    }
    None
}

/// Whether the compiler above takes MSVC-style arguments.
fn msvc_style() -> bool {
    cfg!(target_env = "msvc")
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The directory holding the built `libxisf`.
///
/// `cargo test` builds the test binary but does not necessarily build the
/// cdylib, so this asks cargo for it first. Without that a stale library can
/// be linked and the gate reports on code that is no longer there.
fn library_dir() -> Option<PathBuf> {
    let status = Command::new(env!("CARGO"))
        .args(["build", "-p", "libxisf", "--lib"])
        .current_dir(manifest_dir())
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }

    // target/<profile>/, found by walking up from the test executable.
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?;
    if dir.ends_with("deps") {
        dir = dir.parent()?;
    }
    Some(dir.to_path_buf())
}

/// Compile `source` against the header, link it, run it with `args`, and
/// return its output.
fn run_c_program(name: &str, source: &str, args: &[&Path]) -> Result<String, String> {
    let Some(cc) = compiler() else {
        return Err("SKIP: no C compiler".into());
    };
    let Some(lib_dir) = library_dir() else {
        return Err("SKIP: could not build the library".into());
    };

    let dir = std::env::temp_dir().join(format!("xisf-abi-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let source_path = dir.join(format!("{name}.c"));
    let binary = dir.join(name);
    std::fs::write(&source_path, source).map_err(|e| e.to_string())?;

    let mut command = Command::new(&cc);
    if msvc_style() {
        command
            .arg("/std:c11")
            .arg("/W4")
            .arg("/WX")
            .arg("/nologo")
            .arg(format!("/I{}", manifest_dir().join("include").display()))
            .arg(&source_path)
            .arg(format!("/Fe:{}", binary.display()))
            .arg(lib_dir.join("xisf.lib"))
            // What Rust's Windows target links against; a staticlib does not
            // carry these itself.
            .args(["ws2_32.lib", "userenv.lib", "ntdll.lib", "bcrypt.lib", "advapi32.lib"]);
    } else {
        command
            .arg("-std=c11")
            .arg("-Wall")
            .arg("-Wextra")
            .arg("-Werror")
            .arg("-I")
            .arg(manifest_dir().join("include"))
            .arg(&source_path)
            .arg("-o")
            .arg(&binary)
            .arg("-L")
            .arg(&lib_dir)
            .arg("-lxisf");
    }
    let output = command.output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "compiling {name} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let run = Command::new(&binary)
        .args(args)
        .env("LD_LIBRARY_PATH", &lib_dir)
        .env("DYLD_LIBRARY_PATH", &lib_dir)
        .output()
        .map_err(|e| e.to_string())?;

    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    if !run.status.success() {
        return Err(format!(
            "{name} exited with {:?}\nstdout:\n{stdout}\nstderr:\n{}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        ));
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(stdout)
}

/// Write a file with the engine, so the C program has something real to read.
fn write_sample(path: &Path) {
    use xisf_core::image::{ColorSpace, Image, PixelStorage, SampleFormat};
    use xisf_core::writer::{BlockOptions, PendingImage, Writer};

    let image = Image {
        dimensions: vec![9, 6],
        channels: 1,
        sample_format: SampleFormat::UInt16,
        color_space: ColorSpace::Gray,
        pixel_storage: PixelStorage::Planar,
        bounds: None,
        id: None,
        uuid: None,
        image_type: None,
    };
    let size = image.data_size().unwrap() as usize;
    let data: Vec<u8> = (0..size).map(|i| (i * 11 + 3) as u8).collect();

    let mut writer = Writer::new();
    writer
        .add_image(PendingImage {
            image,
            data,
            options: BlockOptions {
                compression: None,
                checksum: Some(xisf_core::block::ChecksumAlgorithm::Sha256),
            },
        })
        .unwrap();
    std::fs::write(path, writer.to_bytes().unwrap()).unwrap();
}

/// A C program that uses the API the way the header says to.
#[test]
fn a_c_program_can_read_a_file() {
    let dir = std::env::temp_dir().join(format!("xisf-abi-data-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sample = dir.join("sample.xisf");
    write_sample(&sample);

    let source = r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <xisf.h>

int main(int argc, char **argv) {
    if (argc != 2) { fprintf(stderr, "usage: reader <file>\n"); return 2; }

    xisf_error_t err = (xisf_error_t)-1;
    xisf_file_t *file = xisf_open(argv[1], &err);
    if (!file) {
        fprintf(stderr, "open failed: %s\n", xisf_error_message(err));
        return 1;
    }
    if (err != XISF_OK) { fprintf(stderr, "err not reset\n"); return 1; }

    if (xisf_image_count(file) != 1) { fprintf(stderr, "wrong image count\n"); return 1; }
    const xisf_image_t *image = xisf_image_at(file, 0);
    if (!image) { fprintf(stderr, "no image\n"); return 1; }

    printf("geometry %llu x %llu x %llu\n",
           (unsigned long long)xisf_image_width(image),
           (unsigned long long)xisf_image_height(image),
           (unsigned long long)xisf_image_channels(image));
    printf("format %s (%zu bytes per sample)\n",
           xisf_sample_format_name(xisf_image_sample_format(image)),
           xisf_sample_format_size(xisf_image_sample_format(image)));

    if (xisf_image_verify(image) != XISF_OK) {
        fprintf(stderr, "checksum did not verify\n");
        return 1;
    }

    /* The documented ownership rule: what the library allocates, xisf_free
       releases -- not the C library's free(). */
    size_t size = 0;
    void *pixels = xisf_image_read_alloc(image, &size, &err);
    if (!pixels || err != XISF_OK) {
        fprintf(stderr, "read_alloc failed: %s\n", xisf_error_message(err));
        return 1;
    }
    if (size != xisf_image_data_size(image)) {
        fprintf(stderr, "size disagrees with data_size\n");
        return 1;
    }

    /* A real caller casts to the sample type; this must be aligned for it. */
    const uint16_t *samples = (const uint16_t *)pixels;
    printf("first sample %u\n", (unsigned)samples[0]);
    printf("bytes %zu\n", size);

    /* And the caller-provided-buffer form must agree with it. */
    unsigned char *mine = malloc(size);
    if (!mine) { return 1; }
    if (xisf_image_read(image, mine, size) != XISF_OK) {
        fprintf(stderr, "read into buffer failed\n");
        return 1;
    }
    if (memcmp(mine, pixels, size) != 0) {
        fprintf(stderr, "the two read paths disagree\n");
        return 1;
    }
    free(mine);

    /* A short buffer is refused. */
    unsigned char small[4];
    if (xisf_image_read(image, small, sizeof small) != XISF_ERR_INVALID_ARGUMENT) {
        fprintf(stderr, "a short buffer was accepted\n");
        return 1;
    }

    xisf_free(pixels);
    xisf_close(file);

    /* Both destructors accept NULL, as the header promises. */
    xisf_free(NULL);
    xisf_close(NULL);

    printf("OK\n");
    return 0;
}
"#;

    match run_c_program("reader", source, &[&sample]) {
        Err(e) if e.starts_with("SKIP") => eprintln!("skipping: {e}"),
        Err(e) => panic!("{e}"),
        Ok(output) => {
            eprintln!("{output}");
            assert!(output.contains("geometry 9 x 6 x 1"), "wrong geometry:\n{output}");
            assert!(output.contains("format UInt16 (2 bytes per sample)"), "wrong format");
            assert!(output.contains("bytes 108"), "9 * 6 * 2 = 108");
            assert!(output.trim().ends_with("OK"), "the program did not finish");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The header must be usable from C++ too, since that is half of who reads a
/// C API. This also proves the `extern "C"` guard is present and balanced.
#[test]
fn the_header_compiles_as_cplusplus() {
    // Syntax-only, so any C++ compiler will do regardless of which runtime
    // Rust is targeting -- nothing is linked.
    let cxx_name = if cfg!(target_env = "msvc") { "cl" } else { "c++" };
    let Ok(probe) = Command::new(cxx_name).arg("--version").output() else {
        eprintln!("skipping: no C++ compiler");
        return;
    };
    if !probe.status.success() && !cfg!(target_env = "msvc") {
        eprintln!("skipping: no C++ compiler");
        return;
    }

    let dir = std::env::temp_dir().join(format!("xisf-abi-cxx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("header.cpp");
    std::fs::write(&source, "#include <xisf.h>\nint main() { return xisf_version() ? 0 : 1; }\n")
        .unwrap();

    let mut command = Command::new(cxx_name);
    if cfg!(target_env = "msvc") {
        command
            .args(["/std:c++17", "/W4", "/WX", "/nologo", "/Zs"])
            .arg(format!("/I{}", manifest_dir().join("include").display()))
            .arg(&source);
    } else {
        command
            .args(["-std=c++17", "-Wall", "-Wextra", "-Werror", "-fsyntax-only"])
            .arg("-I")
            .arg(manifest_dir().join("include"))
            .arg(&source);
    }
    let output = command.output().expect("run the C++ compiler");

    assert!(
        output.status.success(),
        "the header does not compile as C++:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every function the header declares must exist in the built library.
///
/// Read out of the header rather than from a list kept by hand, so a
/// declaration added without an implementation fails here rather than at some
/// caller's link step.
#[test]
fn every_declared_function_is_defined() {
    let Some(lib_dir) = library_dir() else {
        eprintln!("skipping: could not build the library");
        return;
    };

    let header = std::fs::read_to_string(manifest_dir().join("include/xisf.h")).expect("xisf.h");
    let mut declared: Vec<String> = Vec::new();
    for line in header.lines() {
        let line = line.trim();
        // Declarations are the lines naming `xisf_...(` outside a comment.
        if line.starts_with('*') || line.starts_with("/*") || line.starts_with("//") {
            continue;
        }
        // The name is the identifier immediately before a `(`, not the first
        // `xisf_` on the line: `xisf_file_t *xisf_open(...)` starts with the
        // return type.
        for (paren, _) in line.match_indices('(') {
            let before = &line[..paren];
            let start = before
                .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .map_or(0, |i| i + 1);
            let name = &before[start..];
            if name.starts_with("xisf_") && !declared.contains(&name.to_string()) {
                declared.push(name.to_string());
            }
        }
    }
    // A count alone is a weak check on the scan, so name a few that must be
    // there. If the scan breaks, these go missing before the count looks odd.
    for expected in [
        "xisf_open",
        "xisf_close",
        "xisf_image_at",
        "xisf_image_read",
        "xisf_image_read_alloc",
        "xisf_free",
        "xisf_error_message",
    ] {
        assert!(
            declared.iter().any(|d| d == expected),
            "the header scan missed {expected}; it found {declared:?}"
        );
    }

    // `nm` is not everywhere, so fall back to scanning the static library.
    let lib = lib_dir.join(if cfg!(target_os = "windows") { "xisf.lib" } else { "libxisf.a" });
    let Ok(bytes) = std::fs::read(&lib) else {
        eprintln!("skipping: {} not built", lib.display());
        return;
    };

    let mut missing = Vec::new();
    for name in &declared {
        if !contains(&bytes, name.as_bytes()) {
            missing.push(name.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "{} of {} declared functions are missing from the library: {missing:?}",
        missing.len(),
        declared.len()
    );
    eprintln!("all {} declared functions are present", declared.len());
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
