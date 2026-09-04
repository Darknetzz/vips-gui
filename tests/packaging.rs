//! Packaging checks against the built release binary.
//!
//! These verify the promise the project makes: one `.exe`, no DLLs of its own,
//! runnable from any folder. They skip with an explanation when no release
//! build exists, so `cargo test` still works after a plain `cargo build`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Locate `target/release/vips-gui.exe` relative to this test binary.
///
/// `CARGO_MANIFEST_DIR` is the crate root, which is stable regardless of where
/// the test is invoked from.
fn release_binary() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "vips-gui.exe"
    } else {
        "vips-gui"
    };
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("release")
        .join(name);
    path.is_file().then_some(path)
}

/// Explain a skip so it is never silent.
fn skip_no_release(test: &str) {
    eprintln!(
        "SKIPPING {test}: no release build found. \
         Run `cargo build --release` first."
    );
}

#[test]
fn the_release_binary_is_a_single_self_contained_file() {
    let Some(exe) = release_binary() else {
        skip_no_release("the_release_binary_is_a_single_self_contained_file");
        return;
    };

    let size = std::fs::metadata(&exe).expect("stat the exe").len();
    assert!(size > 1_000_000, "a {size} byte exe looks wrong");
    // A generous ceiling: this is not a size budget, just a tripwire for
    // accidentally embedding something large.
    assert!(
        size < 80 * 1024 * 1024,
        "the exe has grown to {size} bytes; has something large been embedded?"
    );

    // Nothing else should be needed alongside it. The release directory holds
    // plenty of build artefacts, so rather than checking the directory is
    // clean, assert the exe carries no non-system imports.
    let bytes = std::fs::read(&exe).expect("read the exe");

    // Import names appear as plain ASCII in the PE import table, so a byte
    // search is a reliable check without a PE parser.
    for forbidden in [
        // The Visual C++ Redistributable. Statically linking the CRT is what
        // lets the binary run on a machine without it installed.
        "vcruntime140.dll",
        "vcruntime140_1.dll",
        "msvcp140.dll",
        "msvcr120.dll",
        // The Universal CRT forwarders, which appear when the CRT is linked
        // dynamically.
        "api-ms-win-crt-runtime",
        "api-ms-win-crt-heap",
        "api-ms-win-crt-stdio",
        // libvips must be a subprocess, never a linked library.
        "libvips-42.dll",
        "libvips.dll",
        "libglib-2.0-0.dll",
    ] {
        assert!(
            !contains_ascii(&bytes, forbidden),
            "the release binary imports {forbidden}, which breaks the \
             single-file promise"
        );
    }
}

/// Case-insensitive search for an ASCII needle in a byte haystack.
fn contains_ascii(haystack: &[u8], needle: &str) -> bool {
    let needle = needle.to_ascii_lowercase().into_bytes();
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window.to_ascii_lowercase() == needle)
}

#[test]
fn the_release_binary_starts_and_reports_its_version() {
    let Some(exe) = release_binary() else {
        skip_no_release("the_release_binary_starts_and_reports_its_version");
        return;
    };

    let output = Command::new(&exe)
        .arg("--version")
        .output()
        .expect("the release binary should run");

    assert!(
        output.status.success(),
        "--version exited with {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("vips-gui"),
        "--version should name the program, got {stdout:?}"
    );
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "--version should report {}, got {stdout:?}",
        env!("CARGO_PKG_VERSION")
    );
}

#[test]
fn the_release_binary_runs_from_an_unrelated_working_directory() {
    let Some(exe) = release_binary() else {
        skip_no_release("the_release_binary_runs_from_an_unrelated_working_directory");
        return;
    };

    // An empty directory with nothing of ours in it. If the binary needed a
    // sibling DLL or asset, this is where it would fail.
    let elsewhere = tempfile::tempdir().expect("temp dir");

    let output = Command::new(&exe)
        .arg("--check")
        .current_dir(elsewhere.path())
        .output()
        .expect("the release binary should run from anywhere");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // It must produce its report either way; the exit code depends only on
    // whether libvips is installed on this machine.
    assert!(
        stdout.contains("vips-gui"),
        "expected a report from --check.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("libvips:"),
        "the report should describe the libvips situation.\nstdout: {stdout}"
    );

    match output.status.code() {
        Some(0) => {
            // libvips was found, so the report should name a version and the
            // formats it can write.
            assert!(
                stdout.contains("Output formats:"),
                "a successful check should list the formats.\nstdout: {stdout}"
            );
            assert!(
                stdout.contains("Previews:"),
                "a successful check should report the preview mode.\nstdout: {stdout}"
            );
        }
        Some(1) => {
            // No libvips: the report must explain how to install it rather
            // than just saying no.
            assert!(
                stdout.contains("NOT USABLE"),
                "a failed check should say so plainly.\nstdout: {stdout}"
            );
            assert!(
                stdout.contains("github.com/libvips"),
                "a failed check should link to the download.\nstdout: {stdout}"
            );
        }
        other => panic!("unexpected exit code {other:?}.\nstdout: {stdout}\nstderr: {stderr}"),
    }
}

#[test]
fn the_check_flag_agrees_with_the_libraries_own_discovery() {
    let Some(exe) = release_binary() else {
        skip_no_release("the_check_flag_agrees_with_the_libraries_own_discovery");
        return;
    };

    let output = Command::new(&exe)
        .arg("--check")
        .output()
        .expect("the release binary should run");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Whatever the binary reports must match what the library finds in this
    // test process, since they run the same discovery code.
    match vips_gui::vips::discovery::discover(None) {
        Ok(install) => {
            assert_eq!(output.status.code(), Some(0), "check should have succeeded");
            assert!(
                stdout.contains(&install.version.to_string()),
                "the report should name version {}, got:\n{stdout}",
                install.version
            );
            assert!(
                stdout.contains(&install.exe.display().to_string()),
                "the report should name the executable it found, got:\n{stdout}"
            );
        }
        Err(_) => {
            assert_eq!(
                output.status.code(),
                Some(1),
                "check should have failed when discovery fails"
            );
        }
    }
}

#[test]
fn help_output_documents_the_libvips_prerequisite() {
    let Some(exe) = release_binary() else {
        skip_no_release("help_output_documents_the_libvips_prerequisite");
        return;
    };

    let output = Command::new(&exe)
        .arg("--help")
        .output()
        .expect("the release binary should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("libvips must be installed"),
        "help should state the prerequisite, got:\n{stdout}"
    );
    assert!(stdout.contains("--check"), "help should mention --check");
}

#[test]
fn the_readme_documents_the_libvips_prerequisite() {
    let readme = Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md");
    let text = std::fs::read_to_string(&readme).expect("README.md should exist");

    // The single most important thing a reader needs to know.
    assert!(
        text.contains("libvips must be installed separately")
            || text.contains("**libvips must be installed separately**"),
        "the README must state that libvips is a prerequisite"
    );
    assert!(
        text.contains("github.com/libvips/libvips/releases"),
        "the README should link to the libvips download"
    );
    assert!(
        text.contains("PATH"),
        "the README should explain the PATH step"
    );
    assert!(
        text.contains("8.13"),
        "the README should state the minimum libvips version"
    );
    assert!(
        text.contains("VIPS_GUI_VIPS_EXE"),
        "the README should document the environment override"
    );
}

#[cfg(windows)]
#[test]
fn the_release_binary_carries_version_information() {
    let Some(exe) = release_binary() else {
        skip_no_release("the_release_binary_carries_version_information");
        return;
    };

    // The version resource is embedded by build.rs. Rather than parsing the PE
    // resource directory, check the strings the resource script puts there;
    // they are stored as UTF-16 in the .rsrc section.
    let bytes = std::fs::read(&exe).expect("read the exe");
    let utf16_needle = |s: &str| -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    };

    for expected in ["vips-gui", "Batch image conversion with libvips"] {
        let needle = utf16_needle(expected);
        let found = bytes
            .windows(needle.len())
            .any(|window| window == needle.as_slice());
        assert!(
            found,
            "the version resource should contain {expected:?}; \
             did build.rs compile assets/vips-gui.rc?"
        );
    }
}

#[cfg(windows)]
#[test]
fn the_release_binary_is_a_gui_application_with_no_console() {
    let Some(exe) = release_binary() else {
        skip_no_release("the_release_binary_is_a_gui_application_with_no_console");
        return;
    };

    // Read the PE subsystem field. 2 = GUI, 3 = console. A console subsystem
    // would flash a black window every launch.
    let bytes = std::fs::read(&exe).expect("read the exe");
    assert_eq!(&bytes[0..2], b"MZ", "not a PE file");

    // e_lfanew at 0x3C points to the PE signature.
    let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
    assert_eq!(&bytes[pe_offset..pe_offset + 4], b"PE\0\0", "bad PE header");

    // COFF header is 20 bytes after the signature; the optional header follows.
    let optional_header = pe_offset + 4 + 20;
    let magic = u16::from_le_bytes(
        bytes[optional_header..optional_header + 2]
            .try_into()
            .unwrap(),
    );
    // 0x20b = PE32+ (64-bit), 0x10b = PE32.
    assert!(
        magic == 0x20b || magic == 0x10b,
        "unexpected optional header magic {magic:#x}"
    );

    // Subsystem sits at offset 68 in the optional header for both variants.
    let subsystem = u16::from_le_bytes(
        bytes[optional_header + 68..optional_header + 70]
            .try_into()
            .unwrap(),
    );
    assert_eq!(
        subsystem, 2,
        "expected the Windows GUI subsystem (2) so no console window appears, got {subsystem}"
    );
}
