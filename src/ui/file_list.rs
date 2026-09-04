//! The queue table: one row per file, with status and a remove button.

use crate::app::format_bytes;
use crate::job::{JobQueue, JobStatus};

/// What the user did to the list this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    /// Select this row (for the preview panel).
    Select(usize),
    /// Remove this row.
    Remove(usize),
    /// Put this error message on the clipboard.
    CopyError(String),
}

/// Draw the list. `busy` disables the destructive controls mid-conversion.
pub fn show(ui: &mut egui::Ui, queue: &JobQueue, busy: bool) -> Action {
    let mut action = Action::None;

    if queue.is_empty() {
        show_empty_state(ui);
        return action;
    }

    // A caption naming the region, so the list is not an unlabelled table.
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{} file{} queued",
                queue.len(),
                if queue.len() == 1 { "" } else { "s" }
            ))
            .strong()
            .size(12.0),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new("↑ ↓ to select · Delete to remove")
                    .weak()
                    .size(11.0),
            );
        });
    });
    ui.add_space(4.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, job) in queue.jobs().iter().enumerate() {
                let selected = queue.selected == Some(index);

                // One clickable row. A frame gives the selection something
                // visible to fill without needing a custom widget.
                let response = ui
                    .push_id(index, |ui| {
                        egui::Frame::new()
                            .fill(if selected {
                                ui.visuals().selection.bg_fill
                            } else {
                                egui::Color32::TRANSPARENT
                            })
                            .inner_margin(egui::Margin::symmetric(6, 4))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    // Status glyph plus text, so the state is
                                    // legible without relying on colour.
                                    ui.label(
                                        egui::RichText::new(job.status.icon())
                                            .color(status_colour(ui, &job.status))
                                            .monospace(),
                                    )
                                    .on_hover_text(job.status.label());

                                    // File name takes the slack, so the
                                    // controls on the right stay put.
                                    ui.add(
                                        egui::Label::new(job.display_name())
                                            .truncate()
                                            .selectable(false),
                                    )
                                    .on_hover_text(job.input.display().to_string());

                                    // Right-aligned metadata and controls.
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            // The glyph keeps the row compact,
                                            // but an icon alone is meaningless
                                            // to a screen reader, so the
                                            // accessible name spells it out.
                                            let remove = ui
                                                .add_enabled(
                                                    !busy,
                                                    egui::Button::new("✕").frame(false),
                                                )
                                                .on_hover_text(format!(
                                                    "Remove {} from the list",
                                                    job.display_name()
                                                ));
                                            remove.widget_info(|| {
                                                egui::WidgetInfo::labeled(
                                                    egui::WidgetType::Button,
                                                    !busy,
                                                    &format!("Remove {}", job.display_name()),
                                                )
                                            });
                                            if remove.clicked() {
                                                action = Action::Remove(index);
                                            }

                                            show_row_detail(ui, job, &mut action);
                                        },
                                    );
                                });
                            })
                    })
                    .response;

                // Make the whole row a selection target, not just the label.
                let row = response.interact(egui::Sense::click());
                if row.clicked() {
                    action = Action::Select(index);
                }
                // Describe the row as a selectable item with its status, so
                // assistive technology reads something meaningful rather than
                // an anonymous clickable region.
                row.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::SelectableLabel,
                        true,
                        selected,
                        format!("{}, {}", job.display_name(), job.status.label()),
                    )
                });
                if !selected && row.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
        });

    action
}

/// The per-row right-hand detail: sizes, or the error and its copy button.
fn show_row_detail(ui: &mut egui::Ui, job: &crate::job::Job, action: &mut Action) {
    match &job.status {
        JobStatus::Done { bytes, output } => {
            // Show the saving as a percentage, which is the thing people
            // actually want to know after a conversion.
            let detail = match job.input_bytes {
                Some(input_bytes) if input_bytes > 0 => {
                    let ratio = *bytes as f64 / input_bytes as f64;
                    let change = (1.0 - ratio) * 100.0;
                    if change >= 0.0 {
                        format!("{} (−{change:.0}%)", format_bytes(*bytes))
                    } else {
                        format!("{} (+{:.0}%)", format_bytes(*bytes), -change)
                    }
                }
                _ => format_bytes(*bytes),
            };
            ui.label(egui::RichText::new(detail).weak())
                .on_hover_text(output.display().to_string());
        }
        JobStatus::Skipped { output } => {
            ui.label(egui::RichText::new("already exists").weak())
                .on_hover_text(format!("{} was left untouched", output.display()));
        }
        JobStatus::Failed { message, command } => {
            // A copy button, because vips messages are worth pasting into a
            // search box or a bug report.
            let mut clipboard_text = message.clone();
            if let Some(command) = command {
                clipboard_text = format!("{message}\n\nCommand:\n{command}");
            }
            if ui
                .add(egui::Button::new("Copy error").frame(false))
                .on_hover_text("Copy the message and the failing command")
                .clicked()
            {
                *action = Action::CopyError(clipboard_text);
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(message)
                        .color(ui.visuals().error_fg_color)
                        .size(12.0),
                )
                .truncate(),
            )
            .on_hover_text(message);
        }
        JobStatus::Running => {
            ui.add(egui::Spinner::new().size(12.0));
        }
        JobStatus::Queued => {
            if let Some(bytes) = job.input_bytes {
                ui.label(egui::RichText::new(format_bytes(bytes)).weak());
            }
        }
    }
}

/// Colour for a status glyph. Always paired with text, never the only signal.
fn status_colour(ui: &egui::Ui, status: &JobStatus) -> egui::Color32 {
    match status {
        JobStatus::Queued => ui.visuals().weak_text_color(),
        JobStatus::Running => ui.visuals().strong_text_color(),
        JobStatus::Done { .. } => egui::Color32::from_rgb(0x4c, 0xaf, 0x50),
        JobStatus::Skipped { .. } => ui.visuals().weak_text_color(),
        JobStatus::Failed { .. } => ui.visuals().error_fg_color,
    }
}

/// The invitation shown when nothing is queued.
fn show_empty_state(ui: &mut egui::Ui) {
    let available = ui.available_size();
    ui.allocate_ui_with_layout(
        available,
        egui::Layout::centered_and_justified(egui::Layout::default().main_dir),
        |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(available.y / 2.0 - 40.0);
                ui.label(
                    egui::RichText::new("Drop images or folders here")
                        .size(16.0)
                        .weak(),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("or use Add files / Add folder above")
                        .size(12.0)
                        .weak(),
                );
            });
        },
    );
}
