//! The settings side panel: output format, per-format encoder options,
//! resizing, metadata and the overwrite policy.
//!
//! Every control's tooltip names the libvips option it drives, so anything
//! here can be looked up in the libvips reference. Controls that libvips would
//! ignore in the current mode are disabled with an explanation rather than
//! silently doing nothing.

use crate::settings::{
    ConversionSettings, OutputFormat, OverwritePolicy, PerformanceSettings, SubsampleMode,
    WebpPreset,
};
use crate::vips::discovery::Capabilities;

/// Draw the panel. Returns true if any setting changed this frame, which the
/// preview panel uses to know it should re-render.
pub fn show(
    ui: &mut egui::Ui,
    settings: &mut ConversionSettings,
    performance: &mut PerformanceSettings,
    capabilities: Capabilities,
    busy: bool,
) -> bool {
    let mut changed = false;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                changed |= show_format_selector(ui, settings, capabilities);
                ui.add_space(10.0);

                changed |= show_format_options(ui, settings);
                ui.add_space(10.0);

                changed |= show_resize(ui, settings);
                ui.add_space(10.0);

                changed |= show_output_handling(ui, settings);
                ui.add_space(10.0);

                // Parallelism does not change the output, so it never counts as
                // a change for preview purposes.
                show_performance(ui, performance);
            });
        });

    changed
}

/// Parallelism controls.
fn show_performance(ui: &mut egui::Ui, performance: &mut PerformanceSettings) {
    egui::CollapsingHeader::new("Performance")
        .default_open(false)
        .show(ui, |ui| {
            let max_workers = PerformanceSettings::max_workers();

            ui.add(
                egui::Slider::new(&mut performance.workers, 1..=max_workers)
                    .text("Files at once"),
            )
            .on_hover_text(
                "How many images are converted in parallel. \
                 Half the core count is usually fastest, because libvips is \
                 itself multi-threaded.",
            );

            ui.add(
                egui::Slider::new(&mut performance.vips_concurrency, 1..=16)
                    .text("Threads per file"),
            )
            .on_hover_text(
                "VIPS_CONCURRENCY for each child process. \
                 Raise it for a few very large images, lower it for many small ones.",
            );

            ui.label(
                egui::RichText::new(format!(
                    "Up to {} threads in total on {} cores.",
                    performance.workers * performance.vips_concurrency,
                    max_workers
                ))
                .weak()
                .size(11.0),
            );
        });
}

/// Output format, with unsupported formats disabled and explained.
fn show_format_selector(
    ui: &mut egui::Ui,
    settings: &mut ConversionSettings,
    capabilities: Capabilities,
) -> bool {
    let mut changed = false;

    ui.label(egui::RichText::new("Output format").strong());
    ui.add_space(4.0);

    ui.horizontal_wrapped(|ui| {
        for format in OutputFormat::ALL {
            let supported = has_saver(capabilities, format);
            let selected = settings.format == format;

            let response = ui.add_enabled(
                supported,
                egui::RadioButton::new(selected, format.label()),
            );

            let response = if supported {
                response.on_hover_text(format!(
                    "Writes .{} using libvips {}",
                    format.extension(),
                    format.saver_name()
                ))
            } else {
                // Explain rather than leaving a mystery dead control.
                response.on_disabled_hover_text(format!(
                    "This libvips build has no {}, so it cannot write {}.",
                    format.saver_name(),
                    format.label()
                ))
            };

            if response.clicked() && !selected {
                settings.format = format;
                changed = true;
            }
        }
    });

    changed
}

/// Whether the discovered libvips can write this format.
pub fn has_saver(capabilities: Capabilities, format: OutputFormat) -> bool {
    match format {
        OutputFormat::Jpeg => capabilities.jpeg,
        OutputFormat::Png => capabilities.png,
        OutputFormat::Webp => capabilities.webp,
        OutputFormat::Avif => capabilities.avif,
    }
}

/// The option group for whichever format is selected.
fn show_format_options(ui: &mut egui::Ui, settings: &mut ConversionSettings) -> bool {
    let format = settings.format;
    let mut changed = false;

    ui.label(egui::RichText::new(format!("{} options", format.label())).strong());
    ui.add_space(4.0);

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        changed = match format {
            OutputFormat::Jpeg => show_jpeg_options(ui, settings),
            OutputFormat::Png => show_png_options(ui, settings),
            OutputFormat::Webp => show_webp_options(ui, settings),
            OutputFormat::Avif => show_avif_options(ui, settings),
        };
    });

    changed
}

fn show_jpeg_options(ui: &mut egui::Ui, settings: &mut ConversionSettings) -> bool {
    let o = &mut settings.jpeg;
    let mut changed = false;

    changed |= slider(ui, &mut o.quality, 1..=100, "Quality", "Q", None);

    changed |= checkbox(
        ui,
        &mut o.optimize_coding,
        "Optimise Huffman tables",
        "optimize-coding",
        "Computes optimal coding tables. Slightly smaller files for a little more CPU.",
    );

    changed |= checkbox(
        ui,
        &mut o.progressive,
        "Progressive",
        "interlace",
        "Renders in successive passes while loading. Usually slightly smaller too.",
    );

    // libvips only applies scan optimisation to progressive JPEGs, so the
    // control is disabled rather than quietly ignored.
    let scans_enabled = o.progressive;
    let response = ui.add_enabled(
        scans_enabled,
        egui::Checkbox::new(&mut o.optimize_scans, "Optimise scans"),
    );
    if response.changed() {
        changed = true;
    }
    if scans_enabled {
        response.on_hover_text("optimize-scans: splits DCT coefficients into separate scans.");
    } else {
        response.on_disabled_hover_text(
            "optimize-scans only applies to progressive JPEGs. Turn on Progressive first.",
        );
    }

    changed |= checkbox(
        ui,
        &mut o.trellis_quant,
        "Trellis quantisation",
        "trellis-quant",
        "Optimises quantisation per 8x8 block. Smaller files, noticeably slower.",
    );

    changed |= checkbox(
        ui,
        &mut o.overshoot_deringing,
        "Overshoot deringing",
        "overshoot-deringing",
        "Reduces ringing artefacts around hard edges, such as text on flat colour.",
    );

    changed |= slider(
        ui,
        &mut o.quant_table,
        0..=8,
        "Quantisation table",
        "quant-table",
        Some("0 is the JPEG standard table. 3 often suits photographs better."),
    );

    changed |= enum_combo(
        ui,
        "jpeg_subsample",
        &mut o.subsample,
        SubsampleMode::ALL,
        SubsampleMode::label,
        "Chroma subsampling",
        "subsample-mode",
    );

    changed
}

fn show_png_options(ui: &mut egui::Ui, settings: &mut ConversionSettings) -> bool {
    let o = &mut settings.png;
    let mut changed = false;

    changed |= slider(
        ui,
        &mut o.compression,
        0..=9,
        "Compression",
        "compression",
        Some("zlib level. Higher is smaller and slower, and always lossless."),
    );

    changed |= checkbox(
        ui,
        &mut o.interlace,
        "Interlace (Adam7)",
        "interlace",
        "Progressive rendering. Usually makes PNG files noticeably larger.",
    );

    ui.add_space(4.0);
    changed |= checkbox(
        ui,
        &mut o.palette,
        "Quantise to a palette",
        "palette",
        "Lossy, but by far the biggest size win available for PNG.",
    );

    // The quantiser controls only exist in palette mode.
    let palette = o.palette;
    ui.add_enabled_ui(palette, |ui| {
        ui.indent("png_palette_options", |ui| {
            changed |= slider(
                ui,
                &mut o.quality,
                0..=100,
                "Palette quality",
                "Q",
                Some("Target quality for the quantiser."),
            );

            let response = ui.add(
                egui::Slider::new(&mut o.dither, 0.0..=1.0)
                    .text("Dither")
                    .fixed_decimals(2),
            );
            if response.changed() {
                changed = true;
            }
            response.on_hover_text("dither: 0 is flat colour banding, 1 is full dithering.");

            changed |= slider(
                ui,
                &mut o.effort,
                1..=10,
                "Quantiser effort",
                "effort",
                Some("CPU spent searching for a better palette."),
            );
        });
    });
    if !palette {
        ui.indent("png_palette_hint", |ui| {
            ui.label(
                egui::RichText::new("Palette options apply only when quantising.")
                    .weak()
                    .size(11.0),
            );
        });
    }

    ui.add_space(4.0);
    changed |= bitdepth_combo(
        ui,
        "png_bitdepth",
        &mut o.bitdepth,
        &[1, 2, 4, 8, 16],
        "Bit depth",
        "bitdepth",
        "Bits per channel. Below 8 requires a palette to look sensible.",
    );

    changed
}

fn show_webp_options(ui: &mut egui::Ui, settings: &mut ConversionSettings) -> bool {
    let o = &mut settings.webp;
    let mut changed = false;

    changed |= checkbox(
        ui,
        &mut o.lossless,
        "Lossless",
        "lossless",
        "Exact pixel reproduction. Much larger than lossy WebP.",
    );

    changed |= checkbox(
        ui,
        &mut o.near_lossless,
        "Near-lossless",
        "near-lossless",
        "Lossless compression with lossy pre-processing, steered by Quality.",
    );

    // In pure lossless mode Q selects the compression/speed trade-off rather
    // than visual quality, so keep it available but explain the difference.
    let quality_meaning = if o.lossless && !o.near_lossless {
        "Q: in lossless mode this trades compression against speed, not fidelity."
    } else {
        "Q: visual quality. 80 is a good default for photographs."
    };
    let response = ui.add(egui::Slider::new(&mut o.quality, 0..=100).text("Quality"));
    if response.changed() {
        changed = true;
    }
    response.on_hover_text(quality_meaning);

    changed |= slider(
        ui,
        &mut o.effort,
        0..=6,
        "Effort",
        "effort",
        Some("CPU spent reducing size. 6 is slowest and smallest."),
    );

    // Alpha quality is a lossy-mode control only.
    let alpha_enabled = !o.lossless;
    ui.add_enabled_ui(alpha_enabled, |ui| {
        changed |= slider(
            ui,
            &mut o.alpha_quality,
            0..=100,
            "Alpha quality",
            "alpha-q",
            Some("Fidelity of the transparency channel."),
        );
    });
    if !alpha_enabled {
        ui.indent("webp_alpha_hint", |ui| {
            ui.label(
                egui::RichText::new("Alpha quality does not apply to lossless WebP.")
                    .weak()
                    .size(11.0),
            );
        });
    }

    changed |= checkbox(
        ui,
        &mut o.smart_subsample,
        "Smart subsampling",
        "smart-subsample",
        "Higher quality chroma subsampling. Slower, slightly larger.",
    );

    changed |= checkbox(
        ui,
        &mut o.min_size,
        "Optimise for minimum size",
        "min-size",
        "Searches harder for the smallest file. Considerably slower.",
    );

    changed |= enum_combo(
        ui,
        "webp_preset",
        &mut o.preset,
        WebpPreset::ALL,
        WebpPreset::label,
        "Preset",
        "preset",
    );

    changed
}

fn show_avif_options(ui: &mut egui::Ui, settings: &mut ConversionSettings) -> bool {
    let o = &mut settings.avif;
    let mut changed = false;

    changed |= slider(
        ui,
        &mut o.quality,
        1..=100,
        "Quality",
        "Q",
        Some("AVIF Q30 is roughly comparable to JPEG Q75."),
    );

    changed |= checkbox(
        ui,
        &mut o.lossless,
        "Lossless",
        "lossless",
        "Exact pixel reproduction. Very slow and very large.",
    );

    changed |= slider(
        ui,
        &mut o.effort,
        0..=9,
        "Effort",
        "effort",
        Some("The main size lever for AVIF, and the main cost. 9 can be very slow."),
    );

    // Warn about the real-world cost of high effort on a batch.
    if o.effort >= 7 {
        ui.label(
            egui::RichText::new("Effort 7+ can take many seconds per image.")
                .color(ui.visuals().warn_fg_color)
                .size(11.0),
        );
    }

    changed |= bitdepth_combo(
        ui,
        "avif_bitdepth",
        &mut o.bitdepth,
        &[8, 10, 12],
        "Bit depth",
        "bitdepth",
        "8-bit is the most widely supported. libvips itself defaults to 12, \
         which some decoders reject.",
    );

    changed |= enum_combo(
        ui,
        "avif_subsample",
        &mut o.subsample,
        SubsampleMode::ALL,
        SubsampleMode::label,
        "Chroma subsampling",
        "subsample-mode",
    );

    changed
}

/// Resize controls.
fn show_resize(ui: &mut egui::Ui, settings: &mut ConversionSettings) -> bool {
    let mut changed = false;

    ui.label(egui::RichText::new("Resize").strong());
    ui.add_space(4.0);

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());

        let r = &mut settings.resize;
        let response = ui.checkbox(&mut r.enabled, "Fit inside a bounding box");
        if response.changed() {
            changed = true;
        }
        response.on_hover_text(
            "Uses libvips `thumbnail`, which shrinks on load so large sources stay cheap.",
        );

        ui.add_enabled_ui(r.enabled, |ui| {
            ui.horizontal(|ui| {
                ui.label("Max width");
                let response = ui.add(
                    egui::DragValue::new(&mut r.max_width)
                        .range(1..=100_000)
                        .suffix(" px"),
                );
                if response.changed() {
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Max height");
                let response = ui.add(
                    egui::DragValue::new(&mut r.max_height)
                        .range(1..=100_000)
                        .suffix(" px"),
                );
                if response.changed() {
                    changed = true;
                }
            });

            let response = ui.checkbox(&mut r.never_upscale, "Never enlarge");
            if response.changed() {
                changed = true;
            }
            response.on_hover_text(
                "`--size down`: images already smaller than the box are left at their own size.",
            );

            ui.label(
                egui::RichText::new("Aspect ratio is always preserved.")
                    .weak()
                    .size(11.0),
            );
        });
    });

    changed
}

/// Metadata and collision handling.
fn show_output_handling(ui: &mut egui::Ui, settings: &mut ConversionSettings) -> bool {
    let mut changed = false;

    ui.label(egui::RichText::new("Output").strong());
    ui.add_space(4.0);

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());

        let response = ui.checkbox(&mut settings.strip_metadata, "Strip metadata");
        if response.changed() {
            changed = true;
        }
        response.on_hover_text(
            "Removes EXIF, XMP, IPTC and colour profiles. \
             Uses `keep=none`, or `strip` on libvips before 8.15.",
        );

        ui.add_space(6.0);
        ui.label("If the output file already exists:");
        for policy in OverwritePolicy::ALL {
            let selected = settings.overwrite == policy;
            let response = ui.radio(selected, policy.label());
            if response.clicked() && !selected {
                settings.overwrite = policy;
                changed = true;
            }
            let tip = match policy {
                OverwritePolicy::Rename => {
                    "Writes \"name (1).ext\". Safe: it can never overwrite an input."
                }
                OverwritePolicy::Skip => "Leaves the existing file and marks the job skipped.",
                OverwritePolicy::Overwrite => {
                    "Replaces the existing file. Converting to the same format \
                     in place will destroy the original."
                }
            };
            response.on_hover_text(tip);
        }

        // Overwriting is the one policy that can lose data, so say so plainly.
        if settings.overwrite == OverwritePolicy::Overwrite {
            ui.label(
                egui::RichText::new("Existing files will be replaced without warning.")
                    .color(ui.visuals().warn_fg_color)
                    .size(11.0),
            );
        }
    });

    changed
}

// --- small widget helpers -------------------------------------------------

/// A labelled integer slider whose tooltip names the vips option.
fn slider<T>(
    ui: &mut egui::Ui,
    value: &mut T,
    range: std::ops::RangeInclusive<T>,
    label: &str,
    vips_name: &str,
    extra: Option<&str>,
) -> bool
where
    T: egui::emath::Numeric,
{
    let response = ui.add(egui::Slider::new(value, range).text(label));
    let changed = response.changed();
    let tip = match extra {
        Some(extra) => format!("{vips_name}: {extra}"),
        None => format!("libvips option `{vips_name}`"),
    };
    response.on_hover_text(tip);
    changed
}

/// A checkbox whose tooltip names the vips option.
fn checkbox(
    ui: &mut egui::Ui,
    value: &mut bool,
    label: &str,
    vips_name: &str,
    explanation: &str,
) -> bool {
    let response = ui.checkbox(value, label);
    let changed = response.changed();
    response.on_hover_text(format!("{vips_name}: {explanation}"));
    changed
}

/// A combo box over a fixed set of enum values.
fn enum_combo<T, const N: usize>(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut T,
    options: [T; N],
    label_of: fn(T) -> &'static str,
    label: &str,
    vips_name: &str,
) -> bool
where
    T: Copy + PartialEq,
{
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(id)
            .selected_text(label_of(*value))
            .show_ui(ui, |ui| {
                for option in options {
                    if ui
                        .selectable_label(*value == option, label_of(option))
                        .clicked()
                        && *value != option
                    {
                        *value = option;
                        changed = true;
                    }
                }
            });
    })
    .response
    .on_hover_text(format!("libvips option `{vips_name}`"));
    changed
}

/// A combo box over a fixed set of allowed bit depths.
fn bitdepth_combo(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut u8,
    allowed: &[u8],
    label: &str,
    vips_name: &str,
    explanation: &str,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(id)
            .selected_text(format!("{value}-bit"))
            .show_ui(ui, |ui| {
                for &depth in allowed {
                    if ui
                        .selectable_label(*value == depth, format!("{depth}-bit"))
                        .clicked()
                        && *value != depth
                    {
                        *value = depth;
                        changed = true;
                    }
                }
            });
    })
    .response
    .on_hover_text(format!("{vips_name}: {explanation}"));
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saver_lookup_maps_each_format_to_its_capability_flag() {
        let caps = Capabilities {
            jpeg: true,
            png: false,
            webp: true,
            avif: false,
        };
        assert!(has_saver(caps, OutputFormat::Jpeg));
        assert!(!has_saver(caps, OutputFormat::Png));
        assert!(has_saver(caps, OutputFormat::Webp));
        assert!(!has_saver(caps, OutputFormat::Avif));
    }

    #[test]
    fn every_format_is_offered_when_the_build_is_complete() {
        let caps = Capabilities {
            jpeg: true,
            png: true,
            webp: true,
            avif: true,
        };
        for format in OutputFormat::ALL {
            assert!(has_saver(caps, format), "{} should be offered", format.label());
        }
    }
}
