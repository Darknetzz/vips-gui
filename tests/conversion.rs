//! End-to-end conversions against a real libvips.
//!
//! These prove the generated commands are not just well-formed strings but
//! actually produce decodable images of the expected size.

mod common;

use vips_gui::settings::{
    ConversionSettings, OutputDestination, OutputFormat, OverwritePolicy, SubsampleMode,
};
use vips_gui::vips::convert::{self, ConvertError, Outcome};

/// Source fixture dimensions, chosen to be small enough to encode fast and
/// non-square so a bounding-box resize is observable.
const SRC_W: u32 = 400;
const SRC_H: u32 = 300;

#[test]
fn converts_to_every_format_with_default_settings() {
    let Some(install) = common::vips() else {
        common::skip("converts_to_every_format_with_default_settings");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    common::make_source_image(install, &source, SRC_W, SRC_H);

    for format in OutputFormat::ALL {
        // Skip formats this build cannot write rather than failing: a
        // libvips without libheif is a legitimate installation.
        let supported = match format {
            OutputFormat::Jpeg => install.capabilities.jpeg,
            OutputFormat::Png => install.capabilities.png,
            OutputFormat::Webp => install.capabilities.webp,
            OutputFormat::Avif => install.capabilities.avif,
        };
        if !supported {
            eprintln!(
                "skipping {}: this libvips build has no {}",
                format.label(),
                format.saver_name()
            );
            continue;
        }

        let mut settings = ConversionSettings::default();
        settings.format = format;
        // Keep each format's output in its own directory so the file names
        // cannot collide between iterations.
        let out_dir = dir.path().join(format.extension());
        settings.destination = OutputDestination::Directory(out_dir);
        // AVIF at high effort is slow; the default 4 is plenty for a test.
        settings.avif.effort = 0;

        let outcome = convert::convert_one(install, &settings, &source)
            .unwrap_or_else(|e| panic!("converting to {} failed: {e}", format.label()));

        let Outcome::Written { output, bytes } = outcome else {
            panic!("expected a written file for {}", format.label());
        };

        assert!(
            output.is_file(),
            "{} output should exist at {}",
            format.label(),
            output.display()
        );
        assert!(bytes > 0, "{} output should not be empty", format.label());
        assert_eq!(
            output.extension().unwrap().to_string_lossy(),
            format.extension(),
            "output extension should match the chosen format"
        );

        // The real proof: libvips can read it back at the original size.
        let (w, h) = common::image_size(install, &output);
        assert_eq!(
            (w, h),
            (SRC_W, SRC_H),
            "{} should preserve dimensions when not resizing",
            format.label()
        );
    }
}

#[test]
fn resizing_fits_the_image_inside_the_bounding_box() {
    let Some(install) = common::vips() else {
        common::skip("resizing_fits_the_image_inside_the_bounding_box");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    common::make_source_image(install, &source, SRC_W, SRC_H);

    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Webp;
    settings.destination = OutputDestination::Directory(dir.path().join("resized"));
    settings.resize.enabled = true;
    settings.resize.max_width = 200;
    settings.resize.max_height = 200;
    settings.resize.never_upscale = true;

    let outcome =
        convert::convert_one(install, &settings, &source).expect("resize conversion should work");
    let Outcome::Written { output, .. } = outcome else {
        panic!("expected a written file");
    };

    let (w, h) = common::image_size(install, &output);
    // 400x300 into a 200x200 box preserving aspect ratio gives 200x150.
    assert_eq!(
        (w, h),
        (200, 150),
        "should fit the box and keep the aspect ratio"
    );
}

#[test]
fn never_upscale_leaves_a_small_image_alone() {
    let Some(install) = common::vips() else {
        common::skip("never_upscale_leaves_a_small_image_alone");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("small.png");
    common::make_source_image(install, &source, 100, 80);

    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Webp;
    settings.destination = OutputDestination::Directory(dir.path().join("out"));
    settings.resize.enabled = true;
    settings.resize.max_width = 1000;
    settings.resize.max_height = 1000;
    settings.resize.never_upscale = true;

    let outcome = convert::convert_one(install, &settings, &source).expect("conversion should work");
    let Outcome::Written { output, .. } = outcome else {
        panic!("expected a written file");
    };

    let (w, h) = common::image_size(install, &output);
    assert_eq!(
        (w, h),
        (100, 80),
        "`--size down` must not enlarge a small source"
    );
}

#[test]
fn allowing_upscale_enlarges_to_the_box() {
    let Some(install) = common::vips() else {
        common::skip("allowing_upscale_enlarges_to_the_box");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("small.png");
    common::make_source_image(install, &source, 100, 80);

    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Png;
    settings.destination = OutputDestination::Directory(dir.path().join("out"));
    settings.resize.enabled = true;
    settings.resize.max_width = 400;
    settings.resize.max_height = 400;
    settings.resize.never_upscale = false;

    let outcome = convert::convert_one(install, &settings, &source).expect("conversion should work");
    let Outcome::Written { output, .. } = outcome else {
        panic!("expected a written file");
    };

    let (w, h) = common::image_size(install, &output);
    // 100x80 into 400x400 with upscaling allowed gives 400x320.
    assert_eq!((w, h), (400, 320));
}

#[test]
fn quality_settings_actually_change_the_output_size() {
    let Some(install) = common::vips() else {
        common::skip("quality_settings_actually_change_the_output_size");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    common::make_source_image(install, &source, SRC_W, SRC_H);

    let size_at = |quality: u8, subdir: &str| -> u64 {
        let mut settings = ConversionSettings::default();
        settings.format = OutputFormat::Jpeg;
        settings.jpeg.quality = quality;
        settings.destination = OutputDestination::Directory(dir.path().join(subdir));
        match convert::convert_one(install, &settings, &source).expect("conversion should work") {
            Outcome::Written { bytes, .. } => bytes,
            Outcome::Skipped { .. } => panic!("unexpected skip"),
        }
    };

    let low = size_at(20, "low");
    let high = size_at(95, "high");

    // This is the sanity check that the option suffix is reaching the encoder
    // at all: if the options were silently ignored, these would be equal.
    assert!(
        high > low,
        "Q95 ({high} bytes) should be larger than Q20 ({low} bytes)"
    );
}

#[test]
fn stripping_metadata_produces_a_smaller_file() {
    let Some(install) = common::vips() else {
        common::skip("stripping_metadata_produces_a_smaller_file");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    common::make_source_image(install, &source, SRC_W, SRC_H);

    let size_with_strip = |strip: bool, subdir: &str| -> u64 {
        let mut settings = ConversionSettings::default();
        settings.format = OutputFormat::Jpeg;
        settings.strip_metadata = strip;
        settings.destination = OutputDestination::Directory(dir.path().join(subdir));
        match convert::convert_one(install, &settings, &source).expect("conversion should work") {
            Outcome::Written { bytes, .. } => bytes,
            Outcome::Skipped { .. } => panic!("unexpected skip"),
        }
    };

    let kept = size_with_strip(false, "kept");
    let stripped = size_with_strip(true, "stripped");

    // Whichever spelling the installed version uses, the effect must be real.
    assert!(
        stripped < kept,
        "stripping metadata should shrink the file: {stripped} vs {kept} bytes"
    );
}

#[test]
fn a_corrupt_input_surfaces_the_vips_error_message() {
    let Some(install) = common::vips() else {
        common::skip("a_corrupt_input_surfaces_the_vips_error_message");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let bogus = dir.path().join("not-really.jpg");
    std::fs::write(&bogus, b"this is definitely not a jpeg").expect("write fixture");

    let mut settings = ConversionSettings::default();
    settings.destination = OutputDestination::Directory(dir.path().join("out"));

    let err = convert::convert_one(install, &settings, &bogus)
        .expect_err("a corrupt file must not convert");

    match err {
        ConvertError::VipsFailed { message, command } => {
            // libvips says "is not a known file format"; we should be passing
            // that through rather than replacing it with something generic.
            assert!(
                message.contains("not a known file format") || message.contains("unable to load"),
                "expected the libvips diagnostic, got: {message}"
            );
            // The class prefix should have been trimmed.
            assert!(
                !message.starts_with("VipsForeignLoad:"),
                "class prefix should be stripped: {message}"
            );
            assert!(
                command.contains("copy"),
                "the failing command should be reported for copying: {command}"
            );
        }
        other => panic!("expected VipsFailed, got {other:?}"),
    }
}

#[test]
fn overwrite_policies_behave_as_advertised_on_real_files() {
    let Some(install) = common::vips() else {
        common::skip("overwrite_policies_behave_as_advertised_on_real_files");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let source = work.join("photo.png");
    common::make_source_image(install, &source, 120, 90);

    // Convert png -> jpg next to the source, three times, under Rename.
    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Jpeg;
    settings.destination = OutputDestination::SameAsSource;
    settings.overwrite = OverwritePolicy::Rename;

    let first = convert::convert_one(install, &settings, &source).expect("first conversion");
    let Outcome::Written { output: p1, .. } = first else {
        panic!("expected a write");
    };
    assert_eq!(p1.file_name().unwrap(), "photo.jpg");

    let second = convert::convert_one(install, &settings, &source).expect("second conversion");
    let Outcome::Written { output: p2, .. } = second else {
        panic!("expected a write");
    };
    assert_eq!(
        p2.file_name().unwrap(),
        "photo (1).jpg",
        "rename should not clobber the first output"
    );

    // Under Skip, nothing new appears.
    settings.overwrite = OverwritePolicy::Skip;
    let third = convert::convert_one(install, &settings, &source).expect("third conversion");
    assert!(
        matches!(third, Outcome::Skipped { .. }),
        "skip policy should report a skip, got {third:?}"
    );

    // Under Overwrite, the original path is rewritten, not duplicated.
    settings.overwrite = OverwritePolicy::Overwrite;
    let before = std::fs::metadata(&p1).unwrap().len();
    settings.jpeg.quality = 15; // Force a visibly different file size.
    let fourth = convert::convert_one(install, &settings, &source).expect("fourth conversion");
    let Outcome::Written { output: p4, .. } = fourth else {
        panic!("expected a write");
    };
    assert_eq!(p4, p1, "overwrite should reuse the nominal path");
    let after = std::fs::metadata(&p1).unwrap().len();
    assert_ne!(before, after, "the file should have been rewritten");

    // Exactly two jpgs total: photo.jpg and photo (1).jpg.
    let jpgs = std::fs::read_dir(&work)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jpg"))
        .count();
    assert_eq!(jpgs, 2, "no unexpected extra files should have been created");
}

#[test]
fn a_same_format_conversion_cannot_destroy_its_own_input() {
    let Some(install) = common::vips() else {
        common::skip("a_same_format_conversion_cannot_destroy_its_own_input");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let source = work.join("original.jpg");
    common::make_source_image(install, &source, 150, 150);
    let original_bytes = std::fs::read(&source).expect("read source");

    // jpg -> jpg, same folder, with the default policy. This is the case the
    // Rename default exists to protect against.
    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Jpeg;
    settings.destination = OutputDestination::SameAsSource;
    assert_eq!(
        settings.overwrite,
        OverwritePolicy::Rename,
        "this test is about the default policy"
    );

    let outcome = convert::convert_one(install, &settings, &source).expect("conversion");
    let Outcome::Written { output, .. } = outcome else {
        panic!("expected a write");
    };

    assert_ne!(output, source, "must not write over the input");
    assert_eq!(
        std::fs::read(&source).expect("re-read source"),
        original_bytes,
        "the input file must be byte-identical afterwards"
    );
}

#[test]
fn subsampling_option_reaches_the_encoder() {
    let Some(install) = common::vips() else {
        common::skip("subsampling_option_reaches_the_encoder");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    common::make_source_image(install, &source, SRC_W, SRC_H);

    let size_with = |mode: SubsampleMode, subdir: &str| -> u64 {
        let mut settings = ConversionSettings::default();
        settings.format = OutputFormat::Jpeg;
        // Below Q90 so `auto` and `on` agree, making `off` the outlier.
        settings.jpeg.quality = 80;
        settings.jpeg.subsample = mode;
        settings.destination = OutputDestination::Directory(dir.path().join(subdir));
        match convert::convert_one(install, &settings, &source).expect("conversion should work") {
            Outcome::Written { bytes, .. } => bytes,
            Outcome::Skipped { .. } => panic!("unexpected skip"),
        }
    };

    let on = size_with(SubsampleMode::On, "on");
    let off = size_with(SubsampleMode::Off, "off");

    // Disabling chroma subsampling keeps full colour resolution, so the file
    // must be larger. Equality would mean the option never arrived.
    assert!(
        off > on,
        "subsample-mode=off ({off} bytes) should exceed on ({on} bytes)"
    );
}
