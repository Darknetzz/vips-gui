//! Preview rendering against real vips.
//!
//! The claim that matters here is that the "predicted size" shown in the panel
//! is the size the file will actually be. These tests convert the same image
//! both ways and compare.

mod common;

use std::sync::atomic::AtomicBool;

use vips_gui::settings::{ConversionSettings, OutputDestination, OutputFormat};
use vips_gui::vips::convert::{self, Outcome};
use vips_gui::vips::preview::{self, PREVIEW_MAX_SIDE, PreviewError};

/// Render a preview with cancellation disabled.
fn render(
    install: &vips_gui::vips::VipsInstall,
    settings: &ConversionSettings,
    input: &std::path::Path,
    scratch: &std::path::Path,
    use_stdout: bool,
) -> Result<preview::Preview, PreviewError> {
    static NEVER: AtomicBool = AtomicBool::new(false);
    preview::render(install, settings, input, scratch, use_stdout, &NEVER)
}

#[test]
fn stdout_support_is_detected_on_this_libvips() {
    let Some(install) = common::vips() else {
        common::skip("stdout_support_is_detected_on_this_libvips");
        return;
    };

    // Every libvips in circulation supports the leading-dot stdout convention.
    // If this ever fails on a real build, the temp-file fallback covers it, but
    // it is worth knowing.
    assert!(
        preview::probe_stdout_support(install),
        "expected this libvips to write PNG to stdout"
    );
}

#[test]
fn the_predicted_size_equals_the_size_of_a_real_conversion() {
    let Some(install) = common::vips() else {
        common::skip("the_predicted_size_equals_the_size_of_a_real_conversion");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    common::make_source_image(install, &source, 600, 450);
    let scratch = dir.path().join("scratch");

    // A spread of formats and settings, since the prediction must hold for all
    // of them and not just the default.
    let mut cases: Vec<(&str, ConversionSettings)> = Vec::new();

    let mut webp = ConversionSettings::default();
    webp.format = OutputFormat::Webp;
    webp.webp.quality = 55;
    cases.push(("webp Q55", webp));

    let mut jpeg = ConversionSettings::default();
    jpeg.format = OutputFormat::Jpeg;
    jpeg.jpeg.quality = 90;
    cases.push(("jpeg Q90", jpeg));

    let mut png = ConversionSettings::default();
    png.format = OutputFormat::Png;
    png.png.compression = 9;
    cases.push(("png compression 9", png));

    let mut resized = ConversionSettings::default();
    resized.format = OutputFormat::Webp;
    resized.resize.enabled = true;
    resized.resize.max_width = 200;
    resized.resize.max_height = 200;
    cases.push(("webp resized to 200", resized));

    let mut stripped = ConversionSettings::default();
    stripped.format = OutputFormat::Jpeg;
    stripped.strip_metadata = true;
    cases.push(("jpeg stripped", stripped));

    if install.capabilities.avif {
        let mut avif = ConversionSettings::default();
        avif.format = OutputFormat::Avif;
        avif.avif.quality = 40;
        avif.avif.effort = 0;
        cases.push(("avif Q40", avif));
    }

    for (index, (name, settings)) in cases.into_iter().enumerate() {
        let rendered = render(install, &settings, &source, &scratch, true)
            .unwrap_or_else(|e| panic!("{name}: preview failed: {e}"));

        // Now do the conversion for real, into a fresh directory.
        let mut real_settings = settings.clone();
        real_settings.destination = OutputDestination::Directory(dir.path().join(format!("r{index}")));
        let Outcome::Written { bytes, .. } = convert::convert_one(install, &real_settings, &source)
            .unwrap_or_else(|e| panic!("{name}: real conversion failed: {e}"))
        else {
            panic!("{name}: expected a written file");
        };

        assert_eq!(
            rendered.predicted_bytes, bytes,
            "{name}: preview predicted {} bytes but the real conversion produced {bytes}",
            rendered.predicted_bytes
        );
    }
}

#[test]
fn both_previews_are_decodable_pngs_within_the_size_limit() {
    let Some(install) = common::vips() else {
        common::skip("both_previews_are_decodable_pngs_within_the_size_limit");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("big.png");
    // Deliberately larger than the preview limit, so shrinking is exercised.
    common::make_source_image(install, &source, 2000, 1500);
    let scratch = dir.path().join("scratch");

    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Webp;

    let rendered = render(install, &settings, &source, &scratch, true).expect("preview should work");

    for (label, bytes) in [
        ("source", &rendered.source.png),
        ("result", &rendered.result.png),
    ] {
        let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
            .unwrap_or_else(|e| panic!("{label} preview should be a decodable PNG: {e}"));
        let (w, h) = (decoded.width(), decoded.height());

        assert!(
            w <= PREVIEW_MAX_SIDE && h <= PREVIEW_MAX_SIDE,
            "{label} preview is {w}x{h}, larger than the {PREVIEW_MAX_SIDE}px limit"
        );
        // 2000x1500 into a 720 box gives 720x540.
        assert_eq!((w, h), (720, 540), "{label} preview has unexpected dimensions");
    }

    // Dimensions of the real images, not the previews.
    assert_eq!(
        rendered.source_dimensions,
        Some((2000, 1500)),
        "source dimensions should describe the original, not the preview"
    );
    assert_eq!(
        rendered.result_dimensions,
        Some((2000, 1500)),
        "without resizing the output keeps the source dimensions"
    );
}

#[test]
fn resizing_is_reflected_in_the_reported_result_dimensions() {
    let Some(install) = common::vips() else {
        common::skip("resizing_is_reflected_in_the_reported_result_dimensions");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    common::make_source_image(install, &source, 1600, 1200);
    let scratch = dir.path().join("scratch");

    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Webp;
    settings.resize.enabled = true;
    settings.resize.max_width = 800;
    settings.resize.max_height = 800;

    let rendered = render(install, &settings, &source, &scratch, true).expect("preview should work");

    assert_eq!(rendered.source_dimensions, Some((1600, 1200)));
    assert_eq!(
        rendered.result_dimensions,
        Some((800, 600)),
        "the result dimensions should show the resize"
    );
}

#[test]
fn the_temp_file_fallback_produces_the_same_result_as_stdout() {
    let Some(install) = common::vips() else {
        common::skip("the_temp_file_fallback_produces_the_same_result_as_stdout");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    common::make_source_image(install, &source, 400, 300);

    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Webp;
    settings.webp.quality = 70;

    let via_stdout = render(install, &settings, &source, &dir.path().join("s1"), true)
        .expect("stdout path should work");
    let via_temp_file = render(install, &settings, &source, &dir.path().join("s2"), false)
        .expect("temp-file fallback should work");

    // The fallback exists so a libvips without stdout support still works. It
    // must produce identical output, not merely similar.
    assert_eq!(
        via_stdout.predicted_bytes, via_temp_file.predicted_bytes,
        "both paths encode the same file"
    );
    assert_eq!(
        via_stdout.source.png, via_temp_file.source.png,
        "the source preview should be byte-identical either way"
    );
    assert_eq!(
        via_stdout.result.png, via_temp_file.result.png,
        "the result preview should be byte-identical either way"
    );
}

#[test]
fn previewing_leaves_no_intermediate_files_behind() {
    let Some(install) = common::vips() else {
        common::skip("previewing_leaves_no_intermediate_files_behind");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    common::make_source_image(install, &source, 300, 200);
    let scratch = dir.path().join("scratch");

    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Webp;

    // Several previews in a row, as a slider drag would produce.
    for quality in [30u8, 50, 70, 90] {
        settings.webp.quality = quality;
        render(install, &settings, &source, &scratch, true).expect("preview should work");
    }

    // The scratch directory may exist but must be empty: the encoded file and
    // any display temp file are cleaned up after each render.
    if scratch.is_dir() {
        let leftovers: Vec<String> = std::fs::read_dir(&scratch)
            .expect("read scratch")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            leftovers.is_empty(),
            "preview intermediates should be cleaned up, found {leftovers:?}"
        );
    }

    // And the fallback path, which uses an extra temp file.
    settings.webp.quality = 60;
    render(install, &settings, &source, &scratch, false).expect("fallback preview should work");
    if scratch.is_dir() {
        let leftovers: Vec<String> = std::fs::read_dir(&scratch)
            .expect("read scratch")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            leftovers.is_empty(),
            "the fallback path should clean up too, found {leftovers:?}"
        );
    }
}

#[test]
fn a_corrupt_input_reports_a_preview_error_rather_than_panicking() {
    let Some(install) = common::vips() else {
        common::skip("a_corrupt_input_reports_a_preview_error_rather_than_panicking");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let bogus = dir.path().join("broken.png");
    std::fs::write(&bogus, b"not a png").expect("write fixture");
    let scratch = dir.path().join("scratch");

    let settings = ConversionSettings::default();
    let err = render(install, &settings, &bogus, &scratch, true)
        .expect_err("a corrupt file cannot be previewed");

    assert!(
        matches!(err, PreviewError::VipsFailed { .. } | PreviewError::NoOutput),
        "unexpected error variant: {err:?}"
    );
    // The message should be the libvips diagnostic, which is actionable.
    let message = err.to_string();
    assert!(!message.is_empty(), "an error needs an explanation");
}

#[test]
fn the_engine_serves_the_newest_request_and_discards_superseded_ones() {
    let Some(install) = common::vips() else {
        common::skip("the_engine_serves_the_newest_request_and_discards_superseded_ones");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    common::make_source_image(install, &source, 500, 400);

    let mut engine = preview::PreviewEngine::new(install.clone(), true, None);

    // Fire a burst, as an undebounced slider drag would.
    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Webp;

    let mut last_generation = 0;
    for quality in [20u8, 40, 60, 80, 95] {
        settings.webp.quality = quality;
        last_generation = engine.request(source.clone(), settings.clone());
    }

    // Wait for things to settle.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut outcomes = Vec::new();
    while std::time::Instant::now() < deadline {
        outcomes.extend(engine.drain());
        // The newest request must eventually report.
        if outcomes.iter().any(|o| o.generation == last_generation) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // Give any straggler a moment, then collect again.
    std::thread::sleep(std::time::Duration::from_millis(200));
    outcomes.extend(engine.drain());

    assert!(
        outcomes.iter().any(|o| o.generation == last_generation),
        "the newest request should always be served, got generations {:?}",
        outcomes.iter().map(|o| o.generation).collect::<Vec<_>>()
    );

    // Superseded requests are dropped rather than all being rendered, which is
    // the whole point of the single-slot mailbox.
    assert!(
        outcomes.len() < 5,
        "superseded requests should not all have been rendered, got {} outcomes",
        outcomes.len()
    );

    // Whatever did report should have succeeded.
    for outcome in &outcomes {
        assert!(
            outcome.result.is_ok(),
            "generation {} failed: {:?}",
            outcome.generation,
            outcome.result.as_ref().err()
        );
    }
}

#[test]
fn dropping_the_engine_cleans_up_its_scratch_directory() {
    let Some(install) = common::vips() else {
        common::skip("dropping_the_engine_cleans_up_its_scratch_directory");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    common::make_source_image(install, &source, 300, 200);

    let scratch;
    {
        let mut engine = preview::PreviewEngine::new(install.clone(), true, None);
        scratch = engine.scratch_dir().to_path_buf();
        engine.request(source.clone(), ConversionSettings::default());

        // Let it actually run so the directory gets created.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            if !engine.drain().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    assert!(
        !scratch.exists(),
        "the engine should remove {} when dropped",
        scratch.display()
    );
}

#[test]
fn two_engines_in_one_process_do_not_share_a_scratch_directory() {
    let Some(install) = common::vips() else {
        common::skip("two_engines_in_one_process_do_not_share_a_scratch_directory");
        return;
    };

    // Two engines must not collide: dropping one would otherwise delete the
    // other's intermediate files mid-render.
    let first = preview::PreviewEngine::new(install.clone(), true, None);
    let second = preview::PreviewEngine::new(install.clone(), true, None);

    assert_ne!(
        first.scratch_dir(),
        second.scratch_dir(),
        "each engine needs its own scratch directory"
    );

    let survivor = second.scratch_dir().to_path_buf();
    drop(first);
    assert_eq!(
        second.scratch_dir(),
        survivor.as_path(),
        "dropping one engine must not disturb the other"
    );
}
