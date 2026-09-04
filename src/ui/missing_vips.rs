//! The blocking screen shown when we cannot find a usable `vips`.
//!
//! This is the first thing a new user is likely to see, so it explains the
//! install steps inline rather than only linking out, and offers a direct
//! "point me at the exe" escape hatch for non-standard installs.

use crate::vips::{DOWNLOAD_URL, DiscoveryError, MIN_VERSION};

/// What the user asked us to do on this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing this frame.
    None,
    /// Re-run discovery (e.g. after installing libvips in another window).
    Retry,
    /// Use this executable from now on.
    UseExecutable(std::path::PathBuf),
}

/// Draw the screen. Returns the action the user triggered, if any.
pub fn show(ui: &mut egui::Ui, error: &DiscoveryError) -> Action {
    let mut action = Action::None;

    ui.vertical_centered(|ui| {
        ui.add_space(48.0);

        ui.heading("libvips was not found");
        ui.add_space(8.0);

        // The specific failure, so the user knows whether to install, update,
        // or fix a bad path.
        ui.label(
            egui::RichText::new(error.to_string())
                .color(ui.visuals().warn_fg_color)
                .size(14.0),
        );

        ui.add_space(20.0);
        ui.separator();
        ui.add_space(20.0);

        // Only show install instructions when installing is actually the fix.
        // For a too-old or misconfigured install they would be noise.
        let needs_install = matches!(
            error,
            DiscoveryError::NotFound | DiscoveryError::OverrideMissing { .. }
        );

        if needs_install {
            ui.label(
                egui::RichText::new("This app converts images with libvips, which it expects to already be installed.")
                    .size(13.0),
            );
            ui.add_space(12.0);

            // Constrain the width so the numbered steps read as a block
            // instead of stretching across a wide window.
            ui.allocate_ui_with_layout(
                egui::vec2(460.0, 0.0),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.label("To install it on Windows:");
                    ui.add_space(6.0);
                    ui.label("1.  Download  vips-dev-w64-web-<version>.zip  from the releases page.");
                    ui.label("2.  Unzip it somewhere permanent, for example  C:\\Program Files.");
                    ui.label("3.  Add the  vips-<version>\\bin  folder to your PATH.");
                    ui.label("4.  Come back here and press Retry.");
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(
                            "Alternatively, skip the PATH step and use \"Locate vips.exe\" below.",
                        )
                        .italics()
                        .size(12.0),
                    );
                },
            );

            ui.add_space(16.0);
        }

        if matches!(error, DiscoveryError::TooOld { .. }) {
            ui.label(format!(
                "Download {MIN_VERSION} or newer from the releases page, then press Retry."
            ));
            ui.add_space(16.0);
        }

        // Buttons. `open::that` uses the OS handler, which avoids depending on
        // egui's hyperlink behaviour and works with the user's default browser.
        ui.horizontal(|ui| {
            // Centre the button row inside the centred layout.
            ui.add_space(ui.available_width() / 2.0 - 210.0);

            if ui
                .add_sized(
                    [200.0, 32.0],
                    egui::Button::new("Download libvips for Windows"),
                )
                .on_hover_text(DOWNLOAD_URL)
                .clicked()
            {
                // A failure here is not worth interrupting the user over; the
                // URL is in the tooltip if the browser does not open.
                let _ = open::that(DOWNLOAD_URL);
            }

            if ui
                .add_sized([120.0, 32.0], egui::Button::new("Locate vips.exe"))
                .on_hover_text("Pick vips.exe yourself if it is installed somewhere unusual.")
                .clicked()
                && let Some(picked) = rfd::FileDialog::new()
                    .set_title("Select vips.exe")
                    .add_filter("Executable", &["exe"])
                    .pick_file()
            {
                action = Action::UseExecutable(picked);
            }

            if ui
                .add_sized([90.0, 32.0], egui::Button::new("Retry"))
                .on_hover_text("Look for libvips again.")
                .clicked()
            {
                action = Action::Retry;
            }
        });

        ui.add_space(24.0);
        ui.label(
            egui::RichText::new(format!("Requires libvips {MIN_VERSION} or newer."))
                .weak()
                .size(11.0),
        );
    });

    action
}
