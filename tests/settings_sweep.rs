//! Task 5 verification: every option the settings panel can reach produces a
//! command libvips actually accepts, and the lossless modes really are lossless.
//!
//! The panel is a thin binding over `ConversionSettings`, so sweeping the
//! settings struct covers everything the UI can produce.

mod common;

use std::path::Path;

use vips_gui::settings::{
    AvifOptions, ConversionSettings, JpegOptions, OutputDestination, OutputFormat, PngOptions,
    SubsampleMode, WebpPreset, WebpOptions,
};
use vips_gui::vips::command;
use vips_gui::vips::convert::{self, Outcome};
use vips_gui::vips::{VipsInstall, VipsVersion};

/// Every settings variation the UI can produce, as (name, settings) pairs.
///
/// Each option is swept across its full documented range with the others left
/// at their defaults, plus all-minimum and all-maximum combinations. That
/// covers each control and both extremes of the interaction space without a
/// combinatorial explosion.
fn all_variations() -> Vec<(String, ConversionSettings)> {
    let mut out: Vec<(String, ConversionSettings)> = Vec::new();

    let base = |format: OutputFormat| {
        let mut s = ConversionSettings::default();
        s.format = format;
        s
    };

    // --- JPEG ---
    for quality in [1u8, 25, 50, 75, 80, 95, 100] {
        let mut s = base(OutputFormat::Jpeg);
        s.jpeg.quality = quality;
        out.push((format!("jpeg Q={quality}"), s));
    }
    for table in 0u8..=8 {
        let mut s = base(OutputFormat::Jpeg);
        s.jpeg.quant_table = table;
        out.push((format!("jpeg quant-table={table}"), s));
    }
    for mode in SubsampleMode::ALL {
        let mut s = base(OutputFormat::Jpeg);
        s.jpeg.subsample = mode;
        out.push((format!("jpeg subsample={:?}", mode), s));
    }
    // Each boolean on its own, then all of them together.
    let jpeg_flags: [(&str, fn(&mut JpegOptions)); 5] = [
        ("optimize_coding", |o| o.optimize_coding = true),
        ("progressive", |o| o.progressive = true),
        ("optimize_scans", |o| {
            // Only meaningful with a progressive scan, which is how the panel
            // gates it too.
            o.progressive = true;
            o.optimize_scans = true;
        }),
        ("trellis_quant", |o| o.trellis_quant = true),
        ("overshoot_deringing", |o| o.overshoot_deringing = true),
    ];
    for (name, apply) in jpeg_flags {
        let mut s = base(OutputFormat::Jpeg);
        s.jpeg = JpegOptions {
            optimize_coding: false,
            progressive: false,
            ..JpegOptions::default()
        };
        apply(&mut s.jpeg);
        out.push((format!("jpeg {name}"), s));
    }
    {
        let mut s = base(OutputFormat::Jpeg);
        s.jpeg = JpegOptions {
            quality: 100,
            optimize_coding: true,
            progressive: true,
            optimize_scans: true,
            trellis_quant: true,
            overshoot_deringing: true,
            quant_table: 8,
            subsample: SubsampleMode::Off,
        };
        out.push(("jpeg everything on".into(), s));
    }
    {
        let mut s = base(OutputFormat::Jpeg);
        s.jpeg = JpegOptions {
            quality: 1,
            optimize_coding: false,
            progressive: false,
            optimize_scans: false,
            trellis_quant: false,
            overshoot_deringing: false,
            quant_table: 0,
            subsample: SubsampleMode::On,
        };
        out.push(("jpeg everything off".into(), s));
    }

    // --- PNG ---
    for compression in 0u8..=9 {
        let mut s = base(OutputFormat::Png);
        s.png.compression = compression;
        out.push((format!("png compression={compression}"), s));
    }
    for bitdepth in [1u8, 2, 4, 8, 16] {
        let mut s = base(OutputFormat::Png);
        s.png.bitdepth = bitdepth;
        // Sub-8-bit output needs the quantiser, which is how the panel
        // presents it as well.
        if bitdepth < 8 {
            s.png.palette = true;
        }
        out.push((format!("png bitdepth={bitdepth}"), s));
    }
    for effort in 1u8..=10 {
        let mut s = base(OutputFormat::Png);
        s.png.palette = true;
        s.png.effort = effort;
        out.push((format!("png palette effort={effort}"), s));
    }
    for dither in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
        let mut s = base(OutputFormat::Png);
        s.png.palette = true;
        s.png.dither = dither;
        out.push((format!("png dither={dither}"), s));
    }
    for quality in [0u8, 50, 100] {
        let mut s = base(OutputFormat::Png);
        s.png.palette = true;
        s.png.quality = quality;
        out.push((format!("png palette Q={quality}"), s));
    }
    {
        let mut s = base(OutputFormat::Png);
        s.png = PngOptions {
            compression: 9,
            interlace: true,
            palette: true,
            quality: 90,
            dither: 1.0,
            bitdepth: 8,
            effort: 10,
        };
        out.push(("png everything on".into(), s));
    }

    // --- WebP ---
    for quality in [0u8, 25, 50, 80, 100] {
        let mut s = base(OutputFormat::Webp);
        s.webp.quality = quality;
        out.push((format!("webp Q={quality}"), s));
    }
    for effort in 0u8..=6 {
        let mut s = base(OutputFormat::Webp);
        s.webp.effort = effort;
        out.push((format!("webp effort={effort}"), s));
    }
    for alpha in [0u8, 50, 100] {
        let mut s = base(OutputFormat::Webp);
        s.webp.alpha_quality = alpha;
        out.push((format!("webp alpha-q={alpha}"), s));
    }
    for preset in WebpPreset::ALL {
        let mut s = base(OutputFormat::Webp);
        s.webp.preset = preset;
        out.push((format!("webp preset={:?}", preset), s));
    }
    for (name, lossless, near) in [
        ("lossy", false, false),
        ("lossless", true, false),
        ("near-lossless", false, true),
    ] {
        let mut s = base(OutputFormat::Webp);
        s.webp.lossless = lossless;
        s.webp.near_lossless = near;
        out.push((format!("webp {name}"), s));
    }
    {
        let mut s = base(OutputFormat::Webp);
        s.webp = WebpOptions {
            quality: 100,
            lossless: false,
            near_lossless: false,
            effort: 6,
            alpha_quality: 100,
            smart_subsample: true,
            preset: WebpPreset::Photo,
            // `min-size` is very slow, so exercise it on its own at low effort
            // rather than combined with everything else.
            min_size: false,
        };
        out.push(("webp everything on".into(), s));
    }
    {
        let mut s = base(OutputFormat::Webp);
        s.webp.min_size = true;
        s.webp.effort = 0;
        out.push(("webp min-size".into(), s));
    }

    // --- AVIF ---
    // Effort is capped low here on purpose: this sweep runs the real encoder
    // dozens of times, and AVIF effort 9 would take minutes.
    for quality in [1u8, 30, 50, 80, 100] {
        let mut s = base(OutputFormat::Avif);
        s.avif.quality = quality;
        s.avif.effort = 0;
        out.push((format!("avif Q={quality}"), s));
    }
    for bitdepth in [8u8, 10, 12] {
        let mut s = base(OutputFormat::Avif);
        s.avif.bitdepth = bitdepth;
        s.avif.effort = 0;
        out.push((format!("avif bitdepth={bitdepth}"), s));
    }
    for mode in SubsampleMode::ALL {
        let mut s = base(OutputFormat::Avif);
        s.avif.subsample = mode;
        s.avif.effort = 0;
        out.push((format!("avif subsample={:?}", mode), s));
    }
    for effort in [0u8, 1, 2, 3, 4] {
        let mut s = base(OutputFormat::Avif);
        s.avif.effort = effort;
        out.push((format!("avif effort={effort}"), s));
    }
    {
        let mut s = base(OutputFormat::Avif);
        s.avif = AvifOptions {
            quality: 100,
            lossless: true,
            effort: 0,
            bitdepth: 8,
            subsample: SubsampleMode::Off,
        };
        out.push(("avif lossless".into(), s));
    }

    // --- cross-cutting: resize and metadata on top of a normal format ---
    for (name, enabled, never_upscale) in [
        ("resize down", true, true),
        ("resize both", true, false),
        ("resize off", false, true),
    ] {
        let mut s = base(OutputFormat::Webp);
        s.resize.enabled = enabled;
        s.resize.never_upscale = never_upscale;
        s.resize.max_width = 100;
        s.resize.max_height = 100;
        out.push((format!("webp {name}"), s));
    }
    for strip in [false, true] {
        let mut s = base(OutputFormat::Webp);
        s.strip_metadata = strip;
        out.push((format!("webp strip={strip}"), s));
    }

    out
}

#[test]
fn every_reachable_option_builds_a_well_formed_command() {
    // Pure check, no vips needed: the option suffix must always parse.
    for version in [
        VipsVersion {
            major: 8,
            minor: 14,
            patch: 0,
        },
        VipsVersion {
            major: 8,
            minor: 18,
            patch: 6,
        },
    ] {
        for (name, settings) in all_variations() {
            let output = Path::new("out").with_extension(settings.format.extension());
            let args = command::build(&settings, Path::new("in.png"), &output, version);
            let strings = args.as_strings();

            assert!(
                strings[0] == "copy" || strings[0] == "thumbnail",
                "{name}: unexpected subcommand {}",
                strings[0]
            );

            let target = &strings[2];
            let (path_part, options) = match target.split_once('[') {
                Some((path, rest)) => (
                    path,
                    rest.strip_suffix(']')
                        .unwrap_or_else(|| panic!("{name}: unterminated option suffix: {target}")),
                ),
                None => (target.as_str(), ""),
            };

            assert!(
                path_part.ends_with(settings.format.extension()),
                "{name}: target should carry the format extension: {target}"
            );

            for pair in options.split(',').filter(|p| !p.is_empty()) {
                let (key, value) = pair
                    .split_once('=')
                    .unwrap_or_else(|| panic!("{name}: option {pair} is not key=value"));
                assert!(!key.is_empty(), "{name}: empty option key in {target}");
                assert!(!value.is_empty(), "{name}: empty value for {key}");
                assert!(
                    !key.contains(' ') && !value.contains(' '),
                    "{name}: whitespace in option {pair}"
                );
                // The old and new metadata spellings must never both appear.
                assert!(
                    !(options.contains("keep=none") && options.contains("strip=")),
                    "{name}: both metadata spellings emitted: {options}"
                );
            }
        }
    }
}

#[test]
fn every_reachable_option_is_accepted_by_the_real_encoder() {
    let Some(install) = common::vips() else {
        common::skip("every_reachable_option_is_accepted_by_the_real_encoder");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    // Small, so the sweep stays quick even at high effort settings.
    common::make_source_image(install, &source, 64, 48);

    let variations = all_variations();
    let mut ran = 0usize;
    let mut skipped_unsupported = 0usize;

    for (index, (name, mut settings)) in variations.into_iter().enumerate() {
        let supported = match settings.format {
            OutputFormat::Jpeg => install.capabilities.jpeg,
            OutputFormat::Png => install.capabilities.png,
            OutputFormat::Webp => install.capabilities.webp,
            OutputFormat::Avif => install.capabilities.avif,
        };
        if !supported {
            skipped_unsupported += 1;
            continue;
        }

        // A directory per variation, so nothing collides.
        settings.destination = OutputDestination::Directory(dir.path().join(format!("v{index}")));

        match convert::convert_one(install, &settings, &source) {
            Ok(Outcome::Written { output, bytes }) => {
                assert!(bytes > 0, "{name}: wrote an empty file");
                assert!(output.is_file(), "{name}: output missing");
                ran += 1;
            }
            Ok(Outcome::Skipped { .. }) => panic!("{name}: unexpected skip in a clean directory"),
            Err(err) => panic!("{name}: libvips rejected the command: {err}"),
        }
    }

    assert!(
        ran > 60,
        "the sweep should have exercised most variations, only ran {ran} \
         ({skipped_unsupported} skipped as unsupported)"
    );
}

/// Convert an image to raw uncompressed pixels so two images can be compared
/// exactly. `rawsave` writes the pixel buffer with no header or compression.
fn raw_pixels(install: &VipsInstall, image: &Path, scratch: &Path) -> Vec<u8> {
    let raw = scratch.join(format!(
        "{}.raw",
        image.file_name().unwrap().to_string_lossy()
    ));
    common::run_ok(
        install,
        &["copy", &image.to_string_lossy(), &raw.to_string_lossy()],
    );
    std::fs::read(&raw).expect("read raw pixels")
}

#[test]
fn lossless_webp_decodes_back_to_the_original_pixels() {
    let Some(install) = common::vips() else {
        common::skip("lossless_webp_decodes_back_to_the_original_pixels");
        return;
    };
    if !install.capabilities.webp {
        eprintln!("SKIPPING: this libvips build has no webpsave");
        return;
    }

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    common::make_source_image(install, &source, 90, 70);

    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Webp;
    settings.webp.lossless = true;
    settings.destination = OutputDestination::Directory(dir.path().join("out"));

    let Outcome::Written { output, .. } =
        convert::convert_one(install, &settings, &source).expect("lossless webp should encode")
    else {
        panic!("expected a written file");
    };

    let before = raw_pixels(install, &source, dir.path());
    let after = raw_pixels(install, &output, dir.path());

    assert_eq!(
        before.len(),
        after.len(),
        "lossless output should have the same pixel buffer size"
    );
    assert_eq!(
        before, after,
        "lossless WebP must reproduce every pixel exactly"
    );
}

#[test]
fn lossless_avif_decodes_back_to_the_original_pixels() {
    let Some(install) = common::vips() else {
        common::skip("lossless_avif_decodes_back_to_the_original_pixels");
        return;
    };
    if !install.capabilities.avif {
        eprintln!("SKIPPING: this libvips build has no heifsave");
        return;
    }

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    common::make_source_image(install, &source, 90, 70);

    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Avif;
    settings.avif.lossless = true;
    settings.avif.bitdepth = 8;
    // Lossless AVIF is slow; the lowest effort still has to be exact.
    settings.avif.effort = 0;
    settings.destination = OutputDestination::Directory(dir.path().join("out"));

    let Outcome::Written { output, .. } =
        convert::convert_one(install, &settings, &source).expect("lossless avif should encode")
    else {
        panic!("expected a written file");
    };

    let before = raw_pixels(install, &source, dir.path());
    let after = raw_pixels(install, &output, dir.path());

    assert_eq!(
        before.len(),
        after.len(),
        "lossless output should have the same pixel buffer size"
    );
    assert_eq!(
        before, after,
        "lossless AVIF must reproduce every pixel exactly"
    );
}

#[test]
fn a_lossy_encode_is_not_pixel_identical() {
    // The counterpart to the lossless tests: proves they are actually
    // measuring something, rather than the comparison being vacuously true.
    let Some(install) = common::vips() else {
        common::skip("a_lossy_encode_is_not_pixel_identical");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    common::make_source_image(install, &source, 90, 70);

    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Webp;
    settings.webp.lossless = false;
    settings.webp.quality = 40;
    settings.destination = OutputDestination::Directory(dir.path().join("out"));

    let Outcome::Written { output, .. } =
        convert::convert_one(install, &settings, &source).expect("lossy webp should encode")
    else {
        panic!("expected a written file");
    };

    let before = raw_pixels(install, &source, dir.path());
    let after = raw_pixels(install, &output, dir.path());

    assert_ne!(
        before, after,
        "a lossy encode of noise at Q40 should not be pixel-perfect"
    );
}

#[test]
fn png_is_always_lossless_unless_quantised() {
    let Some(install) = common::vips() else {
        common::skip("png_is_always_lossless_unless_quantised");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source.png");
    common::make_source_image(install, &source, 90, 70);
    let before = raw_pixels(install, &source, dir.path());

    // Maximum compression, no palette: still bit-exact.
    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Png;
    settings.png.compression = 9;
    settings.png.palette = false;
    settings.destination = OutputDestination::Directory(dir.path().join("plain"));

    let Outcome::Written { output, .. } =
        convert::convert_one(install, &settings, &source).expect("png should encode")
    else {
        panic!("expected a written file");
    };
    assert_eq!(
        before,
        raw_pixels(install, &output, dir.path()),
        "PNG without a palette must be lossless at any compression level"
    );

    // With the quantiser on, it is lossy by design.
    let mut palette_settings = settings.clone();
    palette_settings.png.palette = true;
    palette_settings.png.quality = 40;
    palette_settings.destination = OutputDestination::Directory(dir.path().join("palette"));

    let Outcome::Written { output, .. } =
        convert::convert_one(install, &palette_settings, &source).expect("palette png should encode")
    else {
        panic!("expected a written file");
    };
    assert_ne!(
        before,
        raw_pixels(install, &output, dir.path()),
        "a quantised PNG of noise should not be pixel-perfect"
    );
}

#[test]
fn two_settings_profiles_produce_measurably_different_batches() {
    // The Task 5 demo: run the same batch under two very different profiles
    // and confirm both work and produce distinct results.
    let Some(install) = common::vips() else {
        common::skip("two_settings_profiles_produce_measurably_different_batches");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let sources_dir = dir.path().join("sources");
    std::fs::create_dir_all(&sources_dir).expect("create sources");

    // A batch of three, larger than the resize target so the resize is visible.
    let sources: Vec<_> = (0..3)
        .map(|i| {
            let path = sources_dir.join(format!("img{i}.png"));
            common::make_source_image(install, &path, 2400, 1600);
            path
        })
        .collect();

    let run_profile = |settings: &ConversionSettings, subdir: &str| -> (u64, Vec<(u32, u32)>) {
        let mut settings = settings.clone();
        settings.destination = OutputDestination::Directory(dir.path().join(subdir));
        let mut total = 0u64;
        let mut sizes = Vec::new();
        for source in &sources {
            let Outcome::Written { output, bytes } = convert::convert_one(install, &settings, source)
                .unwrap_or_else(|e| panic!("{subdir} profile failed: {e}"))
            else {
                panic!("expected a write");
            };
            total += bytes;
            sizes.push(common::image_size(install, &output));
        }
        (total, sizes)
    };

    // Profile A: AVIF Q30, effort 6, resized into a 1920-wide box.
    let mut avif_profile = ConversionSettings::default();
    avif_profile.format = OutputFormat::Avif;
    avif_profile.avif.quality = 30;
    // Effort 6 as the demo describes. Slow, but only three small-ish images.
    avif_profile.avif.effort = 6;
    avif_profile.resize.enabled = true;
    avif_profile.resize.max_width = 1920;
    avif_profile.resize.max_height = 1920;
    avif_profile.resize.never_upscale = true;

    // Profile B: quantised 8-bit PNG at full size.
    let mut png_profile = ConversionSettings::default();
    png_profile.format = OutputFormat::Png;
    png_profile.png.palette = true;
    png_profile.png.bitdepth = 8;
    png_profile.png.compression = 9;

    if install.capabilities.avif {
        let (avif_total, avif_sizes) = run_profile(&avif_profile, "avif");
        // 2400x1600 into a 1920 box preserving aspect gives 1920x1280.
        for (w, h) in &avif_sizes {
            assert_eq!(
                (*w, *h),
                (1920, 1280),
                "the AVIF profile should have resized into the box"
            );
        }
        assert!(avif_total > 0);

        if install.capabilities.png {
            let (png_total, png_sizes) = run_profile(&png_profile, "png");
            for (w, h) in &png_sizes {
                assert_eq!(
                    (*w, *h),
                    (2400, 1600),
                    "the PNG profile should keep the original size"
                );
            }

            // Noise is the worst case for both, but a resized lossy AVIF
            // should still beat a full-size quantised PNG comfortably.
            assert!(
                avif_total < png_total,
                "resized AVIF Q30 ({avif_total} bytes) should be smaller than \
                 full-size palette PNG ({png_total} bytes)"
            );
        }
    } else {
        eprintln!("SKIPPING the AVIF half: this libvips build has no heifsave");
        let (png_total, _) = run_profile(&png_profile, "png");
        assert!(png_total > 0);
    }
}
