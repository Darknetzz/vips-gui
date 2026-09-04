//! Capability detection against the real `vips -l` output of the installed
//! build, and the gating behaviour that depends on it.

mod common;

use vips_gui::settings::{ConversionSettings, OutputFormat};
use vips_gui::ui::settings_panel::has_saver;
use vips_gui::vips::discovery::Capabilities;

/// Capture the operation listing from the installed libvips.
fn real_listing(install: &vips_gui::vips::VipsInstall) -> String {
    let output = install
        .command()
        .arg("-l")
        .output()
        .expect("vips -l should run");
    let mut listing = String::from_utf8_lossy(&output.stdout).into_owned();
    if listing.trim().is_empty() {
        listing = String::from_utf8_lossy(&output.stderr).into_owned();
    }
    assert!(!listing.trim().is_empty(), "vips -l produced no output");
    listing
}

#[test]
fn parsed_capabilities_match_what_the_savers_can_actually_do() {
    let Some(install) = common::vips() else {
        common::skip("parsed_capabilities_match_what_the_savers_can_actually_do");
        return;
    };

    let listing = real_listing(install);
    let parsed = Capabilities::parse_listing(&listing);

    // Cross-check the parse against discovery's own probe.
    assert_eq!(
        parsed, install.capabilities,
        "parsing the listing directly should agree with the discovery probe"
    );

    // And cross-check each flag against a real conversion attempt, which is
    // the only definitive answer.
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    common::make_source_image(install, &source, 40, 30);

    for format in OutputFormat::ALL {
        let claimed = has_saver(parsed, format);
        let target = dir.path().join(format!("out.{}", format.extension()));

        let mut settings = ConversionSettings::default();
        settings.format = format;
        settings.avif.effort = 0;

        let output = install
            .command()
            .args(
                &vips_gui::vips::command::build(&settings, &source, &target, install.version).0,
            )
            .output()
            .expect("vips should run");

        let actually_worked = output.status.success() && target.is_file();
        assert_eq!(
            claimed, actually_worked,
            "capabilities claim {} is {} but converting {} \
             (stderr: {})",
            format.label(),
            if claimed { "available" } else { "unavailable" },
            if actually_worked { "worked" } else { "failed" },
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
}

#[test]
fn removing_heifsave_from_a_real_listing_disables_avif() {
    let Some(install) = common::vips() else {
        common::skip("removing_heifsave_from_a_real_listing_disables_avif");
        return;
    };

    let listing = real_listing(install);
    let full = Capabilities::parse_listing(&listing);

    // Simulate a build compiled without libheif by dropping its saver lines
    // from genuine output, rather than hand-writing a fake listing.
    let without_heif: String = listing
        .lines()
        .filter(|line| !line.contains("heifsave"))
        .collect::<Vec<_>>()
        .join("\n");

    let reduced = Capabilities::parse_listing(&without_heif);

    assert!(
        !reduced.avif,
        "AVIF should be reported unavailable once heifsave is gone"
    );
    assert!(!reduced.all_present());
    // The other formats must be unaffected.
    assert_eq!(reduced.jpeg, full.jpeg);
    assert_eq!(reduced.png, full.png);
    assert_eq!(reduced.webp, full.webp);
}

#[test]
fn a_build_without_heifsave_makes_the_app_pick_a_different_format() {
    let Some(install) = common::vips() else {
        common::skip("a_build_without_heifsave_makes_the_app_pick_a_different_format");
        return;
    };

    let listing = real_listing(install);
    let without_heif: String = listing
        .lines()
        .filter(|line| !line.contains("heifsave"))
        .collect::<Vec<_>>()
        .join("\n");
    let reduced = Capabilities::parse_listing(&without_heif);

    // A user whose last session used AVIF, opening the app against this build.
    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Avif;

    let abandoned = settings.ensure_format_supported(|f| has_saver(reduced, f));

    assert_eq!(
        abandoned,
        Some(OutputFormat::Avif),
        "the app should report which format it gave up on"
    );
    assert!(
        has_saver(reduced, settings.format),
        "the replacement format must be one this build can write, got {}",
        settings.format.label()
    );
}

#[test]
fn every_format_is_available_on_a_complete_build() {
    let Some(install) = common::vips() else {
        common::skip("every_format_is_available_on_a_complete_build");
        return;
    };

    // This is a statement about the machine, not the code, so it explains
    // rather than fails when a format is genuinely absent.
    let caps = install.capabilities;
    if !caps.all_present() {
        eprintln!(
            "NOTE: this libvips build is missing savers (jpeg={} png={} webp={} avif={}); \
             gating will be exercised in the UI",
            caps.jpeg, caps.png, caps.webp, caps.avif
        );
        return;
    }

    for format in OutputFormat::ALL {
        assert!(
            has_saver(caps, format),
            "{} should be offered on a complete build",
            format.label()
        );
    }
}
