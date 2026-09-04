//! The worker pool against real vips subprocesses.
//!
//! The unit tests in `worker.rs` cover the pool mechanics with a stub. These
//! check the parts that only show up with real child processes: that
//! parallelism helps, that cancel kills in-flight children promptly, and that
//! no `vips` processes are left behind.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use vips_gui::job::JobStatus;
use vips_gui::settings::{ConversionSettings, OutputDestination, OutputFormat};
use vips_gui::worker::{BatchRunner, Converter, JobUpdate, VipsConverter};

/// Drive a runner to completion the way the UI does, collecting updates.
fn drain_until_finished(runner: &BatchRunner, timeout: Duration) -> Vec<JobUpdate> {
    let deadline = Instant::now() + timeout;
    let mut updates = Vec::new();
    while Instant::now() < deadline {
        updates.extend(runner.drain());
        if runner.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        runner.is_finished(),
        "the pool did not finish within {timeout:?}"
    );
    updates
}

/// Count running `vips` processes, so orphans can be detected.
///
/// Uses `tasklist` on Windows and `pgrep` elsewhere. Returns `None` when the
/// count cannot be established, so the test can skip rather than fail for an
/// environmental reason.
fn count_vips_processes() -> Option<usize> {
    if cfg!(windows) {
        let mut cmd = std::process::Command::new("tasklist");
        cmd.args(["/FI", "IMAGENAME eq vips.exe", "/NH"]);
        vips_gui::vips::hide_console(&mut cmd);
        let output = cmd.output().ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        // tasklist prints an "INFO: No tasks..." line when nothing matches.
        if text.contains("No tasks") {
            return Some(0);
        }
        Some(
            text.lines()
                .filter(|l| l.to_ascii_lowercase().contains("vips.exe"))
                .count(),
        )
    } else {
        let output = std::process::Command::new("pgrep")
            .args(["-x", "vips"])
            .output()
            .ok()?;
        Some(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count(),
        )
    }
}

#[test]
fn a_real_batch_completes_every_job_across_several_workers() {
    let Some(install) = common::vips() else {
        common::skip("a_real_batch_completes_every_job_across_several_workers");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let sources_dir = dir.path().join("src");
    std::fs::create_dir_all(&sources_dir).expect("create src");

    const COUNT: usize = 12;
    let jobs: Vec<(usize, std::path::PathBuf)> = (0..COUNT)
        .map(|i| {
            let path = sources_dir.join(format!("img{i}.png"));
            common::make_source_image(install, &path, 200, 150);
            (i, path)
        })
        .collect();

    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Webp;
    settings.destination = OutputDestination::Directory(dir.path().join("out"));

    let converter: Arc<dyn Converter> = Arc::new(VipsConverter::new(install.clone(), 2));
    let runner = BatchRunner::start(converter, settings, jobs, 4, None);

    let mut updates = drain_until_finished(&runner, Duration::from_secs(60));
    assert_eq!(runner.total(), COUNT);
    updates.extend(runner.join());

    // Every index should end in Done.
    let mut finals = std::collections::HashMap::new();
    for update in &updates {
        finals.insert(update.index, update.status.clone());
    }
    assert_eq!(finals.len(), COUNT, "every job should report");
    for index in 0..COUNT {
        assert!(
            matches!(finals[&index], JobStatus::Done { .. }),
            "job {index} ended as {:?}",
            finals[&index]
        );
    }

    // And the files really exist.
    let written = std::fs::read_dir(dir.path().join("out"))
        .expect("read out dir")
        .flatten()
        .count();
    assert_eq!(written, COUNT, "every output file should be on disk");
}

#[test]
fn parallel_conversion_beats_a_single_worker_on_real_encodes() {
    let Some(install) = common::vips() else {
        common::skip("parallel_conversion_beats_a_single_worker_on_real_encodes");
        return;
    };
    if !install.capabilities.avif {
        eprintln!("SKIPPING: needs heifsave to make the encode slow enough to measure");
        return;
    }
    // Parallel speedup is only observable with more than one core.
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    if cores < 4 {
        eprintln!("SKIPPING: only {cores} cores, parallel timing would be noise");
        return;
    }

    let dir = tempfile::tempdir().expect("temp dir");
    let sources_dir = dir.path().join("src");
    std::fs::create_dir_all(&sources_dir).expect("create src");

    const COUNT: usize = 8;
    let sources: Vec<std::path::PathBuf> = (0..COUNT)
        .map(|i| {
            let path = sources_dir.join(format!("img{i}.png"));
            common::make_source_image(install, &path, 500, 400);
            path
        })
        .collect();

    // AVIF at a moderate effort gives each job enough work that thread
    // scheduling is not the dominant cost.
    let make_settings = |subdir: &str| {
        let mut s = ConversionSettings::default();
        s.format = OutputFormat::Avif;
        s.avif.effort = 4;
        s.avif.quality = 50;
        s.destination = OutputDestination::Directory(dir.path().join(subdir));
        s
    };

    let time_with = |workers: usize, vips_concurrency: usize, subdir: &str| -> Duration {
        let jobs: Vec<(usize, std::path::PathBuf)> =
            sources.iter().cloned().enumerate().collect();
        let converter: Arc<dyn Converter> =
            Arc::new(VipsConverter::new(install.clone(), vips_concurrency));
        let start = Instant::now();
        let runner = BatchRunner::start(converter, make_settings(subdir), jobs, workers, None);
        drain_until_finished(&runner, Duration::from_secs(300));
        runner.join();
        start.elapsed()
    };

    // One worker with a full thread pool, versus several workers each with a
    // small one. The latter is the configuration the app defaults to.
    let serial = time_with(1, 2, "serial");
    let parallel = time_with(4, 2, "parallel");

    assert!(
        parallel < serial,
        "4 workers took {parallel:?} but 1 worker took {serial:?}; \
         parallelism should help on {cores} cores"
    );
}

#[test]
fn cancelling_a_real_batch_stops_promptly_and_leaves_no_orphan_processes() {
    let Some(install) = common::vips() else {
        common::skip("cancelling_a_real_batch_stops_promptly_and_leaves_no_orphan_processes");
        return;
    };
    if !install.capabilities.avif {
        eprintln!("SKIPPING: needs heifsave to make jobs slow enough to interrupt");
        return;
    }

    let baseline = count_vips_processes();

    let dir = tempfile::tempdir().expect("temp dir");
    let sources_dir = dir.path().join("src");
    std::fs::create_dir_all(&sources_dir).expect("create src");

    // Large sources at high AVIF effort: each job takes long enough that a
    // cancel is guaranteed to land in the middle of one.
    const COUNT: usize = 24;
    let jobs: Vec<(usize, std::path::PathBuf)> = (0..COUNT)
        .map(|i| {
            let path = sources_dir.join(format!("img{i}.png"));
            common::make_source_image(install, &path, 1200, 900);
            (i, path)
        })
        .collect();

    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Avif;
    settings.avif.effort = 8;
    settings.destination = OutputDestination::Directory(dir.path().join("out"));

    let converter: Arc<dyn Converter> = Arc::new(VipsConverter::new(install.clone(), 2));
    let runner = BatchRunner::start(converter, settings, jobs, 2, None);

    // Let real encoding get under way.
    std::thread::sleep(Duration::from_millis(600));
    assert!(
        !runner.is_finished(),
        "the batch should still be running; make the workload heavier"
    );

    let cancelled_at = Instant::now();
    runner.cancel();

    let mut updates = drain_until_finished(&runner, Duration::from_secs(30));
    let stop_time = cancelled_at.elapsed();
    updates.extend(runner.join());

    // Killing a child should be near-instant. The whole batch at effort 8 would
    // take minutes, so anything of this order proves the child was killed
    // rather than allowed to finish.
    assert!(
        stop_time < Duration::from_secs(5),
        "cancel took {stop_time:?}; the in-flight child was probably not killed"
    );

    // Nothing should be left mid-flight.
    let mut finals = std::collections::HashMap::new();
    for update in &updates {
        finals.insert(update.index, update.status.clone());
    }
    assert!(
        !finals.values().any(|s| *s == JobStatus::Running),
        "no job should still report Running after the pool stopped"
    );

    // Most jobs should never have been attempted.
    let attempted = finals.len();
    assert!(
        attempted < COUNT,
        "cancel should have prevented some jobs from starting, {attempted} of {COUNT} reported"
    );

    // No orphaned children. Give the OS a moment to reap the killed processes.
    if let (Some(before), Some(_)) = (baseline, count_vips_processes()) {
        let mut after = count_vips_processes().unwrap_or(before);
        for _ in 0..40 {
            if after <= before {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            after = count_vips_processes().unwrap_or(before);
        }
        assert!(
            after <= before,
            "{after} vips processes running after cancel, started from {before}; \
             children were orphaned"
        );
    } else {
        eprintln!("NOTE: could not count vips processes, skipped the orphan check");
    }
}

#[test]
fn a_cancelled_job_leaves_no_truncated_output_behind() {
    let Some(install) = common::vips() else {
        common::skip("a_cancelled_job_leaves_no_truncated_output_behind");
        return;
    };
    if !install.capabilities.avif {
        eprintln!("SKIPPING: needs heifsave for a slow enough encode");
        return;
    }

    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("big.png");
    common::make_source_image(install, &source, 1600, 1200);
    let out_dir = dir.path().join("out");

    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Avif;
    settings.avif.effort = 9;
    settings.destination = OutputDestination::Directory(out_dir.clone());

    let converter: Arc<dyn Converter> = Arc::new(VipsConverter::new(install.clone(), 2));
    let runner = BatchRunner::start(converter, settings, vec![(0, source)], 1, None);

    std::thread::sleep(Duration::from_millis(400));
    runner.cancel();
    drain_until_finished(&runner, Duration::from_secs(30));
    runner.join();

    // A half-written AVIF would look like a successful conversion to anyone
    // browsing the output folder, so the cancel path removes it.
    if out_dir.is_dir() {
        let leftovers: Vec<String> = std::fs::read_dir(&out_dir)
            .expect("read out dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            leftovers.is_empty(),
            "a cancelled conversion should not leave partial files: {leftovers:?}"
        );
    }
}

#[test]
fn a_cancelled_batch_can_be_run_again_to_completion() {
    let Some(install) = common::vips() else {
        common::skip("a_cancelled_batch_can_be_run_again_to_completion");
        return;
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let sources_dir = dir.path().join("src");
    std::fs::create_dir_all(&sources_dir).expect("create src");

    const COUNT: usize = 6;
    let jobs: Vec<(usize, std::path::PathBuf)> = (0..COUNT)
        .map(|i| {
            let path = sources_dir.join(format!("img{i}.png"));
            common::make_source_image(install, &path, 300, 200);
            (i, path)
        })
        .collect();

    let mut settings = ConversionSettings::default();
    settings.format = OutputFormat::Webp;
    settings.webp.effort = 6;
    settings.destination = OutputDestination::Directory(dir.path().join("out"));

    // First attempt, cancelled almost immediately.
    let converter: Arc<dyn Converter> = Arc::new(VipsConverter::new(install.clone(), 1));
    let runner = BatchRunner::start(
        Arc::clone(&converter),
        settings.clone(),
        jobs.clone(),
        1,
        None,
    );
    runner.cancel();
    drain_until_finished(&runner, Duration::from_secs(30));
    runner.join();

    // Second attempt with everything, which must succeed regardless of what
    // the first attempt managed to write.
    let runner = BatchRunner::start(converter, settings, jobs, 3, None);
    let mut updates = drain_until_finished(&runner, Duration::from_secs(120));
    updates.extend(runner.join());

    let mut finals = std::collections::HashMap::new();
    for update in &updates {
        finals.insert(update.index, update.status.clone());
    }
    for index in 0..COUNT {
        let status = finals
            .get(&index)
            .unwrap_or_else(|| panic!("job {index} never reported on the re-run"));
        assert!(
            matches!(status, JobStatus::Done { .. } | JobStatus::Skipped { .. }),
            "job {index} should have completed on the second run, was {status:?}"
        );
    }
}

#[test]
fn conversions_succeed_at_every_offered_concurrency_setting() {
    let Some(install) = common::vips() else {
        common::skip("conversions_succeed_at_every_offered_concurrency_setting");
        return;
    };

    // That the variable is constructed correctly is covered by the unit test
    // for `concurrency_env`. What this adds is that libvips actually accepts
    // the values the UI can produce, across the range of the slider.
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("a.png");
    common::make_source_image(install, &source, 100, 80);

    let mut settings = ConversionSettings::default();
    settings.destination = OutputDestination::Directory(dir.path().join("out"));

    for concurrency in [1usize, 2, 8] {
        let converter: Arc<dyn Converter> =
            Arc::new(VipsConverter::new(install.clone(), concurrency));
        let mut per_run = settings.clone();
        per_run.destination = OutputDestination::Directory(dir.path().join(format!("c{concurrency}")));

        let runner = BatchRunner::start(converter, per_run, vec![(0, source.clone())], 1, None);
        let mut updates = drain_until_finished(&runner, Duration::from_secs(60));
        updates.extend(runner.join());

        let status = updates
            .iter()
            .rev()
            .find(|u| u.index == 0 && u.status.is_finished())
            .map(|u| u.status.clone())
            .unwrap_or_else(|| panic!("no final status at concurrency {concurrency}"));
        assert!(
            matches!(status, JobStatus::Done { .. }),
            "conversion should succeed with VIPS_CONCURRENCY={concurrency}, got {status:?}"
        );
    }
}
