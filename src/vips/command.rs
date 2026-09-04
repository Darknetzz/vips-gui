//! Turning [`ConversionSettings`] into vips command-line arguments.
//!
//! Everything here is a pure function of the settings, so it is cheap to test
//! exhaustively without touching the filesystem or spawning a process.
//!
//! Two command shapes cover every case:
//!
//! - No resize: `vips copy IN OUT[opts]`
//! - Resize:    `vips thumbnail IN OUT[opts] WIDTH --height H --size down`
//!
//! `thumbnail` is used rather than `resize` because it shrinks on load, so a
//! 60 megapixel source never gets fully decoded just to produce a small
//! thumbnail.
//!
//! Encoder options ride along in the `[key=value,...]` suffix on the output
//! filename, which is a first-class libvips filename convention. Option names
//! use the hyphenated spelling from the libvips reference (`optimize-coding`,
//! not `optimize_coding`); both are accepted, but the hyphenated form is what
//! the docs show.

use std::ffi::OsString;
use std::path::Path;

use crate::settings::{
    AvifOptions, ConversionSettings, JpegOptions, OutputFormat, PngOptions, WebpOptions,
};
use crate::vips::VipsVersion;

/// The vips subcommand plus all its arguments, ready for `Command::args`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VipsArgs(pub Vec<OsString>);

impl VipsArgs {
    /// Render as a shell-ish string. For error messages and tests only; the
    /// real invocation passes the vector straight to the OS with no shell.
    pub fn to_display_string(&self) -> String {
        self.0
            .iter()
            .map(|a| {
                let s = a.to_string_lossy();
                if s.contains(' ') {
                    format!("\"{s}\"")
                } else {
                    s.into_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The arguments as lossy strings, for assertions.
    pub fn as_strings(&self) -> Vec<String> {
        self.0
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }
}

/// Build the full argument list for one conversion.
///
/// `version` decides the metadata-stripping spelling, which changed in 8.15.
pub fn build(
    settings: &ConversionSettings,
    input: &Path,
    output: &Path,
    version: VipsVersion,
) -> VipsArgs {
    let target = target_with_options(settings, output, version);

    let mut args: Vec<OsString> = Vec::with_capacity(8);

    if settings.resize.enabled {
        args.push("thumbnail".into());
        args.push(input.into());
        args.push(target);
        // `thumbnail` takes width as a positional argument and height as an
        // option; together they define a bounding box.
        args.push(settings.resize.max_width.to_string().into());
        args.push("--height".into());
        args.push(settings.resize.max_height.to_string().into());
        args.push("--size".into());
        args.push(if settings.resize.never_upscale {
            "down".into()
        } else {
            "both".into()
        });
    } else {
        // `copy` is a straight pass-through; all the work happens in the
        // saver selected by the output extension.
        args.push("copy".into());
        args.push(input.into());
        args.push(target);
    }

    VipsArgs(args)
}

/// The output path with its `[key=value,...]` option suffix attached.
fn target_with_options(
    settings: &ConversionSettings,
    output: &Path,
    version: VipsVersion,
) -> OsString {
    let options = save_options(settings, version);

    let mut target = OsString::from(output);
    if !options.is_empty() {
        target.push("[");
        target.push(options.join(","));
        target.push("]");
    }
    target
}

/// The save options for the selected format, in a fixed order.
///
/// Order is deliberate and stable so the generated command is reproducible and
/// testable.
fn save_options(settings: &ConversionSettings, version: VipsVersion) -> Vec<String> {
    let mut opts = match settings.format {
        OutputFormat::Jpeg => jpeg_options(&settings.jpeg),
        OutputFormat::Png => png_options(&settings.png),
        OutputFormat::Webp => webp_options(&settings.webp),
        OutputFormat::Avif => avif_options(&settings.avif),
    };

    // Metadata handling is format-independent, so it goes last.
    if settings.strip_metadata {
        opts.push(strip_option(version).to_owned());
    }

    opts
}

/// The option that discards metadata.
///
/// libvips 8.15 introduced the `keep` flags option and deprecated `strip`.
/// Both work on current releases, but only `strip` works before 8.15, so pick
/// based on the version actually installed.
fn strip_option(version: VipsVersion) -> &'static str {
    if version.uses_keep_option() {
        "keep=none"
    } else {
        "strip=true"
    }
}

fn jpeg_options(o: &JpegOptions) -> Vec<String> {
    let mut opts = vec![format!("Q={}", o.quality)];

    // Booleans are only emitted when enabled: libvips defaults them all to
    // false, so this keeps commands short and readable in error messages.
    if o.optimize_coding {
        opts.push("optimize-coding=true".into());
    }
    if o.progressive {
        opts.push("interlace=true".into());
    }
    // `optimize-scans` requires a progressive scan to have any effect, and
    // libvips warns if it is set without one.
    if o.optimize_scans && o.progressive {
        opts.push("optimize-scans=true".into());
    }
    if o.trellis_quant {
        opts.push("trellis-quant=true".into());
    }
    if o.overshoot_deringing {
        opts.push("overshoot-deringing=true".into());
    }
    if o.quant_table != 0 {
        opts.push(format!("quant-table={}", o.quant_table));
    }
    // Only emit a non-default subsampling mode; `auto` is the default.
    if o.subsample != crate::settings::SubsampleMode::Auto {
        opts.push(format!("subsample-mode={}", o.subsample.as_vips()));
    }

    opts
}

fn png_options(o: &PngOptions) -> Vec<String> {
    let mut opts = vec![format!("compression={}", o.compression)];

    if o.interlace {
        opts.push("interlace=true".into());
    }

    if o.palette {
        // Q, dither and effort only mean anything to the quantiser, so they
        // are emitted only alongside `palette`.
        opts.push("palette=true".into());
        opts.push(format!("Q={}", o.quality));
        opts.push(format!("dither={}", format_float(o.dither)));
        opts.push(format!("effort={}", o.effort));
    }

    // 8 is the libvips default, so only mention other depths.
    if o.bitdepth != 8 {
        opts.push(format!("bitdepth={}", o.bitdepth));
    }

    opts
}

fn webp_options(o: &WebpOptions) -> Vec<String> {
    let mut opts = vec![format!("Q={}", o.quality)];

    if o.lossless {
        opts.push("lossless=true".into());
    }
    if o.near_lossless {
        opts.push("near-lossless=true".into());
    }
    opts.push(format!("effort={}", o.effort));

    // Alpha quality is a lossy-mode control, and 100 is the default anyway.
    if o.alpha_quality != 100 && !o.lossless {
        opts.push(format!("alpha-q={}", o.alpha_quality));
    }
    if o.smart_subsample {
        opts.push("smart-subsample=true".into());
    }
    if o.preset != crate::settings::WebpPreset::Default {
        opts.push(format!("preset={}", o.preset.as_vips()));
    }
    if o.min_size {
        opts.push("min-size=true".into());
    }

    opts
}

fn avif_options(o: &AvifOptions) -> Vec<String> {
    let mut opts = vec![format!("Q={}", o.quality)];

    if o.lossless {
        opts.push("lossless=true".into());
    }
    opts.push(format!("effort={}", o.effort));
    // Always explicit: libvips defaults heifsave to 12-bit, which is not what
    // most people want from an .avif.
    opts.push(format!("bitdepth={}", o.bitdepth));

    if o.subsample != crate::settings::SubsampleMode::Auto {
        opts.push(format!("subsample-mode={}", o.subsample.as_vips()));
    }

    opts
}

/// Format a float without a locale-dependent or noisy representation.
///
/// libvips wants `1` or `0.5`, not `1.0000001` or a comma decimal separator.
fn format_float(value: f32) -> String {
    if (value - value.round()).abs() < f32::EPSILON {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{OutputFormat, SubsampleMode, WebpPreset};
    use std::path::PathBuf;

    /// libvips 8.18, i.e. the modern `keep=none` spelling.
    fn modern() -> VipsVersion {
        VipsVersion {
            major: 8,
            minor: 18,
            patch: 6,
        }
    }

    /// libvips 8.14, i.e. the legacy `strip` spelling.
    fn legacy() -> VipsVersion {
        VipsVersion {
            major: 8,
            minor: 14,
            patch: 0,
        }
    }

    fn paths() -> (PathBuf, PathBuf) {
        (PathBuf::from("in.jpg"), PathBuf::from("out.webp"))
    }

    fn build_strings(settings: &ConversionSettings, version: VipsVersion) -> Vec<String> {
        let (input, output) = paths();
        build(settings, &input, &output, version).as_strings()
    }

    #[test]
    fn without_resize_it_uses_copy_with_three_arguments() {
        let settings = ConversionSettings::default();
        let args = build_strings(&settings, modern());
        assert_eq!(args[0], "copy");
        assert_eq!(args[1], "in.jpg");
        assert_eq!(args.len(), 3, "copy takes exactly in and out: {args:?}");
    }

    #[test]
    fn with_resize_it_uses_thumbnail_with_a_bounding_box() {
        let mut settings = ConversionSettings::default();
        settings.resize.enabled = true;
        settings.resize.max_width = 1920;
        settings.resize.max_height = 1080;
        settings.resize.never_upscale = true;

        let args = build_strings(&settings, modern());
        assert_eq!(args[0], "thumbnail");
        assert_eq!(args[1], "in.jpg");
        // Width is positional, height is an option.
        assert_eq!(args[3], "1920");
        assert_eq!(args[4], "--height");
        assert_eq!(args[5], "1080");
        assert_eq!(args[6], "--size");
        assert_eq!(args[7], "down");
    }

    #[test]
    fn allowing_upscale_switches_size_to_both() {
        let mut settings = ConversionSettings::default();
        settings.resize.enabled = true;
        settings.resize.never_upscale = false;

        let args = build_strings(&settings, modern());
        let size_value = &args[args.len() - 1];
        assert_eq!(size_value, "both");
    }

    #[test]
    fn jpeg_defaults_produce_the_expected_option_suffix() {
        let mut settings = ConversionSettings::default();
        settings.format = OutputFormat::Jpeg;
        let (input, _) = paths();
        let output = PathBuf::from("out.jpg");
        let args = build(&settings, &input, &output, modern()).as_strings();
        // Q80, optimised Huffman tables and progressive are our defaults.
        assert_eq!(args[2], "out.jpg[Q=80,optimize-coding=true,interlace=true]");
    }

    #[test]
    fn jpeg_emits_every_advanced_option_when_enabled() {
        let mut settings = ConversionSettings::default();
        settings.format = OutputFormat::Jpeg;
        settings.jpeg.quality = 92;
        settings.jpeg.optimize_scans = true;
        settings.jpeg.trellis_quant = true;
        settings.jpeg.overshoot_deringing = true;
        settings.jpeg.quant_table = 3;
        settings.jpeg.subsample = SubsampleMode::Off;

        let (input, _) = paths();
        let output = PathBuf::from("out.jpg");
        let args = build(&settings, &input, &output, modern()).as_strings();
        assert_eq!(
            args[2],
            "out.jpg[Q=92,optimize-coding=true,interlace=true,optimize-scans=true,\
             trellis-quant=true,overshoot-deringing=true,quant-table=3,subsample-mode=off]"
                .replace(['\n', ' '], "")
        );
    }

    #[test]
    fn optimize_scans_is_dropped_without_a_progressive_scan() {
        let mut settings = ConversionSettings::default();
        settings.format = OutputFormat::Jpeg;
        settings.jpeg.progressive = false;
        settings.jpeg.optimize_scans = true;

        let (input, _) = paths();
        let output = PathBuf::from("out.jpg");
        let args = build(&settings, &input, &output, modern()).as_strings();
        assert!(
            !args[2].contains("optimize-scans"),
            "optimize-scans is meaningless without interlace: {}",
            args[2]
        );
    }

    #[test]
    fn png_without_palette_omits_the_quantiser_options() {
        let mut settings = ConversionSettings::default();
        settings.format = OutputFormat::Png;
        let (input, _) = paths();
        let output = PathBuf::from("out.png");
        let args = build(&settings, &input, &output, modern()).as_strings();
        assert_eq!(args[2], "out.png[compression=6]");
        assert!(!args[2].contains("dither"));
        assert!(!args[2].contains("effort"));
    }

    #[test]
    fn png_with_palette_includes_the_quantiser_options() {
        let mut settings = ConversionSettings::default();
        settings.format = OutputFormat::Png;
        settings.png.palette = true;
        settings.png.compression = 9;
        settings.png.quality = 90;
        settings.png.dither = 0.5;
        settings.png.effort = 8;

        let (input, _) = paths();
        let output = PathBuf::from("out.png");
        let args = build(&settings, &input, &output, modern()).as_strings();
        assert_eq!(
            args[2],
            "out.png[compression=9,palette=true,Q=90,dither=0.5,effort=8]"
        );
    }

    #[test]
    fn png_bitdepth_is_only_mentioned_when_it_differs_from_the_default() {
        let mut settings = ConversionSettings::default();
        settings.format = OutputFormat::Png;
        let (input, _) = paths();
        let output = PathBuf::from("out.png");

        let args = build(&settings, &input, &output, modern()).as_strings();
        assert!(!args[2].contains("bitdepth"), "8 is the default");

        settings.png.bitdepth = 4;
        let args = build(&settings, &input, &output, modern()).as_strings();
        assert!(args[2].contains("bitdepth=4"));
    }

    #[test]
    fn webp_defaults_produce_the_expected_option_suffix() {
        let settings = ConversionSettings::default();
        let args = build_strings(&settings, modern());
        assert_eq!(args[2], "out.webp[Q=80,effort=4]");
    }

    #[test]
    fn webp_lossless_suppresses_alpha_quality() {
        let mut settings = ConversionSettings::default();
        settings.webp.lossless = true;
        settings.webp.alpha_quality = 50;

        let args = build_strings(&settings, modern());
        assert!(args[2].contains("lossless=true"));
        assert!(
            !args[2].contains("alpha-q"),
            "alpha-q is a lossy-mode control: {}",
            args[2]
        );
    }

    #[test]
    fn webp_emits_alpha_quality_only_when_not_the_default() {
        let mut settings = ConversionSettings::default();
        let args = build_strings(&settings, modern());
        assert!(!args[2].contains("alpha-q"), "100 is the default");

        settings.webp.alpha_quality = 60;
        let args = build_strings(&settings, modern());
        assert!(args[2].contains("alpha-q=60"));
    }

    #[test]
    fn webp_emits_every_advanced_option_when_enabled() {
        let mut settings = ConversionSettings::default();
        settings.webp.quality = 65;
        settings.webp.near_lossless = true;
        settings.webp.effort = 6;
        settings.webp.smart_subsample = true;
        settings.webp.preset = WebpPreset::Photo;
        settings.webp.min_size = true;

        let args = build_strings(&settings, modern());
        assert_eq!(
            args[2],
            "out.webp[Q=65,near-lossless=true,effort=6,smart-subsample=true,\
             preset=photo,min-size=true]"
                .replace(['\n', ' '], "")
        );
    }

    #[test]
    fn avif_always_pins_bitdepth_explicitly() {
        let mut settings = ConversionSettings::default();
        settings.format = OutputFormat::Avif;
        let (input, _) = paths();
        let output = PathBuf::from("out.avif");
        let args = build(&settings, &input, &output, modern()).as_strings();
        // Explicit bitdepth matters: libvips would otherwise default to 12.
        assert_eq!(args[2], "out.avif[Q=50,effort=4,bitdepth=8]");
    }

    #[test]
    fn avif_emits_every_advanced_option_when_enabled() {
        let mut settings = ConversionSettings::default();
        settings.format = OutputFormat::Avif;
        settings.avif.quality = 30;
        settings.avif.lossless = true;
        settings.avif.effort = 9;
        settings.avif.bitdepth = 10;
        settings.avif.subsample = SubsampleMode::On;

        let (input, _) = paths();
        let output = PathBuf::from("out.avif");
        let args = build(&settings, &input, &output, modern()).as_strings();
        assert_eq!(
            args[2],
            "out.avif[Q=30,lossless=true,effort=9,bitdepth=10,subsample-mode=on]"
        );
    }

    #[test]
    fn stripping_metadata_uses_keep_none_on_modern_vips() {
        let mut settings = ConversionSettings::default();
        settings.strip_metadata = true;
        let args = build_strings(&settings, modern());
        assert!(args[2].ends_with(",keep=none]"), "got {}", args[2]);
    }

    #[test]
    fn stripping_metadata_falls_back_to_strip_on_older_vips() {
        let mut settings = ConversionSettings::default();
        settings.strip_metadata = true;
        let args = build_strings(&settings, legacy());
        assert!(args[2].ends_with(",strip=true]"), "got {}", args[2]);
        assert!(
            !args[2].contains("keep=none"),
            "keep=none does not exist before 8.15"
        );
    }

    #[test]
    fn metadata_is_kept_by_default_so_no_option_is_emitted() {
        let settings = ConversionSettings::default();
        let args = build_strings(&settings, modern());
        assert!(!args[2].contains("keep"));
        assert!(!args[2].contains("strip"));
    }

    #[test]
    fn resize_and_options_combine_without_interfering() {
        let mut settings = ConversionSettings::default();
        settings.format = OutputFormat::Avif;
        settings.avif.effort = 6;
        settings.resize.enabled = true;
        settings.resize.max_width = 1920;
        settings.resize.max_height = 1920;
        settings.strip_metadata = true;

        let (input, _) = paths();
        let output = PathBuf::from("out.avif");
        let args = build(&settings, &input, &output, modern()).as_strings();

        assert_eq!(args[0], "thumbnail");
        // The option suffix belongs to the output argument, not the geometry.
        assert_eq!(
            args[2],
            "out.avif[Q=50,effort=6,bitdepth=8,keep=none]"
        );
        assert_eq!(args[3], "1920");
    }

    #[test]
    fn every_format_produces_a_parseable_suffix_for_all_defaults() {
        // Guards against a format being added without option support.
        for format in OutputFormat::ALL {
            let mut settings = ConversionSettings::default();
            settings.format = format;
            let output = PathBuf::from(format!("out.{}", format.extension()));
            let args = build(&settings, Path::new("in.png"), &output, modern()).as_strings();
            // Argument 2 is the output target for both command shapes.
            let target = &args[2];

            assert!(
                target.starts_with(&format!("out.{}", format.extension())),
                "target should start with the output path: {target}"
            );
            assert!(
                target.contains('[') && target.ends_with(']'),
                "every format should emit at least one option: {target}"
            );
            // No empty options and no stray separators.
            let inner = target
                .split_once('[')
                .and_then(|(_, rest)| rest.strip_suffix(']'))
                .expect("suffix should be well formed");
            for pair in inner.split(',') {
                assert!(!pair.is_empty(), "empty option in {target}");
                assert!(
                    pair.contains('='),
                    "option {pair} should be key=value in {target}"
                );
            }
        }
    }

    #[test]
    fn float_formatting_avoids_noisy_representations() {
        assert_eq!(format_float(1.0), "1");
        assert_eq!(format_float(0.0), "0");
        assert_eq!(format_float(0.5), "0.5");
        assert_eq!(format_float(0.25), "0.25");
    }

    #[test]
    fn paths_with_spaces_are_passed_through_untouched() {
        // No shell is involved, so no quoting or escaping should be applied to
        // the actual argument; only the display helper adds quotes.
        let settings = ConversionSettings::default();
        let input = PathBuf::from(r"C:\My Photos\a b.jpg");
        let output = PathBuf::from(r"C:\Out Dir\a b.webp");
        let args = build(&settings, &input, &output, modern());
        let strings = args.as_strings();

        assert_eq!(strings[1], r"C:\My Photos\a b.jpg");
        assert!(strings[2].starts_with(r"C:\Out Dir\a b.webp["));

        let shown = args.to_display_string();
        assert!(shown.contains('"'), "display form should quote: {shown}");
    }
}
