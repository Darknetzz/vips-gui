//! The top toolbar: adding files, the output destination, and Convert.

use std::path::PathBuf;

use crate::job::{INPUT_EXTENSIONS, Tally};
use crate::settings::{ConversionSettings, OutputDestination};

/// What the user asked for this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    AddPaths(Vec<PathBuf>),
    Clear,
    ClearCompleted,
    StartConversion,
    Cancel,
}

/// Draw the toolbar.
///
/// `busy` is true while a conversion is running, which disables anything that
/// would change the queue underneath it.
pub fn show(
    ui: &mut egui::Ui,
    settings: &mut ConversionSettings,
    tally: Tally,
    busy: bool,
    convert_blocked_reason: Option<&str>,
) -> Action {
    let mut action = Action::None;

    ui.horizontal(|ui| {
        if ui
            .add_enabled(!busy, egui::Button::new("Add files"))
            .on_hover_text("Choose images to convert")
            .clicked()
            && let Some(files) = rfd::FileDialog::new()
                .set_title("Add images")
                .add_filter("Images", INPUT_EXTENSIONS)
                .pick_files()
        {
            action = Action::AddPaths(files);
        }

        if ui
            .add_enabled(!busy, egui::Button::new("Add folder"))
            .on_hover_text("Add every image in a folder, including subfolders")
            .clicked()
            && let Some(folder) = rfd::FileDialog::new()
                .set_title("Add a folder of images")
                .pick_folder()
        {
            action = Action::AddPaths(vec![folder]);
        }

        ui.separator();

        let has_finished = tally.finished() > 0;
        if ui
            .add_enabled(!busy && has_finished, egui::Button::new("Clear finished"))
            .on_hover_text("Remove converted and skipped files, keeping failures")
            .clicked()
        {
            action = Action::ClearCompleted;
        }

        if ui
            .add_enabled(!busy && tally.total() > 0, egui::Button::new("Clear all"))
            .on_hover_text("Empty the list")
            .clicked()
        {
            action = Action::Clear;
        }

        // Convert (or Cancel) sits on the right, where the eye finishes.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if busy {
                // While running, the same slot becomes Cancel, so the primary
                // action is always in the same place.
                let cancel = egui::Button::new(egui::RichText::new("Cancel").strong())
                    .min_size(egui::vec2(150.0, 28.0));
                if ui
                    .add(cancel)
                    .on_hover_text("Stop after the images already being converted")
                    .clicked()
                {
                    action = Action::Cancel;
                }
                return;
            }

            let can_convert = tally.queued > 0 && convert_blocked_reason.is_none();

            let button = egui::Button::new(
                egui::RichText::new(format!("Convert {}", describe_pending(tally))).strong(),
            )
            .min_size(egui::vec2(150.0, 28.0));

            let response = ui.add_enabled(can_convert, button);

            // Explain the disabled state rather than leaving a dead button.
            let response = if let Some(reason) = convert_blocked_reason {
                response.on_disabled_hover_text(reason)
            } else if tally.total() == 0 {
                response.on_disabled_hover_text("Add some images first")
            } else {
                response.on_disabled_hover_text("Nothing left to convert")
            };

            if response.clicked() {
                action = Action::StartConversion;
            }
        });
    });

    ui.add_space(6.0);

    // Output destination on its own row: it is a per-batch decision and
    // deserves more room than a toolbar slot.
    ui.horizontal(|ui| {
        ui.label("Save to:");

        let mut same_as_source = matches!(settings.destination, OutputDestination::SameAsSource);
        if ui
            .add_enabled(
                !busy,
                egui::RadioButton::new(same_as_source, "Alongside each original"),
            )
            .on_hover_text("Each converted file is written next to its source")
            .clicked()
        {
            same_as_source = true;
            settings.destination = OutputDestination::SameAsSource;
        }

        let existing_dir = match &settings.destination {
            OutputDestination::Directory(dir) => Some(dir.clone()),
            OutputDestination::SameAsSource => None,
        };

        if ui
            .add_enabled(
                !busy,
                egui::RadioButton::new(!same_as_source, "This folder:"),
            )
            .on_hover_text("Write every converted file into one folder")
            .clicked()
        {
            // Choosing this option with no folder yet should ask for one,
            // rather than leaving the app in a state it cannot act on.
            match existing_dir {
                Some(dir) => settings.destination = OutputDestination::Directory(dir),
                None => {
                    if let Some(picked) = rfd::FileDialog::new()
                        .set_title("Choose an output folder")
                        .pick_folder()
                    {
                        settings.destination = OutputDestination::Directory(picked);
                    }
                }
            }
        }

        let label = match &settings.destination {
            OutputDestination::Directory(dir) => shorten_path(dir, 48),
            OutputDestination::SameAsSource => "not set".to_owned(),
        };

        let browse = ui
            .add_enabled(!busy, egui::Button::new(label))
            .on_hover_text(match &settings.destination {
                OutputDestination::Directory(dir) => dir.display().to_string(),
                OutputDestination::SameAsSource => "Click to choose an output folder".to_owned(),
            });

        if browse.clicked()
            && let Some(picked) = rfd::FileDialog::new()
                .set_title("Choose an output folder")
                .pick_folder()
        {
            settings.destination = OutputDestination::Directory(picked);
        }
    });

    action
}

/// "3 files" / "1 file", or nothing when the queue is empty.
fn describe_pending(tally: Tally) -> String {
    match tally.queued {
        0 => "all".to_owned(),
        1 => "1 file".to_owned(),
        n => format!("{n} files"),
    }
}

/// Trim a long path from the left so the file name stays visible.
pub fn shorten_path(path: &std::path::Path, max_chars: usize) -> String {
    let text = path.display().to_string();
    let count = text.chars().count();
    if count <= max_chars {
        return text;
    }
    let tail: String = text
        .chars()
        .skip(count.saturating_sub(max_chars.saturating_sub(1)))
        .collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn pending_description_is_grammatical() {
        let mut t = Tally::default();
        t.queued = 0;
        assert_eq!(describe_pending(t), "all");
        t.queued = 1;
        assert_eq!(describe_pending(t), "1 file");
        t.queued = 7;
        assert_eq!(describe_pending(t), "7 files");
    }

    #[test]
    fn short_paths_are_left_alone() {
        let p = Path::new("C:\\out");
        assert_eq!(shorten_path(p, 48), "C:\\out");
    }

    #[test]
    fn long_paths_are_trimmed_from_the_left_keeping_the_tail() {
        let p = Path::new("C:\\Users\\Someone\\Pictures\\2026\\Holiday\\Processed\\Output");
        let shortened = shorten_path(p, 20);
        assert!(shortened.chars().count() <= 20, "got {shortened}");
        assert!(shortened.starts_with('…'));
        assert!(
            shortened.ends_with("Output"),
            "the tail is the useful part: {shortened}"
        );
    }

    #[test]
    fn shortening_handles_a_max_of_one() {
        let p = Path::new("C:\\a\\b\\c");
        let shortened = shorten_path(p, 1);
        assert_eq!(shortened, "…");
    }
}
