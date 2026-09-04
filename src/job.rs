//! The queue of files to convert, and the per-file status the UI displays.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// Input extensions we offer to load.
///
/// Deliberately broader than the four output formats: libvips reads far more
/// than it writes here, and converting a TIFF or GIF to WebP is a perfectly
/// reasonable thing to want. Matched case-insensitively.
pub const INPUT_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "webp", "avif", "heic", "heif", "tif", "tiff", "gif",
    "bmp", "jp2", "jxl", "ppm", "pgm", "pbm", "pnm", "exr", "hdr", "svg",
];

/// True when a path looks like an image we can offer to convert.
pub fn is_supported_input(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let lower = e.to_ascii_lowercase();
            INPUT_EXTENSIONS.contains(&lower.as_str())
        })
        .unwrap_or(false)
}

/// Where a single file is in its lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum JobStatus {
    /// Waiting to be picked up.
    #[default]
    Queued,
    /// Currently being converted.
    Running,
    /// Converted successfully.
    Done {
        output: PathBuf,
        /// Size of the written file.
        bytes: u64,
    },
    /// Deliberately not converted, because the output already existed.
    Skipped { output: PathBuf },
    /// Conversion failed. `message` is suitable for showing to a user.
    Failed {
        message: String,
        /// The vips command that failed, for the copy-error action.
        command: Option<String>,
    },
}

impl JobStatus {
    /// A short label. Paired with [`Self::icon`] so status is never conveyed
    /// by colour alone.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Running => "Converting",
            Self::Done { .. } => "Done",
            Self::Skipped { .. } => "Skipped",
            Self::Failed { .. } => "Failed",
        }
    }

    /// A text glyph for the status, readable without colour vision.
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Queued => "·",
            Self::Running => "»",
            Self::Done { .. } => "✔",
            Self::Skipped { .. } => "–",
            Self::Failed { .. } => "✖",
        }
    }

    /// True once this job will not change again without being reset.
    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            Self::Done { .. } | Self::Skipped { .. } | Self::Failed { .. }
        )
    }
}

/// One file to convert.
#[derive(Debug, Clone)]
pub struct Job {
    pub input: PathBuf,
    pub status: JobStatus,
    /// Size of the input, read when the job is added. `None` if unreadable.
    pub input_bytes: Option<u64>,
}

impl Job {
    pub fn new(input: PathBuf) -> Self {
        let input_bytes = std::fs::metadata(&input).ok().map(|m| m.len());
        Self {
            input,
            status: JobStatus::Queued,
            input_bytes,
        }
    }

    /// The file name for display, falling back to the whole path.
    pub fn display_name(&self) -> String {
        self.input
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.input.display().to_string())
    }
}

/// The list of jobs, with duplicate suppression.
#[derive(Debug, Default)]
pub struct JobQueue {
    jobs: Vec<Job>,
    /// Normalised paths already present, so re-dropping a folder is cheap and
    /// does not create duplicate rows.
    seen: HashSet<PathBuf>,
    /// Index of the row the user has selected, for the preview panel.
    pub selected: Option<usize>,
}

impl JobQueue {
    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub fn jobs_mut(&mut self) -> &mut [Job] {
        &mut self.jobs
    }

    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// The currently selected job, if any.
    pub fn selected_job(&self) -> Option<&Job> {
        self.selected.and_then(|i| self.jobs.get(i))
    }

    /// Add many paths, expanding directories and skipping non-images and
    /// duplicates. Returns how many jobs were actually added.
    pub fn add_paths<I, P>(&mut self, paths: I) -> usize
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut added = 0;
        for path in paths {
            added += self.add_path(path.as_ref());
        }
        // Give the user something selected so the preview is not blank after a
        // drop, but never override an existing choice.
        if self.selected.is_none() && !self.jobs.is_empty() {
            self.selected = Some(0);
        }
        added
    }

    /// Add one path. Directories are walked recursively.
    fn add_path(&mut self, path: &Path) -> usize {
        if path.is_dir() {
            let mut added = 0;
            // `WalkDir` handles the recursion; `flatten` drops entries we
            // cannot read (permissions, vanished files) rather than aborting
            // the whole drop.
            for entry in WalkDir::new(path).follow_links(false).into_iter().flatten() {
                if entry.file_type().is_file() && is_supported_input(entry.path()) {
                    added += usize::from(self.push_unique(entry.path()));
                }
            }
            added
        } else if path.is_file() && is_supported_input(path) {
            usize::from(self.push_unique(path))
        } else {
            // A non-image file or something that vanished. Silently ignored:
            // dropping a folder of mixed content should just work.
            0
        }
    }

    /// Push one file if it is not already queued. Returns true when added.
    fn push_unique(&mut self, path: &Path) -> bool {
        let key = normalise(path);
        if self.seen.contains(&key) {
            return false;
        }
        self.seen.insert(key);
        self.jobs.push(Job::new(path.to_path_buf()));
        true
    }

    /// Remove the job at `index`, keeping the selection sensible.
    pub fn remove(&mut self, index: usize) {
        if index >= self.jobs.len() {
            return;
        }
        let job = self.jobs.remove(index);
        self.seen.remove(&normalise(&job.input));

        // Keep the selection pointing at roughly the same place in the list.
        self.selected = match self.selected {
            None => None,
            Some(sel) if self.jobs.is_empty() => {
                let _ = sel;
                None
            }
            Some(sel) if sel > index => Some(sel - 1),
            Some(sel) if sel == index => Some(sel.min(self.jobs.len() - 1)),
            Some(sel) => Some(sel),
        };
    }

    /// Drop every job.
    pub fn clear(&mut self) {
        self.jobs.clear();
        self.seen.clear();
        self.selected = None;
    }

    /// Drop only the jobs that finished, keeping failures for another attempt.
    pub fn clear_completed(&mut self) {
        let mut kept = Vec::with_capacity(self.jobs.len());
        for job in self.jobs.drain(..) {
            let finished_successfully = matches!(
                job.status,
                JobStatus::Done { .. } | JobStatus::Skipped { .. }
            );
            if finished_successfully {
                self.seen.remove(&normalise(&job.input));
            } else {
                kept.push(job);
            }
        }
        self.jobs = kept;
        // The old index may now point anywhere, so clamp it.
        self.selected = match self.selected {
            Some(_) if self.jobs.is_empty() => None,
            Some(sel) => Some(sel.min(self.jobs.len() - 1)),
            None => None,
        };
    }

    /// Move the selection down one row, for arrow-key navigation.
    ///
    /// Returns true when the selection moved.
    pub fn select_next(&mut self) -> bool {
        if self.jobs.is_empty() {
            return false;
        }
        let next = match self.selected {
            // Arriving with nothing selected should land on the first row.
            None => 0,
            Some(current) if current + 1 < self.jobs.len() => current + 1,
            // Deliberately stops at the end rather than wrapping: wrapping in a
            // long list makes it easy to lose your place.
            Some(current) => current,
        };
        let moved = self.selected != Some(next);
        self.selected = Some(next);
        moved
    }

    /// Move the selection up one row.
    pub fn select_previous(&mut self) -> bool {
        if self.jobs.is_empty() {
            return false;
        }
        let previous = match self.selected {
            None => 0,
            Some(0) => 0,
            Some(current) => current - 1,
        };
        let moved = self.selected != Some(previous);
        self.selected = Some(previous);
        moved
    }

    /// Put every job back to queued, so a finished batch can be re-run.
    pub fn reset_statuses(&mut self) {
        for job in &mut self.jobs {
            job.status = JobStatus::Queued;
        }
    }

    /// Counts by status, for the summary line.
    pub fn tally(&self) -> Tally {
        let mut t = Tally::default();
        for job in &self.jobs {
            match job.status {
                JobStatus::Queued => t.queued += 1,
                JobStatus::Running => t.running += 1,
                JobStatus::Done { .. } => t.done += 1,
                JobStatus::Skipped { .. } => t.skipped += 1,
                JobStatus::Failed { .. } => t.failed += 1,
            }
        }
        t
    }
}

/// How many jobs are in each state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    pub queued: usize,
    pub running: usize,
    pub done: usize,
    pub skipped: usize,
    pub failed: usize,
}

impl Tally {
    pub fn total(self) -> usize {
        self.queued + self.running + self.done + self.skipped + self.failed
    }

    pub fn finished(self) -> usize {
        self.done + self.skipped + self.failed
    }

    /// Progress from 0.0 to 1.0. An empty queue counts as complete.
    pub fn fraction(self) -> f32 {
        let total = self.total();
        if total == 0 {
            return 1.0;
        }
        self.finished() as f32 / total as f32
    }
}

/// A key for duplicate detection.
///
/// Canonicalising resolves `.`, `..`, symlinks and short 8.3 names, so the same
/// file reached two ways is recognised once. It fails for files that no longer
/// exist, in which case the path is used as given.
///
/// On Windows, paths are case-insensitive, so the fallback is lowercased to
/// stop `A.JPG` and `a.jpg` both being queued.
fn normalise(path: &Path) -> PathBuf {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if cfg!(windows) {
        PathBuf::from(resolved.to_string_lossy().to_lowercase())
    } else {
        resolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a directory tree of empty files for scan tests.
    fn make_tree(root: &Path, relative_paths: &[&str]) {
        for rel in relative_paths {
            let full = root.join(rel);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&full, b"x").unwrap();
        }
    }

    #[test]
    fn recognises_image_extensions_case_insensitively() {
        assert!(is_supported_input(Path::new("a.jpg")));
        assert!(is_supported_input(Path::new("a.JPG")));
        assert!(is_supported_input(Path::new("a.JpEg")));
        assert!(is_supported_input(Path::new("a.webp")));
        assert!(is_supported_input(Path::new("a.AVIF")));
        assert!(is_supported_input(Path::new("a.tiff")));
    }

    #[test]
    fn rejects_non_images() {
        assert!(!is_supported_input(Path::new("notes.txt")));
        assert!(!is_supported_input(Path::new("archive.zip")));
        assert!(!is_supported_input(Path::new("no-extension")));
        assert!(!is_supported_input(Path::new("video.mp4")));
        // A directory-looking name with no extension.
        assert!(!is_supported_input(Path::new("folder")));
    }

    #[test]
    fn adding_a_folder_finds_images_recursively_and_filters_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(
            dir.path(),
            &[
                "top.jpg",
                "notes.txt",
                "sub/inner.png",
                "sub/deeper/deep.webp",
                "sub/readme.md",
                "sub/deeper/thumbs.db",
            ],
        );

        let mut queue = JobQueue::default();
        let added = queue.add_paths([dir.path()]);

        assert_eq!(added, 3, "should find exactly the three images");
        let mut names: Vec<String> = queue.jobs().iter().map(|j| j.display_name()).collect();
        names.sort();
        assert_eq!(names, vec!["deep.webp", "inner.png", "top.jpg"]);
    }

    #[test]
    fn adding_the_same_folder_twice_adds_nothing_the_second_time() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["a.jpg", "b.png"]);

        let mut queue = JobQueue::default();
        assert_eq!(queue.add_paths([dir.path()]), 2);
        assert_eq!(
            queue.add_paths([dir.path()]),
            0,
            "the second drop should be a no-op"
        );
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn adding_a_file_already_present_via_its_folder_is_deduplicated() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["a.jpg"]);

        let mut queue = JobQueue::default();
        queue.add_paths([dir.path()]);
        assert_eq!(queue.len(), 1);

        // Now drop the individual file that the folder scan already picked up.
        assert_eq!(queue.add_paths([dir.path().join("a.jpg")]), 0);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn a_path_reached_via_a_dot_segment_is_recognised_as_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["sub/a.jpg"]);

        let mut queue = JobQueue::default();
        queue.add_paths([dir.path().join("sub").join("a.jpg")]);

        // Same file, spelled with a redundant "./" style detour.
        let indirect = dir.path().join("sub").join(".").join("a.jpg");
        assert_eq!(
            queue.add_paths([indirect]),
            0,
            "canonicalisation should collapse the detour"
        );
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn dropping_a_mix_of_files_and_folders_works() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["loose.png", "folder/a.jpg", "folder/b.txt"]);

        let mut queue = JobQueue::default();
        let added = queue.add_paths([dir.path().join("loose.png"), dir.path().join("folder")]);
        assert_eq!(added, 2);
    }

    #[test]
    fn a_nonexistent_path_is_ignored_rather_than_queued() {
        let mut queue = JobQueue::default();
        assert_eq!(queue.add_paths([Path::new("no/such/file.jpg")]), 0);
        assert!(queue.is_empty());
    }

    #[test]
    fn the_first_added_job_becomes_selected() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["a.jpg", "b.jpg"]);

        let mut queue = JobQueue::default();
        assert_eq!(queue.selected, None);
        queue.add_paths([dir.path()]);
        assert_eq!(queue.selected, Some(0), "something should be previewable");
    }

    #[test]
    fn adding_more_files_does_not_move_an_existing_selection() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["a.jpg", "b.jpg"]);
        let mut queue = JobQueue::default();
        queue.add_paths([dir.path()]);
        queue.selected = Some(1);

        make_tree(dir.path(), &["c.jpg"]);
        queue.add_paths([dir.path()]);
        assert_eq!(queue.selected, Some(1), "the user's choice should stand");
    }

    #[test]
    fn removing_a_job_before_the_selection_shifts_it_down() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["a.jpg", "b.jpg", "c.jpg"]);
        let mut queue = JobQueue::default();
        queue.add_paths([dir.path()]);
        queue.selected = Some(2);

        queue.remove(0);
        assert_eq!(
            queue.selected,
            Some(1),
            "the same file should still be selected"
        );
    }

    #[test]
    fn removing_the_selected_job_keeps_a_valid_selection() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["a.jpg", "b.jpg", "c.jpg"]);
        let mut queue = JobQueue::default();
        queue.add_paths([dir.path()]);

        queue.selected = Some(1);
        queue.remove(1);
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.selected, Some(1), "should land on the next item");

        // Removing the last item must clamp, not dangle.
        queue.selected = Some(1);
        queue.remove(1);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.selected, Some(0));
    }

    #[test]
    fn removing_the_only_job_clears_the_selection() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["a.jpg"]);
        let mut queue = JobQueue::default();
        queue.add_paths([dir.path()]);

        queue.remove(0);
        assert!(queue.is_empty());
        assert_eq!(queue.selected, None, "nothing left to select");
    }

    #[test]
    fn removing_an_out_of_range_index_is_harmless() {
        let mut queue = JobQueue::default();
        queue.remove(5);
        assert!(queue.is_empty());
    }

    #[test]
    fn a_removed_file_can_be_added_again() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["a.jpg"]);
        let mut queue = JobQueue::default();
        queue.add_paths([dir.path()]);
        queue.remove(0);

        assert_eq!(
            queue.add_paths([dir.path().join("a.jpg")]),
            1,
            "removal should also forget the dedupe entry"
        );
    }

    #[test]
    fn clearing_empties_everything_including_the_dedupe_set() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["a.jpg", "b.jpg"]);
        let mut queue = JobQueue::default();
        queue.add_paths([dir.path()]);

        queue.clear();
        assert!(queue.is_empty());
        assert_eq!(queue.selected, None);
        // And the same files can be re-added.
        assert_eq!(queue.add_paths([dir.path()]), 2);
    }

    #[test]
    fn clearing_completed_keeps_failures_and_pending_work() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["done.jpg", "failed.jpg", "queued.jpg"]);
        let mut queue = JobQueue::default();
        queue.add_paths([dir.path()]);

        // Assign statuses by name so the test does not depend on scan order.
        for job in queue.jobs_mut() {
            let name = job.display_name();
            job.status = match name.as_str() {
                "done.jpg" => JobStatus::Done {
                    output: PathBuf::from("done.webp"),
                    bytes: 10,
                },
                "failed.jpg" => JobStatus::Failed {
                    message: "boom".into(),
                    command: None,
                },
                _ => JobStatus::Queued,
            };
        }

        queue.clear_completed();

        let names: std::collections::HashSet<String> =
            queue.jobs().iter().map(|j| j.display_name()).collect();
        assert!(!names.contains("done.jpg"), "successes should be cleared");
        assert!(
            names.contains("failed.jpg"),
            "failures should stay so they can be retried"
        );
        assert!(names.contains("queued.jpg"), "pending work should stay");
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn clearing_completed_on_an_all_done_queue_clears_the_selection() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["a.jpg"]);
        let mut queue = JobQueue::default();
        queue.add_paths([dir.path()]);
        queue.jobs_mut()[0].status = JobStatus::Done {
            output: PathBuf::from("a.webp"),
            bytes: 1,
        };

        queue.clear_completed();
        assert!(queue.is_empty());
        assert_eq!(queue.selected, None);
    }

    #[test]
    fn resetting_statuses_requeues_everything() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["a.jpg", "b.jpg"]);
        let mut queue = JobQueue::default();
        queue.add_paths([dir.path()]);
        queue.jobs_mut()[0].status = JobStatus::Failed {
            message: "x".into(),
            command: None,
        };

        queue.reset_statuses();
        assert!(queue.jobs().iter().all(|j| j.status == JobStatus::Queued));
    }

    #[test]
    fn arrow_navigation_walks_the_list_and_stops_at_the_ends() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["a.jpg", "b.jpg", "c.jpg"]);
        let mut queue = JobQueue::default();
        queue.add_paths([dir.path()]);
        assert_eq!(queue.selected, Some(0));

        assert!(queue.select_next());
        assert_eq!(queue.selected, Some(1));
        assert!(queue.select_next());
        assert_eq!(queue.selected, Some(2));

        // Stops at the bottom rather than wrapping around.
        assert!(!queue.select_next(), "should report no movement at the end");
        assert_eq!(queue.selected, Some(2));

        assert!(queue.select_previous());
        assert_eq!(queue.selected, Some(1));
        assert!(queue.select_previous());
        assert_eq!(queue.selected, Some(0));
        assert!(!queue.select_previous(), "should stop at the top");
        assert_eq!(queue.selected, Some(0));
    }

    #[test]
    fn arrow_navigation_from_no_selection_lands_on_the_first_row() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["a.jpg", "b.jpg"]);
        let mut queue = JobQueue::default();
        queue.add_paths([dir.path()]);

        queue.selected = None;
        assert!(queue.select_next());
        assert_eq!(queue.selected, Some(0));

        queue.selected = None;
        assert!(queue.select_previous());
        assert_eq!(queue.selected, Some(0));
    }

    #[test]
    fn arrow_navigation_on_an_empty_list_does_nothing() {
        let mut queue = JobQueue::default();
        assert!(!queue.select_next());
        assert!(!queue.select_previous());
        assert_eq!(queue.selected, None, "must not select a row that is not there");
    }

    #[test]
    fn tally_counts_each_state() {
        let dir = tempfile::tempdir().unwrap();
        make_tree(dir.path(), &["a.jpg", "b.jpg", "c.jpg", "d.jpg"]);
        let mut queue = JobQueue::default();
        queue.add_paths([dir.path()]);

        queue.jobs_mut()[0].status = JobStatus::Done {
            output: PathBuf::from("x"),
            bytes: 1,
        };
        queue.jobs_mut()[1].status = JobStatus::Failed {
            message: "x".into(),
            command: None,
        };
        queue.jobs_mut()[2].status = JobStatus::Running;

        let t = queue.tally();
        assert_eq!(t.done, 1);
        assert_eq!(t.failed, 1);
        assert_eq!(t.running, 1);
        assert_eq!(t.queued, 1);
        assert_eq!(t.total(), 4);
        assert_eq!(t.finished(), 2);
        assert!((t.fraction() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn an_empty_tally_reads_as_complete_not_divided_by_zero() {
        let t = Tally::default();
        assert_eq!(t.total(), 0);
        assert_eq!(t.fraction(), 1.0);
    }

    #[test]
    fn status_icons_and_labels_are_distinct_so_colour_is_never_the_only_cue() {
        let statuses = [
            JobStatus::Queued,
            JobStatus::Running,
            JobStatus::Done {
                output: PathBuf::new(),
                bytes: 0,
            },
            JobStatus::Skipped {
                output: PathBuf::new(),
            },
            JobStatus::Failed {
                message: String::new(),
                command: None,
            },
        ];

        let labels: std::collections::HashSet<&str> =
            statuses.iter().map(JobStatus::label).collect();
        assert_eq!(labels.len(), statuses.len(), "labels must be distinguishable");

        let icons: std::collections::HashSet<&str> = statuses.iter().map(JobStatus::icon).collect();
        assert_eq!(icons.len(), statuses.len(), "icons must be distinguishable");
    }

    #[test]
    fn only_terminal_statuses_report_as_finished() {
        assert!(!JobStatus::Queued.is_finished());
        assert!(!JobStatus::Running.is_finished());
        assert!(
            JobStatus::Done {
                output: PathBuf::new(),
                bytes: 0
            }
            .is_finished()
        );
        assert!(
            JobStatus::Skipped {
                output: PathBuf::new()
            }
            .is_finished()
        );
        assert!(
            JobStatus::Failed {
                message: String::new(),
                command: None
            }
            .is_finished()
        );
    }
}
