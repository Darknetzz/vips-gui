//! Conversion settings: the user's choices, in a form the command builder can
//! translate directly into vips arguments.
//!
//! Option names in the doc comments match the libvips save option names, so a
//! reader can look any of them up in the libvips reference. Defaults here match
//! the libvips defaults except where noted.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Chroma subsampling control, shared by `jpegsave` and `heifsave`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SubsampleMode {
    /// `subsample-mode=auto`: libvips disables subsampling at Q >= 90.
    #[default]
    Auto,
    /// `subsample-mode=on`: always subsample (smaller, softer colour).
    On,
    /// `subsample-mode=off`: never subsample (larger, sharper colour).
    Off,
}

impl SubsampleMode {
    /// The value libvips expects.
    pub fn as_vips(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto (off above Q90)",
            Self::On => "Always on",
            Self::Off => "Always off",
        }
    }

    pub const ALL: [Self; 3] = [Self::Auto, Self::On, Self::Off];
}

/// `webpsave` tuning preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WebpPreset {
    #[default]
    Default,
    Picture,
    Photo,
    Drawing,
    Icon,
    Text,
}

impl WebpPreset {
    pub fn as_vips(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Picture => "picture",
            Self::Photo => "photo",
            Self::Drawing => "drawing",
            Self::Icon => "icon",
            Self::Text => "text",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Picture => "Picture",
            Self::Photo => "Photo",
            Self::Drawing => "Drawing",
            Self::Icon => "Icon",
            Self::Text => "Text",
        }
    }

    pub const ALL: [Self; 6] = [
        Self::Default,
        Self::Picture,
        Self::Photo,
        Self::Drawing,
        Self::Icon,
        Self::Text,
    ];
}

/// JPEG encoder settings (`jpegsave`).
/// `#[serde(default)]` on each option struct, not just the top level: a config
/// written by a version that lacked one of these fields must still load, and
/// without it the whole settings tree would silently reset to defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JpegOptions {
    /// `Q`, 1-100.
    pub quality: u8,
    /// `optimize-coding`: compute optimal Huffman tables. Smaller, slower.
    pub optimize_coding: bool,
    /// `interlace`: progressive JPEG.
    pub progressive: bool,
    /// `optimize-scans`: split DCT coefficients into separate scans.
    /// Only meaningful for progressive JPEGs.
    pub optimize_scans: bool,
    /// `trellis-quant`: trellis quantisation per 8x8 block.
    pub trellis_quant: bool,
    /// `overshoot-deringing`: reduce ringing around hard edges.
    pub overshoot_deringing: bool,
    /// `quant-table`, 0-8. Table 3 is often better for photos than the default.
    pub quant_table: u8,
    /// `subsample-mode`.
    pub subsample: SubsampleMode,
}

impl Default for JpegOptions {
    fn default() -> Self {
        Self {
            quality: 80, // libvips defaults to 75; 80 is a friendlier starting point.
            optimize_coding: true, // Free size win for a little CPU.
            progressive: true,
            optimize_scans: false,
            trellis_quant: false,
            overshoot_deringing: false,
            quant_table: 0,
            subsample: SubsampleMode::Auto,
        }
    }
}

/// PNG encoder settings (`pngsave`).
///
/// Not `Eq`: `dither` is a float, matching the libvips option type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PngOptions {
    /// `compression`, 0-9 (zlib level).
    pub compression: u8,
    /// `interlace`: Adam7. Usually makes files bigger.
    pub interlace: bool,
    /// `palette`: quantise to a palette. This is what makes PNGs small.
    pub palette: bool,
    /// `Q`, 0-100: palette quantisation quality. Only used when `palette`.
    pub quality: u8,
    /// `dither`, 0.0-1.0. Only used when `palette`.
    pub dither: f32,
    /// `bitdepth`: 1, 2, 4, 8 or 16.
    pub bitdepth: u8,
    /// `effort`, 1-10: quantisation CPU effort. Only used when `palette`.
    pub effort: u8,
}

impl Default for PngOptions {
    fn default() -> Self {
        Self {
            compression: 6,
            interlace: false,
            palette: false,
            quality: 100,
            dither: 1.0,
            bitdepth: 8,
            effort: 7,
        }
    }
}

/// WebP encoder settings (`webpsave`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebpOptions {
    /// `Q`, 0-100. In near-lossless mode this selects the preprocessing level.
    pub quality: u8,
    /// `lossless`.
    pub lossless: bool,
    /// `near-lossless`: lossless with lossy preprocessing, driven by `Q`.
    pub near_lossless: bool,
    /// `effort`, 0-6.
    pub effort: u8,
    /// `alpha-q`, 0-100: alpha channel fidelity in lossy mode.
    pub alpha_quality: u8,
    /// `smart-subsample`: higher quality chroma subsampling.
    pub smart_subsample: bool,
    /// `preset`.
    pub preset: WebpPreset,
    /// `min-size`: optimise hard for size. Slow.
    pub min_size: bool,
}

impl Default for WebpOptions {
    fn default() -> Self {
        Self {
            quality: 80,
            lossless: false,
            near_lossless: false,
            effort: 4,
            alpha_quality: 100,
            smart_subsample: false,
            preset: WebpPreset::Default,
            min_size: false,
        }
    }
}

/// AVIF encoder settings (`heifsave` with an `.avif` target).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AvifOptions {
    /// `Q`, 1-100. AVIF Q30 is roughly JPEG Q75.
    pub quality: u8,
    /// `lossless`.
    pub lossless: bool,
    /// `effort`, 0-9. Slow but effective; this is the main size lever.
    pub effort: u8,
    /// `bitdepth`: 8, 10 or 12.
    pub bitdepth: u8,
    /// `subsample-mode`.
    pub subsample: SubsampleMode,
}

impl Default for AvifOptions {
    fn default() -> Self {
        Self {
            quality: 50,
            lossless: false,
            effort: 4,
            // libvips defaults heifsave bitdepth to 12. We default to 8
            // because 12-bit AVIF is rejected or mis-rendered by a fair
            // number of consumers.
            bitdepth: 8,
            subsample: SubsampleMode::Auto,
        }
    }
}

/// Output image format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Hash)]
pub enum OutputFormat {
    Jpeg,
    #[default]
    Webp,
    Png,
    Avif,
}

impl OutputFormat {
    /// File extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Avif => "avif",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Jpeg => "JPEG",
            Self::Png => "PNG",
            Self::Webp => "WebP",
            Self::Avif => "AVIF",
        }
    }

    /// The libvips saver this format uses, for capability checks and messages.
    pub fn saver_name(self) -> &'static str {
        match self {
            Self::Jpeg => "jpegsave",
            Self::Png => "pngsave",
            Self::Webp => "webpsave",
            Self::Avif => "heifsave",
        }
    }

    pub const ALL: [Self; 4] = [Self::Jpeg, Self::Png, Self::Webp, Self::Avif];
}

/// Downscaling settings. Upscaling is never done unless explicitly allowed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResizeSettings {
    pub enabled: bool,
    /// Bounding box width in pixels.
    pub max_width: u32,
    /// Bounding box height in pixels.
    pub max_height: u32,
    /// When true, images smaller than the box are left alone
    /// (`--size down`). When false, they are enlarged (`--size both`).
    pub never_upscale: bool,
}

impl Default for ResizeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            max_width: 1920,
            max_height: 1920,
            never_upscale: true,
        }
    }
}

/// What to do when the output path already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OverwritePolicy {
    /// Leave the existing file alone and report the job as skipped.
    Skip,
    /// Replace it.
    Overwrite,
    /// Write `name (1).ext`, `name (2).ext`, and so on.
    ///
    /// The default, because it makes a same-format conversion (jpg -> jpg into
    /// the source folder) incapable of destroying the input.
    #[default]
    Rename,
}

impl OverwritePolicy {
    pub fn label(self) -> &'static str {
        match self {
            Self::Skip => "Skip existing files",
            Self::Overwrite => "Overwrite existing files",
            Self::Rename => "Rename to avoid collisions",
        }
    }

    pub const ALL: [Self; 3] = [Self::Rename, Self::Skip, Self::Overwrite];
}

/// Where converted files go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OutputDestination {
    /// Alongside each input file. The default: no directory picking needed to
    /// get started.
    #[default]
    SameAsSource,
    /// A single directory for every output.
    Directory(PathBuf),
}

/// How much of the machine to use for a batch.
///
/// Kept apart from [`ConversionSettings`] because it does not affect the
/// output, only how fast it is produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerformanceSettings {
    /// Number of files converted at once.
    pub workers: usize,
    /// Thread pool size inside each `vips` child (`VIPS_CONCURRENCY`).
    pub vips_concurrency: usize,
}

impl Default for PerformanceSettings {
    fn default() -> Self {
        Self {
            workers: crate::worker::default_worker_count(),
            vips_concurrency: crate::worker::DEFAULT_VIPS_CONCURRENCY,
        }
    }
}

impl PerformanceSettings {
    /// Upper bound offered in the UI. Beyond the core count there is nothing
    /// to gain and plenty of contention to lose.
    pub fn max_workers() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .max(1)
    }

    /// Clamp to something this machine can sensibly run, in case a persisted
    /// value came from a very different computer.
    pub fn sanitised(self) -> Self {
        Self {
            workers: self.workers.clamp(1, Self::max_workers().max(1)),
            vips_concurrency: self.vips_concurrency.clamp(1, 32),
        }
    }
}

/// The complete set of conversion choices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ConversionSettings {
    pub format: OutputFormat,
    pub jpeg: JpegOptions,
    pub png: PngOptions,
    pub webp: WebpOptions,
    pub avif: AvifOptions,
    pub resize: ResizeSettings,
    /// Drop EXIF, XMP, IPTC, ICC and friends (`keep=none`, or `strip` on
    /// libvips older than 8.15).
    pub strip_metadata: bool,
    pub destination: OutputDestination,
    pub overwrite: OverwritePolicy,
}

impl ConversionSettings {
    /// Switch away from a format the installed libvips cannot write.
    ///
    /// Matters on startup: settings persist across runs, but the libvips on
    /// PATH can change. Someone who last used AVIF and then moved to a build
    /// without libheif would otherwise open the app with a format selected that
    /// the Convert button refuses to act on.
    ///
    /// Returns the format that was abandoned, if any.
    pub fn ensure_format_supported(
        &mut self,
        supported: impl Fn(OutputFormat) -> bool,
    ) -> Option<OutputFormat> {
        if supported(self.format) {
            return None;
        }

        let unsupported = self.format;
        // Prefer the declared order, so the fallback is predictable.
        if let Some(replacement) = OutputFormat::ALL.into_iter().find(|f| supported(*f)) {
            self.format = replacement;
        }
        // With no supported format at all, leave the selection alone: the
        // toolbar explains the problem, and changing it would achieve nothing.
        Some(unsupported)
    }

    /// Resolve the output path for one input, before collision handling.
    pub fn output_path_for(&self, input: &std::path::Path) -> PathBuf {
        let stem = input.file_stem().unwrap_or_default();
        let file_name = format!(
            "{}.{}",
            stem.to_string_lossy(),
            self.format.extension()
        );

        match &self.destination {
            OutputDestination::SameAsSource => input
                .parent()
                .map(|dir| dir.join(&file_name))
                .unwrap_or_else(|| PathBuf::from(&file_name)),
            OutputDestination::Directory(dir) => dir.join(&file_name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn defaults_are_sensible_for_a_first_run() {
        let s = ConversionSettings::default();
        // WebP is the best all-round default: broad support, good ratio.
        assert_eq!(s.format, OutputFormat::Webp);
        // Rename protects the input from a same-format conversion.
        assert_eq!(s.overwrite, OverwritePolicy::Rename);
        assert_eq!(s.destination, OutputDestination::SameAsSource);
        assert!(!s.resize.enabled, "resize should be opt-in");
        assert!(!s.strip_metadata, "metadata should be kept unless asked");
    }

    #[test]
    fn avif_defaults_to_8_bit_not_the_libvips_12() {
        // 12-bit AVIF trips up a lot of decoders, so we deliberately diverge
        // from the libvips default here.
        assert_eq!(AvifOptions::default().bitdepth, 8);
    }

    #[test]
    fn extensions_match_the_formats() {
        assert_eq!(OutputFormat::Jpeg.extension(), "jpg");
        assert_eq!(OutputFormat::Png.extension(), "png");
        assert_eq!(OutputFormat::Webp.extension(), "webp");
        assert_eq!(OutputFormat::Avif.extension(), "avif");
    }

    #[test]
    fn output_path_next_to_the_source_swaps_the_extension() {
        let mut s = ConversionSettings::default();
        s.format = OutputFormat::Avif;
        let got = s.output_path_for(Path::new("/photos/holiday/beach.jpeg"));
        assert_eq!(got, Path::new("/photos/holiday/beach.avif"));
    }

    #[test]
    fn output_path_into_a_fixed_directory_keeps_only_the_file_name() {
        let mut s = ConversionSettings::default();
        s.format = OutputFormat::Png;
        s.destination = OutputDestination::Directory(PathBuf::from("/out"));
        let got = s.output_path_for(Path::new("/deep/nested/source/pic.jpg"));
        assert_eq!(got, Path::new("/out/pic.png"));
    }

    #[test]
    fn output_path_handles_names_with_several_dots() {
        let mut s = ConversionSettings::default();
        s.format = OutputFormat::Webp;
        // Only the final extension should be replaced.
        let got = s.output_path_for(Path::new("/a/my.photo.v2.jpg"));
        assert_eq!(got, Path::new("/a/my.photo.v2.webp"));
    }

    #[test]
    fn output_path_handles_a_file_with_no_extension() {
        let mut s = ConversionSettings::default();
        s.format = OutputFormat::Webp;
        let got = s.output_path_for(Path::new("/a/noextension"));
        assert_eq!(got, Path::new("/a/noextension.webp"));
    }

    #[test]
    fn a_supported_format_is_left_alone() {
        let mut s = ConversionSettings::default();
        s.format = OutputFormat::Avif;
        assert_eq!(
            s.ensure_format_supported(|_| true),
            None,
            "nothing should change when the format is available"
        );
        assert_eq!(s.format, OutputFormat::Avif);
    }

    #[test]
    fn an_unsupported_format_falls_back_to_an_available_one() {
        let mut s = ConversionSettings::default();
        s.format = OutputFormat::Avif;

        // A libvips without libheif.
        let abandoned = s.ensure_format_supported(|f| f != OutputFormat::Avif);
        assert_eq!(abandoned, Some(OutputFormat::Avif));
        assert_ne!(s.format, OutputFormat::Avif);
        // The declared order puts JPEG first, so the fallback is predictable.
        assert_eq!(s.format, OutputFormat::Jpeg);
    }

    #[test]
    fn the_fallback_picks_the_first_available_format_in_order() {
        let mut s = ConversionSettings::default();
        s.format = OutputFormat::Avif;
        // Only WebP available.
        s.ensure_format_supported(|f| f == OutputFormat::Webp);
        assert_eq!(s.format, OutputFormat::Webp);
    }

    #[test]
    fn with_no_supported_format_the_selection_is_left_for_the_ui_to_explain() {
        let mut s = ConversionSettings::default();
        s.format = OutputFormat::Avif;
        let abandoned = s.ensure_format_supported(|_| false);
        assert_eq!(abandoned, Some(OutputFormat::Avif));
        // Changing it would be pointless; the toolbar reports the real problem.
        assert_eq!(s.format, OutputFormat::Avif);
    }

    #[test]
    fn settings_survive_a_serde_round_trip() {
        let mut original = ConversionSettings::default();
        original.format = OutputFormat::Avif;
        original.avif.effort = 7;
        original.resize.enabled = true;
        original.resize.max_width = 1280;
        original.strip_metadata = true;

        let json = serde_json::to_string(&original).expect("serialize");
        let restored: ConversionSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn a_config_from_a_newer_version_still_loads() {
        // Unknown keys, at the top level and nested, must be ignored rather
        // than throwing away the settings the user has.
        let json = r#"{
            "format": "Avif",
            "avif": { "quality": 33, "effort": 7, "future_option": "surprise" },
            "resize": { "enabled": true, "max_width": 1280 },
            "brand_new_section": { "anything": [1, 2, 3] },
            "strip_metadata": true
        }"#;

        let restored: ConversionSettings =
            serde_json::from_str(json).expect("unknown fields should be ignored");

        assert_eq!(restored.format, OutputFormat::Avif);
        assert_eq!(restored.avif.quality, 33);
        assert_eq!(restored.avif.effort, 7);
        assert!(restored.strip_metadata);
        assert!(restored.resize.enabled);
        assert_eq!(restored.resize.max_width, 1280);
        // Fields the JSON omitted fall back to defaults, not to zero.
        assert_eq!(
            restored.avif.bitdepth,
            AvifOptions::default().bitdepth,
            "an omitted nested field should use its default"
        );
        assert_eq!(
            restored.resize.max_height,
            ResizeSettings::default().max_height
        );
        assert_eq!(restored.jpeg, JpegOptions::default());
    }

    #[test]
    fn a_config_from_an_older_version_missing_whole_sections_still_loads() {
        // The very first release might have persisted only a format.
        let json = r#"{ "format": "Png" }"#;
        let restored: ConversionSettings =
            serde_json::from_str(json).expect("missing sections should default");

        assert_eq!(restored.format, OutputFormat::Png);
        assert_eq!(restored.png, PngOptions::default());
        assert_eq!(restored.webp, WebpOptions::default());
        assert_eq!(restored.overwrite, OverwritePolicy::default());
        assert_eq!(restored.destination, OutputDestination::SameAsSource);
    }

    #[test]
    fn an_empty_config_object_yields_the_defaults() {
        let restored: ConversionSettings =
            serde_json::from_str("{}").expect("an empty object should be valid");
        assert_eq!(restored, ConversionSettings::default());
    }

    #[test]
    fn performance_settings_round_trip_and_ignore_unknown_fields() {
        let original = PerformanceSettings {
            workers: 3,
            vips_concurrency: 4,
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: PerformanceSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);

        let with_extra: PerformanceSettings =
            serde_json::from_str(r#"{ "workers": 2, "something_new": true }"#)
                .expect("unknown fields should be ignored");
        assert_eq!(with_extra.workers, 2);
        assert_eq!(
            with_extra.vips_concurrency,
            PerformanceSettings::default().vips_concurrency
        );
    }

    #[test]
    fn performance_settings_from_another_machine_are_clamped() {
        // A config written on a 64-core machine, loaded on a small one.
        let absurd = PerformanceSettings {
            workers: 512,
            vips_concurrency: 999,
        };
        let sane = absurd.sanitised();
        assert!(sane.workers >= 1);
        assert!(
            sane.workers <= PerformanceSettings::max_workers(),
            "{} workers on a {}-core machine",
            sane.workers,
            PerformanceSettings::max_workers()
        );
        assert!((1..=32).contains(&sane.vips_concurrency));

        // And nonsense in the other direction.
        let zeroed = PerformanceSettings {
            workers: 0,
            vips_concurrency: 0,
        };
        let sane = zeroed.sanitised();
        assert_eq!(sane.workers, 1, "must always have at least one worker");
        assert_eq!(sane.vips_concurrency, 1);
    }

    #[test]
    fn a_persisted_output_directory_survives_a_round_trip() {
        let mut original = ConversionSettings::default();
        original.destination = OutputDestination::Directory(PathBuf::from(r"D:\Pictures\Out"));

        let json = serde_json::to_string(&original).expect("serialize");
        let restored: ConversionSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            restored.destination,
            OutputDestination::Directory(PathBuf::from(r"D:\Pictures\Out")),
            "the chosen output folder should be remembered between runs"
        );
    }
}
