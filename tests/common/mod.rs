//! Shared helpers for the integration tests.
//!
//! These tests drive a real `vips` binary. On a machine without libvips they
//! report why they are skipping and pass, so the suite stays green on a bare
//! checkout rather than failing for an environmental reason.
//!
//! Usage in a test:
//!
//! ```ignore
//! let Some(install) = common::vips() else { common::skip("my_test"); return; };
//! ```

#![allow(dead_code)] // Not every test binary uses every helper.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use vips_gui::vips::discovery::{self, VipsInstall};

/// Discover vips once per test binary. `None` means it is not installed.
pub fn vips() -> Option<&'static VipsInstall> {
    static INSTALL: OnceLock<Option<VipsInstall>> = OnceLock::new();
    INSTALL.get_or_init(|| discovery::discover(None).ok()).as_ref()
}

/// Explain why a test did nothing, so a skip is never silent.
pub fn skip(test_name: &str) {
    eprintln!(
        "SKIPPING {test_name}: no usable libvips found. \
         Install libvips and put its bin directory on PATH to run this test."
    );
}

/// Generate a test image with vips itself, so no binary fixtures need to be
/// committed to the repository.
///
/// The result is a three-band sRGB image of independent random noise per
/// channel. Both properties matter:
///
/// - Noise is incompressible, so quality settings visibly change the output
///   size. A flat colour would compress to nearly the same size at every
///   quality and would prove nothing.
/// - Independent channels give real chroma variation, without which chroma
///   subsampling is a no-op and cannot be tested.
pub fn make_source_image(install: &VipsInstall, path: &Path, width: u32, height: u32) {
    let scratch = path.parent().unwrap_or(Path::new("."));
    let tag = path.file_stem().unwrap_or_default().to_string_lossy();

    // One noise plane per channel, each with its own seed.
    let planes: Vec<PathBuf> = (0..3)
        .map(|i| scratch.join(format!("{tag}-plane{i}.png")))
        .collect();
    for (i, plane) in planes.iter().enumerate() {
        run_ok(
            install,
            &[
                "gaussnoise",
                &plane.to_string_lossy(),
                &width.to_string(),
                &height.to_string(),
                "--seed",
                &(i as i32 + 1).to_string(),
            ],
        );
    }

    // Join them into one three-band image. `bandjoin` takes its array of
    // inputs as a single space-separated argument.
    //
    // Backslashes must not appear inside that argument: libvips treats `\` as
    // an escape character while parsing an image array, so a Windows path
    // arrives with its separators eaten (`C:UsersKriss...`). Forward slashes
    // are accepted on Windows and sidestep the problem entirely.
    let joined = scratch.join(format!("{tag}-joined.v"));
    let plane_list = planes
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect::<Vec<_>>()
        .join(" ");
    run_ok(
        install,
        &["bandjoin", &plane_list, &joined.to_string_lossy()],
    );

    // bandjoin leaves the interpretation as B_W. Saving a three-band B_W
    // image to PNG makes libvips treat band 0 as grey and promote to RGBA,
    // giving a four-band file. Tagging it sRGB keeps it a clean RGB image.
    run_ok(
        install,
        &[
            "copy",
            &joined.to_string_lossy(),
            &path.to_string_lossy(),
            "--interpretation",
            "srgb",
        ],
    );

    for temp in planes.iter().chain(std::iter::once(&joined)) {
        let _ = std::fs::remove_file(temp);
    }
    assert!(path.is_file(), "fixture {} was not created", path.display());
}

/// Run a vips subcommand and assert it succeeded.
pub fn run_ok(install: &VipsInstall, args: &[&str]) {
    let output = install
        .command()
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn vips {args:?}: {e}"));
    assert!(
        output.status.success(),
        "vips {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Read an image's pixel dimensions by asking `vipsheader`.
///
/// `vipsheader` ships alongside `vips` in every distribution, so derive its
/// path from the discovered executable rather than assuming it is on PATH.
pub fn image_size(install: &VipsInstall, path: &Path) -> (u32, u32) {
    (
        header_field(install, path, "width"),
        header_field(install, path, "height"),
    )
}

/// Read one numeric header field via `vipsheader -f <field>`.
fn header_field(install: &VipsInstall, path: &Path, field: &str) -> u32 {
    let exe = vipsheader_path(install);
    let mut cmd = std::process::Command::new(&exe);
    vips_gui::vips::hide_console(&mut cmd);
    let output = cmd
        .args(["-f", field, &path.to_string_lossy()])
        .output()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", exe.display()));
    assert!(
        output.status.success(),
        "vipsheader -f {field} failed for {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("{field} was not a number: {e}"))
}

/// `vipsheader` next to the discovered `vips`.
fn vipsheader_path(install: &VipsInstall) -> PathBuf {
    let name = if cfg!(windows) {
        "vipsheader.exe"
    } else {
        "vipsheader"
    };
    install
        .exe
        .parent()
        .map(|dir| dir.join(name))
        .unwrap_or_else(|| PathBuf::from(name))
}
