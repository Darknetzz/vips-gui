//! The preview panel: the selected image before and after conversion, with a
//! measured prediction of the output file size.

use crate::app::format_bytes;
use crate::vips::preview::{Preview, PreviewError};

/// A decoded preview, held as GPU textures so it is not re-uploaded per frame.
pub struct PreviewTextures {
    pub source: egui::TextureHandle,
    pub result: egui::TextureHandle,
}

/// What the panel is currently able to show.
pub enum PreviewState<'a> {
    /// No file is selected.
    NoSelection,
    /// A preview is being produced.
    Rendering { file_name: String },
    /// Ready.
    Ready {
        file_name: String,
        preview: &'a Preview,
        textures: &'a PreviewTextures,
    },
    /// Something went wrong.
    Failed {
        file_name: String,
        error: &'a PreviewError,
    },
}

/// Draw the panel.
pub fn show(ui: &mut egui::Ui, state: PreviewState<'_>) {
    ui.label(egui::RichText::new("Preview").strong());
    ui.add_space(6.0);

    match state {
        PreviewState::NoSelection => {
            ui.label(
                egui::RichText::new("Select a file in the list to preview it.")
                    .weak()
                    .size(12.0),
            );
        }
        PreviewState::Rendering { file_name } => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(
                    egui::RichText::new(format!("Encoding {file_name}..."))
                        .weak()
                        .size(12.0),
                );
            });
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "The output is encoded at full size, so the predicted file size is exact.",
                )
                .weak()
                .size(11.0),
            );
        }
        PreviewState::Failed { file_name, error } => {
            ui.label(
                egui::RichText::new(format!("Could not preview {file_name}"))
                    .color(ui.visuals().error_fg_color)
                    .size(12.0),
            );
            ui.add_space(4.0);
            ui.label(egui::RichText::new(error.to_string()).size(12.0));
        }
        PreviewState::Ready {
            file_name,
            preview,
            textures,
        } => show_ready(ui, &file_name, preview, textures),
    }
}

/// The populated panel.
fn show_ready(
    ui: &mut egui::Ui,
    file_name: &str,
    preview: &Preview,
    textures: &PreviewTextures,
) {
    ui.add(
        egui::Label::new(egui::RichText::new(file_name).size(12.0))
            .truncate(),
    );
    ui.add_space(6.0);

    // The size comparison is the headline number, so it goes first.
    show_size_summary(ui, preview);
    ui.add_space(8.0);

    // Images stacked vertically: the panel is tall and narrow, and stacking
    // keeps each image as wide as possible for judging artefacts.
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            show_labelled_image(
                ui,
                "Original",
                &textures.source,
                preview.source_dimensions,
                preview.source_bytes,
            );
            ui.add_space(10.0);
            show_labelled_image(
                ui,
                "Converted",
                &textures.result,
                preview.result_dimensions,
                preview.predicted_bytes,
            );
        });
}

/// The before/after size line.
fn show_size_summary(ui: &mut egui::Ui, preview: &Preview) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Predicted size").weak().size(11.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format_bytes(preview.predicted_bytes))
                        .strong()
                        .size(14.0),
                );
            });
        });

        if let Some(delta) = preview.size_delta_percent() {
            let smaller = delta < 0.0;
            // Green for a saving, amber for growth, with the sign in the text so
            // colour is never the only signal.
            let colour = if smaller {
                egui::Color32::from_rgb(0x4c, 0xaf, 0x50)
            } else {
                ui.visuals().warn_fg_color
            };
            let text = if smaller {
                format!(
                    "{:.0}% smaller than the original ({})",
                    -delta,
                    format_bytes(preview.source_bytes)
                )
            } else {
                format!(
                    "{delta:.0}% larger than the original ({})",
                    format_bytes(preview.source_bytes)
                )
            };
            ui.label(egui::RichText::new(text).color(colour).size(11.0));
        }

        ui.label(
            egui::RichText::new("Measured from a real encode at the current settings.")
                .weak()
                .size(10.0),
        );
    });
}

/// One captioned image, scaled to the panel width.
fn show_labelled_image(
    ui: &mut egui::Ui,
    caption: &str,
    texture: &egui::TextureHandle,
    dimensions: Option<(u32, u32)>,
    bytes: u64,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(caption).strong().size(12.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let detail = match dimensions {
                Some((w, h)) => format!("{w}x{h} · {}", format_bytes(bytes)),
                None => format_bytes(bytes),
            };
            ui.label(egui::RichText::new(detail).weak().size(11.0));
        });
    });
    ui.add_space(2.0);

    // Fit to the available width without enlarging beyond the rendered
    // resolution, which would only show interpolation blur.
    let size = texture.size_vec2();
    let available = ui.available_width().max(1.0);
    let scale = (available / size.x).min(1.0);

    ui.add(
        egui::Image::new(texture)
            .fit_to_exact_size(size * scale)
            .corner_radius(2.0),
    );
}

/// Decode PNG bytes into a texture.
///
/// Preview bytes always come from libvips as PNG, which is why the `image`
/// crate is compiled with only the PNG decoder.
pub fn decode_to_texture(
    ctx: &egui::Context,
    name: &str,
    png: &[u8],
) -> Result<egui::TextureHandle, String> {
    let decoded = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map_err(|e| format!("could not decode the preview image: {e}"))?;
    let rgba = decoded.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let colour_image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());

    Ok(ctx.load_texture(
        name,
        colour_image,
        // Linear filtering: previews are scaled to fit, and nearest-neighbour
        // would add its own artefacts on top of the encoder's.
        egui::TextureOptions::LINEAR,
    ))
}
