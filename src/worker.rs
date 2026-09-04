//! Running a batch across several threads, with progress reporting and cancel.
//!
//! Each worker thread pulls the next index from a shared counter and converts
//! that file in its own `vips` subprocess. Progress arrives on an `mpsc`
//! channel, which the UI drains once per frame. Cancellation is a single
//! `AtomicBool` that workers check between jobs and that
//! [`convert_one_cancellable`] also polls mid-encode so it can kill the child.
//!
//! [`convert_one_cancellable`]: crate::vips::convert::convert_one_cancellable

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, atomic::AtomicU64};
use std::thread::JoinHandle;

use crate::job::JobStatus;
use crate::settings::ConversionSettings;
use crate::vips::VipsInstall;
use crate::vips::convert::{ConvertError, Outcome};

/// The conversion step, abstracted so the pool can be tested without vips.
pub trait Converter: Send + Sync + 'static {
    /// Convert one file, honouring `cancel` as promptly as practical.
    fn convert(
        &self,
        settings: &ConversionSettings,
        input: &std::path::Path,
        cancel: &AtomicBool,
    ) -> Result<Outcome, ConvertError>;
}

/// The real converter: spawns `vips`.
pub struct VipsConverter {
    install: VipsInstall,
    /// Value for the child's `VIPS_CONCURRENCY`.
    vips_concurrency: usize,
}

impl VipsConverter {
    pub fn new(install: VipsInstall, vips_concurrency: usize) -> Self {
        Self {
            install,
            vips_concurrency,
        }
    }
}

impl Converter for VipsConverter {
    fn convert(
        &self,
        settings: &ConversionSettings,
        input: &std::path::Path,
        cancel: &AtomicBool,
    ) -> Result<Outcome, ConvertError> {
        crate::vips::convert::convert_one_cancellable(
            &self.install,
            settings,
            input,
            cancel,
            self.vips_concurrency,
        )
    }
}

/// One job's outcome, addressed by its index in the queue.
#[derive(Debug, Clone)]
pub struct JobUpdate {
    pub index: usize,
    pub status: JobStatus,
}

/// A batch in flight.
pub struct BatchRunner {
    updates: Receiver<JobUpdate>,
    cancel: Arc<AtomicBool>,
    /// Worker threads, taken when joined.
    handles: Vec<JoinHandle<()>>,
    /// Counts workers that have exited, so completion needs no thread probing.
    finished_workers: Arc<AtomicUsize>,
    worker_count: usize,
    /// Total jobs handed to the pool.
    total: usize,
    /// Wall-clock start, for the summary.
    started: std::time::Instant,
    /// Bytes written so far, summed across workers.
    bytes_written: Arc<AtomicU64>,
}

impl BatchRunner {
    /// Start converting `jobs`, which pairs each queue index with its input path.
    ///
    /// `repaint` is an egui context to wake when progress arrives; without it
    /// the UI would not refresh until the user moved the mouse.
    pub fn start(
        converter: Arc<dyn Converter>,
        settings: ConversionSettings,
        jobs: Vec<(usize, PathBuf)>,
        worker_count: usize,
        repaint: Option<egui::Context>,
    ) -> Self {
        let total = jobs.len();
        let worker_count = worker_count.clamp(1, total.max(1));

        let cancel = Arc::new(AtomicBool::new(false));
        let finished_workers = Arc::new(AtomicUsize::new(0));
        let bytes_written = Arc::new(AtomicU64::new(0));
        let (tx, updates) = channel();

        let jobs = Arc::new(jobs);
        let next = Arc::new(AtomicUsize::new(0));
        let settings = Arc::new(settings);

        let mut handles = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let worker = Worker {
                converter: Arc::clone(&converter),
                settings: Arc::clone(&settings),
                jobs: Arc::clone(&jobs),
                next: Arc::clone(&next),
                cancel: Arc::clone(&cancel),
                finished_workers: Arc::clone(&finished_workers),
                bytes_written: Arc::clone(&bytes_written),
                tx: tx.clone(),
                repaint: repaint.clone(),
            };
            handles.push(std::thread::spawn(move || worker.run()));
        }
        // Drop our copy so the channel closes once every worker is gone.
        drop(tx);

        Self {
            updates,
            cancel,
            handles,
            finished_workers,
            worker_count,
            total,
            started: std::time::Instant::now(),
            bytes_written,
        }
    }

    /// Take every update that has arrived since the last call.
    pub fn drain(&self) -> Vec<JobUpdate> {
        let mut out = Vec::new();
        loop {
            match self.updates.try_recv() {
                Ok(update) => out.push(update),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        out
    }

    /// Ask the batch to stop. Workers abandon their queue and kill any child.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// True once cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// True when every worker has exited.
    pub fn is_finished(&self) -> bool {
        self.finished_workers.load(Ordering::Acquire) >= self.worker_count
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes_written.load(Ordering::Relaxed)
    }

    /// Wait for the workers and return any updates that were still in flight.
    ///
    /// Always call this before dropping a finished runner, so the threads are
    /// reaped and no update is lost.
    pub fn join(mut self) -> Vec<JobUpdate> {
        for handle in self.handles.drain(..) {
            // A panicking worker should not take the UI down with it; the job
            // simply never reports, and the summary will show it unfinished.
            let _ = handle.join();
        }
        self.drain()
    }
}

impl Drop for BatchRunner {
    fn drop(&mut self) {
        // If a runner is dropped without joining (app closing mid-batch), make
        // sure the workers stop and their children die rather than lingering.
        self.cancel.store(true, Ordering::Relaxed);
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

/// The per-thread half of the pool.
struct Worker {
    converter: Arc<dyn Converter>,
    settings: Arc<ConversionSettings>,
    jobs: Arc<Vec<(usize, PathBuf)>>,
    next: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    finished_workers: Arc<AtomicUsize>,
    bytes_written: Arc<AtomicU64>,
    tx: Sender<JobUpdate>,
    repaint: Option<egui::Context>,
}

impl Worker {
    fn run(self) {
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                break;
            }

            // Claim the next job. `fetch_add` makes the hand-out lock-free and
            // means slow jobs cannot block other workers.
            let slot = self.next.fetch_add(1, Ordering::Relaxed);
            let Some((index, input)) = self.jobs.get(slot) else {
                break;
            };

            self.send(JobUpdate {
                index: *index,
                status: JobStatus::Running,
            });

            let status = match self.converter.convert(&self.settings, input, &self.cancel) {
                Ok(Outcome::Written { output, bytes }) => {
                    self.bytes_written.fetch_add(bytes, Ordering::Relaxed);
                    JobStatus::Done { output, bytes }
                }
                Ok(Outcome::Skipped { output }) => JobStatus::Skipped { output },
                Err(ConvertError::Cancelled) => {
                    // Put the job back to queued so a re-run picks it up, and
                    // stop pulling more work.
                    self.send(JobUpdate {
                        index: *index,
                        status: JobStatus::Queued,
                    });
                    break;
                }
                Err(err) => JobStatus::Failed {
                    command: match &err {
                        ConvertError::VipsFailed { command, .. } => Some(command.clone()),
                        _ => None,
                    },
                    message: err.to_string(),
                },
            };

            self.send(JobUpdate {
                index: *index,
                status,
            });
        }

        // Release ordering pairs with the Acquire load in `is_finished`, so a
        // UI thread that sees the count also sees every update we sent.
        self.finished_workers.fetch_add(1, Ordering::Release);
        if let Some(ctx) = &self.repaint {
            ctx.request_repaint();
        }
    }

    /// Send an update and wake the UI.
    ///
    /// A send error means the receiver is gone, which only happens if the app
    /// dropped the runner. There is nothing useful to do about it.
    fn send(&self, update: JobUpdate) {
        let _ = self.tx.send(update);
        if let Some(ctx) = &self.repaint {
            ctx.request_repaint();
        }
    }
}

/// Sensible parallelism for this machine.
///
/// Half the cores, because each worker spawns a `vips` that is itself
/// multi-threaded. Running one worker per core with a full libvips thread pool
/// inside each is markedly slower than this on every machine I have measured.
pub fn default_worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(2)
}

/// Thread count to give each child libvips.
///
/// Two keeps a single image's decode and encode overlapping without letting N
/// children fight over every core.
pub const DEFAULT_VIPS_CONCURRENCY: usize = 2;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    /// A converter that does no work, for testing the pool itself.
    struct StubConverter {
        /// Outcome per input file name.
        outcomes: Mutex<std::collections::HashMap<String, StubOutcome>>,
        /// How long each conversion pretends to take.
        duration: Duration,
        /// Inputs seen, in completion order.
        seen: Mutex<Vec<String>>,
        /// Set when a conversion observed the cancel flag mid-run.
        observed_cancel: AtomicBool,
    }

    #[derive(Clone)]
    enum StubOutcome {
        Ok,
        Skip,
        Fail(String),
    }

    impl StubConverter {
        fn new(duration: Duration) -> Self {
            Self {
                outcomes: Mutex::new(std::collections::HashMap::new()),
                duration,
                seen: Mutex::new(Vec::new()),
                observed_cancel: AtomicBool::new(false),
            }
        }

        fn with_outcome(self, name: &str, outcome: StubOutcome) -> Self {
            self.outcomes
                .lock()
                .unwrap()
                .insert(name.to_owned(), outcome);
            self
        }

        fn seen_count(&self) -> usize {
            self.seen.lock().unwrap().len()
        }
    }

    impl Converter for StubConverter {
        fn convert(
            &self,
            _settings: &ConversionSettings,
            input: &std::path::Path,
            cancel: &AtomicBool,
        ) -> Result<Outcome, ConvertError> {
            let name = input
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();

            // Simulate work in small slices so cancellation is observable.
            let slice = Duration::from_millis(5);
            let mut elapsed = Duration::ZERO;
            while elapsed < self.duration {
                if cancel.load(Ordering::Relaxed) {
                    self.observed_cancel.store(true, Ordering::Relaxed);
                    return Err(ConvertError::Cancelled);
                }
                std::thread::sleep(slice);
                elapsed += slice;
            }

            self.seen.lock().unwrap().push(name.clone());

            let outcome = self
                .outcomes
                .lock()
                .unwrap()
                .get(&name)
                .cloned()
                .unwrap_or(StubOutcome::Ok);

            match outcome {
                StubOutcome::Ok => Ok(Outcome::Written {
                    output: input.with_extension("webp"),
                    bytes: 100,
                }),
                StubOutcome::Skip => Ok(Outcome::Skipped {
                    output: input.with_extension("webp"),
                }),
                StubOutcome::Fail(message) => Err(ConvertError::VipsFailed {
                    message,
                    command: "vips copy ...".into(),
                }),
            }
        }
    }

    fn jobs(count: usize) -> Vec<(usize, PathBuf)> {
        (0..count)
            .map(|i| (i, PathBuf::from(format!("file{i}.png"))))
            .collect()
    }

    /// Run to completion and collect every update.
    fn run_to_completion(
        converter: Arc<dyn Converter>,
        job_list: Vec<(usize, PathBuf)>,
        workers: usize,
    ) -> Vec<JobUpdate> {
        let runner = BatchRunner::start(
            converter,
            ConversionSettings::default(),
            job_list,
            workers,
            None,
        );

        let mut all = Vec::new();
        // Poll like the UI does, with a generous ceiling so a hang fails the
        // test rather than blocking the suite forever.
        for _ in 0..2_000 {
            all.extend(runner.drain());
            if runner.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(runner.is_finished(), "the pool should have finished");
        all.extend(runner.join());
        all
    }

    /// Final status per index, discarding the intermediate Running updates.
    fn final_statuses(updates: &[JobUpdate]) -> std::collections::HashMap<usize, JobStatus> {
        let mut map = std::collections::HashMap::new();
        for update in updates {
            // Later updates supersede earlier ones, which is exactly how the
            // UI applies them.
            map.insert(update.index, update.status.clone());
        }
        map
    }

    #[test]
    fn every_job_succeeds_when_the_converter_always_succeeds() {
        let stub = Arc::new(StubConverter::new(Duration::from_millis(5)));
        let updates = run_to_completion(Arc::clone(&stub) as Arc<dyn Converter>, jobs(8), 4);

        let finals = final_statuses(&updates);
        assert_eq!(finals.len(), 8, "every job should report a final status");
        for index in 0..8 {
            assert!(
                matches!(finals[&index], JobStatus::Done { .. }),
                "job {index} should be done, was {:?}",
                finals[&index]
            );
        }
        assert_eq!(stub.seen_count(), 8);
    }

    #[test]
    fn every_job_reports_running_before_its_final_status() {
        let stub = Arc::new(StubConverter::new(Duration::from_millis(5)));
        let updates = run_to_completion(stub as Arc<dyn Converter>, jobs(4), 2);

        for index in 0..4 {
            let for_index: Vec<&JobUpdate> =
                updates.iter().filter(|u| u.index == index).collect();
            assert!(
                for_index.len() >= 2,
                "job {index} should have a Running and a final update"
            );
            assert_eq!(
                for_index[0].status,
                JobStatus::Running,
                "the first update for a job should be Running"
            );
            assert!(
                for_index.last().unwrap().status.is_finished(),
                "the last update for a job should be terminal"
            );
        }
    }

    #[test]
    fn a_mixture_of_outcomes_is_reported_per_job() {
        let stub = Arc::new(
            StubConverter::new(Duration::from_millis(5))
                .with_outcome("file1.png", StubOutcome::Fail("boom".into()))
                .with_outcome("file2.png", StubOutcome::Skip),
        );
        let updates = run_to_completion(stub as Arc<dyn Converter>, jobs(4), 2);
        let finals = final_statuses(&updates);

        assert!(matches!(finals[&0], JobStatus::Done { .. }));
        assert!(
            matches!(&finals[&1], JobStatus::Failed { message, .. } if message == "boom"),
            "got {:?}",
            finals[&1]
        );
        assert!(matches!(finals[&2], JobStatus::Skipped { .. }));
        assert!(matches!(finals[&3], JobStatus::Done { .. }));
    }

    #[test]
    fn a_failure_carries_the_command_for_the_copy_error_action() {
        let stub = Arc::new(
            StubConverter::new(Duration::from_millis(1))
                .with_outcome("file0.png", StubOutcome::Fail("bad".into())),
        );
        let updates = run_to_completion(stub as Arc<dyn Converter>, jobs(1), 1);
        let finals = final_statuses(&updates);

        let JobStatus::Failed { command, .. } = &finals[&0] else {
            panic!("expected a failure, got {:?}", finals[&0]);
        };
        assert!(
            command.as_deref().is_some_and(|c| c.contains("vips")),
            "the failing command should be reported: {command:?}"
        );
    }

    #[test]
    fn one_failure_does_not_stop_the_other_jobs() {
        let stub = Arc::new(
            StubConverter::new(Duration::from_millis(2))
                .with_outcome("file0.png", StubOutcome::Fail("first fails".into())),
        );
        let updates = run_to_completion(Arc::clone(&stub) as Arc<dyn Converter>, jobs(6), 2);
        let finals = final_statuses(&updates);

        assert!(matches!(finals[&0], JobStatus::Failed { .. }));
        for index in 1..6 {
            assert!(
                matches!(finals[&index], JobStatus::Done { .. }),
                "job {index} should still have run"
            );
        }
    }

    #[test]
    fn cancelling_stops_the_batch_early_and_requeues_the_interrupted_job() {
        // Long jobs, so cancel definitely lands mid-batch.
        let stub = Arc::new(StubConverter::new(Duration::from_millis(200)));
        let runner = BatchRunner::start(
            Arc::clone(&stub) as Arc<dyn Converter>,
            ConversionSettings::default(),
            jobs(20),
            2,
            None,
        );

        // Let a couple of jobs get going, then pull the plug.
        std::thread::sleep(Duration::from_millis(60));
        runner.cancel();
        assert!(runner.is_cancelled());

        let cancel_requested = std::time::Instant::now();
        let mut updates = Vec::new();
        for _ in 0..1_000 {
            updates.extend(runner.drain());
            if runner.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let stop_time = cancel_requested.elapsed();
        assert!(
            runner.is_finished(),
            "the pool should shut down after a cancel"
        );
        updates.extend(runner.join());

        // Promptness: 20 jobs at 200ms over 2 workers would be ~2s. Stopping
        // must take a fraction of that.
        assert!(
            stop_time < Duration::from_millis(500),
            "cancel took {stop_time:?}, which is not prompt"
        );

        assert!(
            stub.observed_cancel.load(Ordering::Relaxed),
            "the in-flight conversion should have seen the cancel flag"
        );

        // Far fewer than all 20 should have completed.
        assert!(
            stub.seen_count() < 20,
            "cancel should have prevented most jobs from running, {} ran",
            stub.seen_count()
        );

        // The interrupted job goes back to Queued so a re-run retries it,
        // rather than being left stuck showing a spinner.
        let finals = final_statuses(&updates);
        let requeued = finals
            .values()
            .filter(|s| **s == JobStatus::Queued)
            .count();
        assert!(
            requeued >= 1,
            "the interrupted job should be requeued, statuses: {finals:?}"
        );
        assert!(
            !finals.values().any(|s| *s == JobStatus::Running),
            "no job should be left showing Running after a cancel: {finals:?}"
        );
    }

    #[test]
    fn cancelling_before_any_work_starts_runs_nothing() {
        let stub = Arc::new(StubConverter::new(Duration::from_millis(50)));
        let runner = BatchRunner::start(
            Arc::clone(&stub) as Arc<dyn Converter>,
            ConversionSettings::default(),
            jobs(10),
            2,
            None,
        );
        runner.cancel();

        for _ in 0..1_000 {
            if runner.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(runner.is_finished());
        runner.join();

        // A worker may already have claimed a job before the flag was seen, so
        // allow a small number rather than insisting on exactly zero.
        assert!(
            stub.seen_count() <= 2,
            "an immediate cancel should run almost nothing, {} ran",
            stub.seen_count()
        );
    }

    #[test]
    fn parallel_workers_are_actually_faster_than_one() {
        // Six jobs at 60ms: ~360ms serially, ~120ms across three workers.
        let serial_stub = Arc::new(StubConverter::new(Duration::from_millis(60)));
        let start = std::time::Instant::now();
        run_to_completion(serial_stub as Arc<dyn Converter>, jobs(6), 1);
        let serial = start.elapsed();

        let parallel_stub = Arc::new(StubConverter::new(Duration::from_millis(60)));
        let start = std::time::Instant::now();
        run_to_completion(parallel_stub as Arc<dyn Converter>, jobs(6), 3);
        let parallel = start.elapsed();

        assert!(
            parallel < serial,
            "three workers ({parallel:?}) should beat one ({serial:?})"
        );
    }

    #[test]
    fn each_job_is_handed_to_exactly_one_worker() {
        // With many workers and many jobs, the shared counter must not hand the
        // same index out twice or skip any.
        let stub = Arc::new(StubConverter::new(Duration::from_millis(1)));
        let updates = run_to_completion(Arc::clone(&stub) as Arc<dyn Converter>, jobs(50), 8);

        let mut done_counts: std::collections::HashMap<usize, usize> =
            std::collections::HashMap::new();
        for update in &updates {
            if update.status.is_finished() {
                *done_counts.entry(update.index).or_default() += 1;
            }
        }

        assert_eq!(done_counts.len(), 50, "every index should be accounted for");
        for (index, count) in done_counts {
            assert_eq!(count, 1, "job {index} finished {count} times");
        }
        assert_eq!(stub.seen_count(), 50, "no job should run twice");
    }

    #[test]
    fn more_workers_than_jobs_is_harmless() {
        let stub = Arc::new(StubConverter::new(Duration::from_millis(2)));
        let updates = run_to_completion(Arc::clone(&stub) as Arc<dyn Converter>, jobs(2), 16);
        let finals = final_statuses(&updates);
        assert_eq!(finals.len(), 2);
        assert_eq!(stub.seen_count(), 2);
    }

    #[test]
    fn an_empty_batch_finishes_immediately() {
        let stub = Arc::new(StubConverter::new(Duration::from_millis(10)));
        let runner = BatchRunner::start(
            stub as Arc<dyn Converter>,
            ConversionSettings::default(),
            Vec::new(),
            4,
            None,
        );
        for _ in 0..500 {
            if runner.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(runner.is_finished());
        assert_eq!(runner.total(), 0);
        assert!(runner.join().is_empty());
    }

    #[test]
    fn written_bytes_are_accumulated_across_workers() {
        let stub = Arc::new(StubConverter::new(Duration::from_millis(1)));
        let runner = BatchRunner::start(
            stub as Arc<dyn Converter>,
            ConversionSettings::default(),
            jobs(5),
            3,
            None,
        );
        for _ in 0..1_000 {
            if runner.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        runner.drain();
        // The stub reports 100 bytes per file.
        assert_eq!(runner.bytes_written(), 500);
        runner.join();
    }

    #[test]
    fn dropping_a_running_batch_stops_it_rather_than_leaking_threads() {
        let stub = Arc::new(StubConverter::new(Duration::from_millis(100)));
        {
            let _runner = BatchRunner::start(
                Arc::clone(&stub) as Arc<dyn Converter>,
                ConversionSettings::default(),
                jobs(40),
                2,
                None,
            );
            std::thread::sleep(Duration::from_millis(30));
            // Dropping here must cancel and join, not detach.
        }
        // If Drop had not joined, jobs would keep completing after this point.
        let count_at_drop = stub.seen_count();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            stub.seen_count(),
            count_at_drop,
            "no work should continue after the runner is dropped"
        );
    }

    #[test]
    fn default_worker_count_is_at_least_one() {
        let n = default_worker_count();
        assert!(n >= 1, "must always have at least one worker");
        // Half the cores, so never more than the machine has.
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        assert!(n <= cores.max(1), "{n} workers for {cores} cores");
    }
}
