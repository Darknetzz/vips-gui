//! Top-level application state and the `eframe::App` implementation.

use std::path::PathBuf;

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::job::{JobQueue, JobStatus};
use crate::settings::{ConversionSettings, PerformanceSettings};
use crate::ui;
use crate::ui::preview_panel::PreviewTextures;
use crate::vips::preview::{Debouncer, Preview, PreviewEngine, PreviewError};
use crate::vips::{self, DiscoveryError, VipsInstall};
use crate::worker::{BatchRunner, Converter, VipsConverter};

/// How long to wait after a change before re-rendering the preview.
///
/// Long enough that dragging a slider does not queue an encode per frame, short
/// enough that releasing it feels immediate.
const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(300);

/// Whether we have a working vips yet. Everything else in the UI depends on
/// this, so it is modelled as an explicit two-state machine rather than an
/// `Option` plus a scattering of checks.
enum VipsState {
    Ready(Box<VipsInstall>),
    Unavailable(Box<DiscoveryError>),
}

pub struct App {
    vips: VipsState,
    /// Persisted path to a manually chosen vips.exe, if the user set one.
    vips_override: Option<PathBuf>,
    /// Conversion choices.
    settings: ConversionSettings,
    /// The files to convert.
    queue: JobQueue,
    /// A transient message shown under the toolbar, e.g. "added 12 files".
    notice: Option<String>,
    /// How much of the machine to use.
    performance: PerformanceSettings,
    /// The batch currently running, if any.
    runner: Option<BatchRunner>,
    /// Preview rendering, on its own thread.
    preview: Option<PreviewEngine>,
    /// Collapses slider drags into one preview render.
    preview_debounce: Debouncer,
    /// What the visible preview corresponds to, so needless renders are skipped.
    preview_key: Option<PreviewKey>,
    /// The generation we are waiting on, if any.
    awaiting_preview: Option<u64>,
    /// The finished preview and its textures.
    preview_state: Option<LoadedPreview>,
    /// The most recent preview failure.
    preview_error: Option<(String, PreviewError)>,
}

/// Identifies the inputs a preview was produced from.
///
/// Comparing this before requesting avoids re-encoding when nothing that
/// affects the output has changed, for instance when the user resizes the
/// window or toggles a performance slider.
#[derive(Debug, Clone, PartialEq)]
struct PreviewKey {
    input: PathBuf,
    settings: ConversionSettings,
}

/// A rendered preview with its uploaded textures.
struct LoadedPreview {
    file_name: String,
    preview: Preview,
    textures: PreviewTextures,
}

impl App {
    /// Build the app, running discovery once at startup.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Reload the manual override from the previous session before
        // discovery, so a user who located vips by hand is not asked again.
        let vips_override: Option<PathBuf> = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, Self::OVERRIDE_KEY))
            .unwrap_or(None);

        let settings: ConversionSettings = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, Self::SETTINGS_KEY))
            .unwrap_or_default();

        // Clamp the persisted values: they may have come from a machine with a
        // very different core count.
        let performance: PerformanceSettings = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, Self::PERFORMANCE_KEY))
            .unwrap_or_default();
        let performance = performance.sanitised();

        let mut app = Self {
            vips: VipsState::Unavailable(Box::new(DiscoveryError::NotFound)),
            vips_override,
            settings,
            queue: JobQueue::default(),
            notice: None,
            performance,
            runner: None,
            preview: None,
            preview_debounce: Debouncer::new(PREVIEW_DEBOUNCE),
            preview_key: None,
            awaiting_preview: None,
            preview_state: None,
            preview_error: None,
        };
        app.run_discovery();
        app.start_preview_engine(&cc.egui_ctx);
        app
    }

    /// Spin up the preview thread once vips is known to work.
    ///
    /// Probing stdout support here means it happens once at startup rather than
    /// on every preview.
    fn start_preview_engine(&mut self, ctx: &egui::Context) {
        let VipsState::Ready(install) = &self.vips else {
            self.preview = None;
            return;
        };

        let use_stdout = vips::preview::probe_stdout_support(install);
        self.preview = Some(PreviewEngine::new(
            (**install).clone(),
            use_stdout,
            Some(ctx.clone()),
        ));
    }

    /// Throw away any preview state, for when the selection or vips changes.
    fn reset_preview(&mut self) {
        self.preview_key = None;
        self.awaiting_preview = None;
        self.preview_state = None;
        self.preview_error = None;
        self.preview_debounce.clear();
    }

    /// Storage key for the manual override path.
    const OVERRIDE_KEY: &'static str = "vips_override";
    /// Storage key for the conversion settings.
    const SETTINGS_KEY: &'static str = "settings";
    /// Storage key for the parallelism settings.
    const PERFORMANCE_KEY: &'static str = "performance";

    /// Attempt discovery and record the outcome.
    fn run_discovery(&mut self) {
        self.vips = match vips::discovery::discover(self.vips_override.as_deref()) {
            Ok(install) => VipsState::Ready(Box::new(install)),
            Err(err) => VipsState::Unavailable(Box::new(err)),
        };
        // The old engine is bound to the previous executable, so retire it.
        self.preview = None;
        self.reset_preview();

        // Persisted settings may name a format this libvips cannot write.
        if let VipsState::Ready(install) = &self.vips {
            let capabilities = install.capabilities;
            if let Some(abandoned) = self
                .settings
                .ensure_format_supported(|f| ui::settings_panel::has_saver(capabilities, f))
            {
                self.notice = Some(format!(
                    "This libvips build cannot write {} (no {}), so {} is selected instead.",
                    abandoned.label(),
                    abandoned.saver_name(),
                    self.settings.format.label()
                ));
            }
        }
    }

    /// Adopt a user-picked executable and validate it immediately.
    fn set_override(&mut self, exe: PathBuf) {
        self.vips_override = Some(exe);
        self.run_discovery();
        // If the pick turned out to be unusable, drop it so the next Retry
        // falls back to normal discovery instead of failing the same way.
        if matches!(self.vips, VipsState::Unavailable(_)) {
            self.vips_override = None;
        }
    }

    /// Clear any override and search again from scratch.
    fn retry(&mut self) {
        self.vips_override = None;
        self.run_discovery();
    }

    /// The main window, shown once vips is available.
    fn show_main(&mut self, ui: &mut egui::Ui, install: &VipsInstall) {
        // Files dropped anywhere on the window land in the queue.
        self.handle_dropped_files(ui.ctx());

        // Take in whatever the workers reported since the last frame.
        self.poll_runner(ui.ctx());

        let tally = self.queue.tally();
        let busy = self.runner.is_some();

        self.handle_shortcuts(ui.ctx(), busy);

        // Toolbar and destination row across the top.
        // egui 0.36 unified SidePanel and TopBottomPanel into `Panel`, with
        // side-agnostic sizing methods.
        egui::Panel::top("toolbar")
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 8)))
            .show(ui, |ui| {
                let blocked = self.convert_blocked_reason(install);
                let toolbar_action =
                    ui::toolbar::show(ui, &mut self.settings, tally, busy, blocked.as_deref());
                self.apply_toolbar_action(toolbar_action, install, ui.ctx());

                // A transient status line, so actions have visible consequences.
                if let Some(notice) = &self.notice {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(notice).weak().size(12.0));
                }
            });

        // Summary strip along the bottom.
        egui::Panel::bottom("summary")
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6)))
            .show(ui, |ui| {
                self.show_summary(ui, install, tally);
            });

        // Settings down the right, where it stays out of the way of the list
        // but is always visible while choosing files.
        egui::Panel::right("settings")
            .resizable(true)
            .default_size(300.0)
            .size_range(260.0..=420.0)
            .frame(egui::Frame::new().inner_margin(egui::Margin::same(8)))
            .show(ui, |ui| {
                // The return value is ignored: `drive_preview` detects changes
                // by comparing the settings a preview was built from, which
                // also catches selection changes in one place.
                let _ = ui::settings_panel::show(
                    ui,
                    &mut self.settings,
                    &mut self.performance,
                    install.capabilities,
                    busy,
                );
            });

        // Preview to the left of the settings, so the image sits next to the
        // controls that change it.
        egui::Panel::right("preview")
            .resizable(true)
            .default_size(320.0)
            .size_range(240.0..=560.0)
            .frame(egui::Frame::new().inner_margin(egui::Margin::same(8)))
            .show(ui, |ui| {
                self.show_preview(ui);
            });

        // The queue fills whatever is left.
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin::same(4)))
            .show(ui, |ui| {
                let list_action = ui::file_list::show(ui, &self.queue, busy);
                self.apply_list_action(list_action, ui.ctx());
            });

        // Decide about previews after the list has had a chance to change the
        // selection, so a click is reflected without a frame of lag.
        self.drive_preview(ui.ctx());
    }

    /// Keyboard navigation for the queue.
    ///
    /// Arrow keys move the selection and Delete removes a row, so the list is
    /// usable without a mouse. Shortcuts are ignored while a text field has
    /// focus, otherwise typing a width would jump the selection about.
    fn handle_shortcuts(&mut self, ctx: &egui::Context, busy: bool) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }

        let (down, up, delete) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Delete),
            )
        });

        if down {
            self.queue.select_next();
        }
        if up {
            self.queue.select_previous();
        }
        // Removing a row mid-batch would desynchronise the worker indices.
        if delete && !busy && let Some(selected) = self.queue.selected {
            self.queue.remove(selected);
        }
    }

    /// Collect files the user dragged onto the window.
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        // `dropped_files` is populated for one frame after the drop completes.
        // In egui 0.36 each entry is a trait object owned by the windowing
        // integration; `path()` is an absolute path on native platforms.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });

        if dropped.is_empty() {
            return;
        }

        let added = self.queue.add_paths(&dropped);
        self.notice = Some(describe_addition(added, dropped.len()));
    }

    /// Show a drop hint while a drag is in progress over the window.
    fn show_drop_hint(&self, ctx: &egui::Context) {
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if !hovering {
            return;
        }

        // A translucent overlay makes it obvious the window will accept the
        // drop, which is otherwise invisible feedback.
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop_overlay"),
        ));
        let screen = ctx.viewport_rect();
        painter.rect_filled(
            screen,
            0.0,
            egui::Color32::from_black_alpha(160),
        );
        painter.text(
            screen.center(),
            egui::Align2::CENTER_CENTER,
            "Drop to add images",
            egui::FontId::proportional(24.0),
            egui::Color32::WHITE,
        );
    }

    /// Why Convert is unavailable, if it is.
    fn convert_blocked_reason(&self, install: &VipsInstall) -> Option<String> {
        let format = self.settings.format;
        let supported = match format {
            crate::settings::OutputFormat::Jpeg => install.capabilities.jpeg,
            crate::settings::OutputFormat::Png => install.capabilities.png,
            crate::settings::OutputFormat::Webp => install.capabilities.webp,
            crate::settings::OutputFormat::Avif => install.capabilities.avif,
        };

        if !supported {
            return Some(format!(
                "This libvips build cannot write {}: it has no {}.",
                format.label(),
                format.saver_name()
            ));
        }
        None
    }

    fn apply_toolbar_action(
        &mut self,
        action: ui::toolbar::Action,
        install: &VipsInstall,
        ctx: &egui::Context,
    ) {
        match action {
            ui::toolbar::Action::None => {}
            ui::toolbar::Action::AddPaths(paths) => {
                let added = self.queue.add_paths(&paths);
                self.notice = Some(describe_addition(added, paths.len()));
            }
            ui::toolbar::Action::Clear => {
                self.queue.clear();
                self.notice = None;
            }
            ui::toolbar::Action::ClearCompleted => {
                self.queue.clear_completed();
                self.notice = None;
            }
            ui::toolbar::Action::StartConversion => self.start_conversion(install, ctx),
            ui::toolbar::Action::Cancel => {
                if let Some(runner) = &self.runner {
                    runner.cancel();
                    self.notice = Some("Cancelling...".to_owned());
                }
            }
        }
    }

    fn apply_list_action(&mut self, action: ui::file_list::Action, ctx: &egui::Context) {
        match action {
            ui::file_list::Action::None => {}
            ui::file_list::Action::Select(index) => self.queue.selected = Some(index),
            ui::file_list::Action::Remove(index) => self.queue.remove(index),
            ui::file_list::Action::CopyError(text) => {
                ctx.copy_text(text);
                self.notice = Some("Error copied to the clipboard.".to_owned());
            }
        }
    }

    /// Hand every queued job to a fresh worker pool.
    fn start_conversion(&mut self, install: &VipsInstall, ctx: &egui::Context) {
        if self.runner.is_some() {
            return;
        }

        let jobs: Vec<(usize, std::path::PathBuf)> = self
            .queue
            .jobs()
            .iter()
            .enumerate()
            .filter(|(_, job)| job.status == JobStatus::Queued)
            .map(|(index, job)| (index, job.input.clone()))
            .collect();

        if jobs.is_empty() {
            self.notice = Some("Nothing to convert.".to_owned());
            return;
        }

        let converter: Arc<dyn Converter> = Arc::new(VipsConverter::new(
            install.clone(),
            self.performance.vips_concurrency,
        ));

        self.notice = None;
        self.runner = Some(BatchRunner::start(
            converter,
            self.settings.clone(),
            jobs,
            self.performance.workers,
            // Give the workers a handle so progress wakes the UI even when the
            // user is not touching the mouse.
            Some(ctx.clone()),
        ));
    }

    /// Apply any progress that arrived, and tidy up when the batch ends.
    fn poll_runner(&mut self, ctx: &egui::Context) {
        let Some(runner) = &self.runner else {
            return;
        };

        for update in runner.drain() {
            if let Some(job) = self.queue.jobs_mut().get_mut(update.index) {
                job.status = update.status;
            }
        }

        if !runner.is_finished() {
            // Keep animating the spinners and progress bar.
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }

        // Finished: take the runner, drain the last updates, and summarise.
        let runner = self.runner.take().expect("checked above");
        let cancelled = runner.is_cancelled();
        let elapsed = runner.elapsed();
        let bytes = runner.bytes_written();

        for update in runner.join() {
            if let Some(job) = self.queue.jobs_mut().get_mut(update.index) {
                job.status = update.status;
            }
        }

        let tally = self.queue.tally();
        self.notice = Some(if cancelled {
            format!(
                "Cancelled after {:.1}s. {} converted, {} skipped, {} failed, {} still queued.",
                elapsed.as_secs_f32(),
                tally.done,
                tally.skipped,
                tally.failed,
                tally.queued
            )
        } else {
            format!(
                "Converted {} file{} in {:.1}s ({} written){}{}.",
                tally.done,
                if tally.done == 1 { "" } else { "s" },
                elapsed.as_secs_f32(),
                format_bytes(bytes),
                if tally.skipped > 0 {
                    format!(", {} skipped", tally.skipped)
                } else {
                    String::new()
                },
                if tally.failed > 0 {
                    format!(", {} failed", tally.failed)
                } else {
                    String::new()
                }
            )
        });
    }

    /// Decide whether to ask for a new preview, and take in finished ones.
    fn drive_preview(&mut self, ctx: &egui::Context) {
        // Collect finished renders first, so a result that just arrived is
        // displayed this frame.
        self.collect_preview_outcomes(ctx);

        let Some(job) = self.queue.selected_job() else {
            // Nothing selected: drop whatever was on screen.
            if self.preview_key.is_some() || self.preview_state.is_some() {
                self.reset_preview();
            }
            return;
        };

        let wanted = PreviewKey {
            input: job.input.clone(),
            settings: self.settings.clone(),
        };

        // Only restart the timer when the inputs genuinely differ. Without this
        // check the preview would re-render on every frame.
        if self.preview_key.as_ref() != Some(&wanted) {
            self.preview_key = Some(wanted.clone());
            self.preview_debounce.touch(Instant::now());
            // Keep showing the previous image while the new one renders rather
            // than flashing to an empty panel.
            self.preview_error = None;
        }

        // While a batch is running, the machine is busy and the user is not
        // fiddling with settings. Holding off avoids competing for cores.
        if self.runner.is_some() {
            return;
        }

        let now = Instant::now();
        if self.preview_debounce.take_if_ready(now) {
            if let Some(engine) = &mut self.preview {
                let generation = engine.request(wanted.input, wanted.settings);
                self.awaiting_preview = Some(generation);
            }
        } else if let Some(remaining) = self.preview_debounce.time_remaining(now) {
            // Make sure a frame happens when the debounce expires, even if the
            // user stops moving the mouse.
            ctx.request_repaint_after(remaining);
        }
    }

    /// Take finished previews and upload their textures.
    fn collect_preview_outcomes(&mut self, ctx: &egui::Context) {
        let Some(engine) = &self.preview else {
            return;
        };

        for outcome in engine.drain() {
            // Ignore anything superseded while it was in flight.
            if self.awaiting_preview != Some(outcome.generation) {
                continue;
            }
            self.awaiting_preview = None;

            let file_name = outcome
                .input
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| outcome.input.display().to_string());

            match outcome.result {
                Ok(preview) => {
                    // Texture names include the generation so egui replaces the
                    // old texture rather than reusing a stale one.
                    let source = ui::preview_panel::decode_to_texture(
                        ctx,
                        &format!("preview-source-{}", outcome.generation),
                        &preview.source.png,
                    );
                    let result = ui::preview_panel::decode_to_texture(
                        ctx,
                        &format!("preview-result-{}", outcome.generation),
                        &preview.result.png,
                    );

                    match (source, result) {
                        (Ok(source), Ok(result)) => {
                            self.preview_error = None;
                            self.preview_state = Some(LoadedPreview {
                                file_name,
                                preview,
                                textures: PreviewTextures { source, result },
                            });
                        }
                        (Err(message), _) | (_, Err(message)) => {
                            self.preview_state = None;
                            self.preview_error = Some((
                                file_name,
                                PreviewError::VipsFailed { message },
                            ));
                        }
                    }
                }
                Err(error) => {
                    self.preview_state = None;
                    self.preview_error = Some((file_name, error));
                }
            }
        }
    }

    /// Render the preview panel from whatever state we have.
    fn show_preview(&self, ui: &mut egui::Ui) {
        use ui::preview_panel::PreviewState;

        let selected_name = self
            .queue
            .selected_job()
            .map(crate::job::Job::display_name);

        let state = match (&selected_name, &self.preview_error, &self.preview_state) {
            (None, _, _) => PreviewState::NoSelection,
            (Some(name), Some((error_name, error)), _) if error_name == name => {
                PreviewState::Failed {
                    file_name: name.clone(),
                    error,
                }
            }
            (Some(name), _, Some(loaded)) if &loaded.file_name == name => PreviewState::Ready {
                file_name: name.clone(),
                preview: &loaded.preview,
                textures: &loaded.textures,
            },
            (Some(name), _, _) => PreviewState::Rendering {
                file_name: name.clone(),
            },
        };

        ui::preview_panel::show(ui, state);
    }

    /// The bottom strip: progress and the vips build in use.
    fn show_summary(&self, ui: &mut egui::Ui, install: &VipsInstall, tally: crate::job::Tally) {
        // A progress bar only while a batch is running, so the strip stays
        // quiet the rest of the time.
        if let Some(runner) = &self.runner {
            let fraction = tally.fraction();
            let text = if runner.is_cancelled() {
                "Cancelling...".to_owned()
            } else {
                format!(
                    "{} of {} · {} running",
                    tally.finished(),
                    tally.total(),
                    tally.running
                )
            };
            ui.add(
                egui::ProgressBar::new(fraction)
                    .text(text)
                    .animate(!runner.is_cancelled()),
            );
            ui.add_space(4.0);
        }

        ui.horizontal(|ui| {
            if tally.total() == 0 {
                ui.label(egui::RichText::new("No files queued").weak().size(12.0));
            } else {
                ui.label(
                    egui::RichText::new(format!(
                        "{} file{} · {} done · {} skipped · {} failed",
                        tally.total(),
                        if tally.total() == 1 { "" } else { "s" },
                        tally.done,
                        tally.skipped,
                        tally.failed
                    ))
                    .size(12.0),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "libvips {} · → {}",
                        install.version,
                        self.settings.format.label()
                    ))
                    .weak()
                    .size(12.0),
                )
                .on_hover_text(format!(
                    "{}\n{}",
                    install.exe.display(),
                    install.source.describe()
                ));
            });
        });
    }
}

/// Wording for "added N files", including the nothing-happened case.
fn describe_addition(added: usize, dropped: usize) -> String {
    match (added, dropped) {
        (0, 1) => "Nothing added: that is not an image we can read.".to_owned(),
        (0, _) => "Nothing added: no new images found.".to_owned(),
        (1, _) => "Added 1 file.".to_owned(),
        (n, _) => format!("Added {n} files."),
    }
}

/// Render a byte count compactly for the UI.
pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    let b = bytes as f64;
    if b >= MIB {
        format!("{:.1} MB", b / MIB)
    } else if b >= KIB {
        format!("{:.0} kB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

impl eframe::App for App {
    // eframe 0.36 hands the app a `Ui` for the root viewport rather than a
    // `Context` plus a panel of our own making.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        match &self.vips {
            VipsState::Ready(install) => {
                // Clone the handle so the immutable borrow of `self.vips` ends
                // before any panel gets the chance to mutate `self`.
                let install = install.clone();
                // Discovery may have replaced the engine (Retry, or a manually
                // located executable), so make sure one exists.
                if self.preview.is_none() {
                    self.start_preview_engine(ui.ctx());
                }
                self.show_main(ui, &install);
                self.show_drop_hint(ui.ctx());
            }
            VipsState::Unavailable(err) => {
                // Same reasoning: `show` may return an action that mutates self.
                let err = err.clone();
                match ui::missing_vips::show(ui, &err) {
                    ui::missing_vips::Action::None => {}
                    ui::missing_vips::Action::Retry => self.retry(),
                    ui::missing_vips::Action::UseExecutable(path) => self.set_override(path),
                }
            }
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, Self::OVERRIDE_KEY, &self.vips_override);
        eframe::set_value(storage, Self::SETTINGS_KEY, &self.settings);
        eframe::set_value(storage, Self::PERFORMANCE_KEY, &self.performance);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_formatting_picks_readable_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1 kB");
        assert_eq!(format_bytes(2048), "2 kB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 / 2), "1.5 MB");
    }

    #[test]
    fn addition_wording_covers_the_nothing_happened_cases() {
        // Dropping one non-image should say why, not just "added 0".
        assert!(describe_addition(0, 1).contains("not an image"));
        assert!(describe_addition(0, 5).contains("no new images"));
        assert_eq!(describe_addition(1, 1), "Added 1 file.");
        assert_eq!(describe_addition(4, 9), "Added 4 files.");
    }
}
