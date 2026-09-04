//! The Task 4 workflow end to end: point the queue at a folder of mixed
//! content, then convert everything it collected.
//!
//! Drag-and-drop itself cannot be driven headlessly, but the drop handler does
//! nothing more than hand the paths to `JobQueue::add_paths`, which is exactly
//! what this exercises.

mod common;

use std::path::Path;

use vips_gui::job::{JobQueue, JobStatus};
use vips_gui::settings::{ConversionSettings, OutputDestination, OutputFormat};
use vips_gui::vips::convert::{self, Outcome};

#[test]
fn a_dropped_folder_of_mixed_content_converts_only_the_images() {
    let Some(install) = common::vips() else {
        common::skip("a_dropped_folder_of_mixed_content_converts_only_the_images");
        return;
    };

    let root = tempfile::tempdir().expect("temp dir");
    let source_dir = root.path().join("incoming");
    std::fs::create_dir_all(source_dir.join("nested").join("deeper")).expect("create tree");

    // Five real images, spread across three directory levels.
    let images = [
        source_dir.join("one.png"),
        source_dir.join("two.jpg"),
        source_dir.join("nested").join("three.webp"),
        source_dir.join("nested").join("four.png"),
        source_dir.join("nested").join("deeper").join("five.jpg"),
    ];
    for path in &images {
        common::make_source_image(install, path, 80, 60);
    }

    // Assorted things that must not be picked up.
    let decoys = [
        source_dir.join("notes.txt"),
        source_dir.join("nested").join("archive.zip"),
        source_dir.join("nested").join("deeper").join("README.md"),
        source_dir.join("no-extension"),
    ];
    for path in &decoys {
        std::fs::write(path, b"not an image").expect("write decoy");
    }

    // The drop.
    let mut queue = JobQueue::default();
    let added = queue.add_paths([&source_dir]);

    assert_eq!(
        added,
        images.len(),
        "should collect exactly the five images, got {} jobs: {:?}",
        queue.len(),
        queue
            .jobs()
            .iter()
            .map(|j| j.display_name())
            .collect::<Vec<_>>()
    );
    for decoy in &decoys {
        let name = decoy.file_name().unwrap().to_string_lossy();
        assert!(
            !queue.jobs().iter().any(|j| j.display_name() == name),
            "{name} should have been filtered out"
        );
    }
    assert_eq!(queue.selected, Some(0), "a row should be preselected");

    // Every job knows its input size, which the list uses before conversion.
    assert!(
        queue.jobs().iter().all(|j| j.input_bytes.is_some_and(|b| b > 0)),
        "input sizes should have been read at add time"
    );

    // Convert sequentially into one output folder, mirroring the Convert button.
    let out_dir = root.path().join("converted");
    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Webp;
    settings.destination = OutputDestination::Directory(out_dir.clone());

    for index in 0..queue.len() {
        let input = queue.jobs()[index].input.clone();
        let status = match convert::convert_one(install, &settings, &input) {
            Ok(Outcome::Written { output, bytes }) => JobStatus::Done { output, bytes },
            Ok(Outcome::Skipped { output }) => JobStatus::Skipped { output },
            Err(err) => JobStatus::Failed {
                message: err.to_string(),
                command: None,
            },
        };
        queue.jobs_mut()[index].status = status;
    }

    let tally = queue.tally();
    assert_eq!(
        (tally.done, tally.failed, tally.skipped),
        (5, 0, 0),
        "all five should convert cleanly: {tally:?}"
    );
    assert_eq!(tally.fraction(), 1.0, "progress should read as complete");

    // The output directory was created on demand and holds five webp files.
    assert!(out_dir.is_dir(), "the output folder should have been created");
    let mut written: Vec<String> = std::fs::read_dir(&out_dir)
        .expect("read output dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();
    assert_eq!(
        written,
        vec![
            "five.webp",
            "four.webp",
            "one.webp",
            "three.webp",
            "two.webp"
        ]
    );

    // And each one is a real image at the original size.
    for name in &written {
        let (w, h) = common::image_size(install, &out_dir.join(name));
        assert_eq!((w, h), (80, 60), "{name} should be readable at 80x60");
    }
}

#[test]
fn re_dropping_the_same_folder_adds_nothing_and_keeps_statuses() {
    let Some(install) = common::vips() else {
        common::skip("re_dropping_the_same_folder_adds_nothing_and_keeps_statuses");
        return;
    };

    let root = tempfile::tempdir().expect("temp dir");
    let source_dir = root.path().join("pics");
    std::fs::create_dir_all(&source_dir).expect("create dir");
    common::make_source_image(install, &source_dir.join("a.png"), 40, 40);
    common::make_source_image(install, &source_dir.join("b.png"), 40, 40);

    let mut queue = JobQueue::default();
    assert_eq!(queue.add_paths([&source_dir]), 2);

    // Mark one as done, then re-drop the folder.
    queue.jobs_mut()[0].status = JobStatus::Done {
        output: std::path::PathBuf::from("a.webp"),
        bytes: 123,
    };

    assert_eq!(
        queue.add_paths([&source_dir]),
        0,
        "a second drop of the same folder should be a no-op"
    );
    assert_eq!(queue.len(), 2);
    assert!(
        matches!(queue.jobs()[0].status, JobStatus::Done { .. }),
        "an existing job's status must survive a re-drop"
    );
}

#[test]
fn same_as_source_writes_next_to_each_original_across_subfolders() {
    let Some(install) = common::vips() else {
        common::skip("same_as_source_writes_next_to_each_original_across_subfolders");
        return;
    };

    let root = tempfile::tempdir().expect("temp dir");
    let a = root.path().join("albumA");
    let b = root.path().join("albumB").join("sub");
    std::fs::create_dir_all(&a).expect("create a");
    std::fs::create_dir_all(&b).expect("create b");
    common::make_source_image(install, &a.join("first.png"), 50, 50);
    common::make_source_image(install, &b.join("second.png"), 50, 50);

    let mut queue = JobQueue::default();
    queue.add_paths([root.path()]);
    assert_eq!(queue.len(), 2);

    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Jpeg;
    settings.destination = OutputDestination::SameAsSource;

    for index in 0..queue.len() {
        let input = queue.jobs()[index].input.clone();
        convert::convert_one(install, &settings, &input).expect("conversion should work");
    }

    // Each output sits beside its own source, not pooled together.
    assert!(a.join("first.jpg").is_file(), "albumA/first.jpg missing");
    assert!(b.join("second.jpg").is_file(), "albumB/sub/second.jpg missing");
}

#[test]
fn one_bad_file_does_not_stop_the_rest_of_the_batch() {
    let Some(install) = common::vips() else {
        common::skip("one_bad_file_does_not_stop_the_rest_of_the_batch");
        return;
    };

    let root = tempfile::tempdir().expect("temp dir");
    let source_dir = root.path().join("mixed");
    std::fs::create_dir_all(&source_dir).expect("create dir");

    common::make_source_image(install, &source_dir.join("good1.png"), 40, 40);
    // A file with an image extension but garbage content: it passes the
    // extension filter and only fails once vips looks at it.
    std::fs::write(source_dir.join("broken.png"), b"definitely not a png").expect("write broken");
    common::make_source_image(install, &source_dir.join("good2.png"), 40, 40);

    let mut queue = JobQueue::default();
    assert_eq!(queue.add_paths([&source_dir]), 3);

    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Webp;
    settings.destination = OutputDestination::Directory(root.path().join("out"));

    for index in 0..queue.len() {
        let input = queue.jobs()[index].input.clone();
        let status = match convert::convert_one(install, &settings, &input) {
            Ok(Outcome::Written { output, bytes }) => JobStatus::Done { output, bytes },
            Ok(Outcome::Skipped { output }) => JobStatus::Skipped { output },
            Err(err) => JobStatus::Failed {
                message: err.to_string(),
                command: None,
            },
        };
        queue.jobs_mut()[index].status = status;
    }

    let tally = queue.tally();
    assert_eq!(tally.done, 2, "the two good files should convert");
    assert_eq!(tally.failed, 1, "the broken one should be marked failed");

    // The failure carries a message worth showing.
    let failure = queue
        .jobs()
        .iter()
        .find(|j| matches!(j.status, JobStatus::Failed { .. }))
        .expect("a failed job");
    let JobStatus::Failed { message, .. } = &failure.status else {
        unreachable!()
    };
    assert!(!message.is_empty(), "a failure needs an explanation");
    assert_eq!(failure.display_name(), "broken.png");
}

#[test]
fn clearing_finished_leaves_only_the_failure_to_retry() {
    let Some(install) = common::vips() else {
        common::skip("clearing_finished_leaves_only_the_failure_to_retry");
        return;
    };

    let root = tempfile::tempdir().expect("temp dir");
    let dir = root.path().join("q");
    std::fs::create_dir_all(&dir).expect("create dir");
    common::make_source_image(install, &dir.join("ok.png"), 30, 30);
    std::fs::write(dir.join("bad.png"), b"nope").expect("write bad");

    let mut queue = JobQueue::default();
    queue.add_paths([&dir]);

    let mut settings = ConversionSettings::default();
    settings.destination = OutputDestination::Directory(root.path().join("out"));

    for index in 0..queue.len() {
        let input = queue.jobs()[index].input.clone();
        queue.jobs_mut()[index].status = match convert::convert_one(install, &settings, &input) {
            Ok(Outcome::Written { output, bytes }) => JobStatus::Done { output, bytes },
            Ok(Outcome::Skipped { output }) => JobStatus::Skipped { output },
            Err(err) => JobStatus::Failed {
                message: err.to_string(),
                command: None,
            },
        };
    }

    queue.clear_completed();

    assert_eq!(queue.len(), 1, "only the failure should remain");
    assert_eq!(queue.jobs()[0].display_name(), "bad.png");
    assert!(
        queue.selected.is_some_and(|s| s < queue.len()),
        "the selection must stay in range after clearing"
    );
}

#[test]
fn extension_filtering_matches_what_vips_can_actually_read() {
    // A guard against the input filter drifting away from reality: every
    // extension we advertise should at least be a format libvips knows.
    let Some(install) = common::vips() else {
        common::skip("extension_filtering_matches_what_vips_can_actually_read");
        return;
    };

    // Ask vips for its loader list once.
    let output = install
        .command()
        .arg("-l")
        .output()
        .expect("vips -l should run");
    let listing = String::from_utf8_lossy(&output.stdout);

    // These are the loaders behind the extensions we accept. If a build lacks
    // one, that is fine; we only care that the common ones are recognised, so
    // the filter is not advertising fantasy formats.
    for loader in ["jpegload", "pngload", "webpload", "tiffload"] {
        assert!(
            listing.contains(&format!("({loader})")),
            "expected {loader} in this libvips build"
        );
    }

    // And a sanity check that our own predicate agrees on the basics.
    assert!(vips_gui::job::is_supported_input(Path::new("x.jpg")));
    assert!(vips_gui::job::is_supported_input(Path::new("x.tiff")));
    assert!(!vips_gui::job::is_supported_input(Path::new("x.docx")));
}
