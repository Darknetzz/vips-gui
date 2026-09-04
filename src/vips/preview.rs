//! Rendering previews of a source image and of what it will look like once
//! converted, plus a real prediction of the output file size.
//!
//! The result preview is a round trip: encode the image at the settings the
//! user has actually chosen, measure the file that comes out, then shrink that
//! file to something displayable. Encoding at full size is the only way to get
//! an honest size prediction, and it is also what makes the preview worth
//! looking at, since compression artefacts depend on the real resolution.
//!
//! Previews are produced on a background thread, one at a time, with newer
//! requests replacing older ones. See [`PreviewEngine`].

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::settings::ConversionSettings;
use crate::vips::{VipsInstall, command};

/// How often to check for cancellation while a preview subprocess runs.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Longest edge of a rendered preview, in pixels.
///
/// Generous enough to judge compression artefacts when the panel is wide,
/// small enough that decoding and uploading it is imperceptible.
pub const PREVIEW_MAX_SIDE: u32 = 720;

/// What went wrong producing a preview.
#[derive(Debug, Clone, thiserror::Error)]
pub enum PreviewError {
    #[error("could not start vips: {reason}")]
    SpawnFailed { reason: String },

    #[error("{message}")]
    VipsFailed { message: String },

    #[error("vips produced no image data")]
    NoOutput,

    #[error("superseded")]
    Superseded,
}

/// A rendered PNG, ready to be decoded into a texture.
#[derive(Debug, Clone)]
pub struct RenderedImage {
    /// PNG-encoded preview bytes.
    pub png: Vec<u8>,
}

/// Everything a preview produces.
#[derive(Debug, Clone)]
pub struct Preview {
    /// The source as it is now.
    pub source: RenderedImage,
    /// Pixel dimensions of the source, when they could be read.
    pub source_dimensions: Option<(u32, u32)>,
    /// Size of the source file on disk.
    pub source_bytes: u64,
    /// The converted result, and how big the real output would be.
    pub result: RenderedImage,
    /// Exact size of the encoded output, measured rather than estimated.
    pub predicted_bytes: u64,
    /// Pixel dimensions the output would have, when they could be read.
    pub result_dimensions: Option<(u32, u32)>,
}

impl Preview {
    /// Size change as a percentage. Negative means smaller.
    pub fn size_delta_percent(&self) -> Option<f64> {
        if self.source_bytes == 0 {
            return None;
        }
        let ratio = self.predicted_bytes as f64 / self.source_bytes as f64;
        Some((ratio - 1.0) * 100.0)
    }
}

/// Whether this libvips can write an image to stdout.
///
/// Every libvips I have tested can, using the leading-dot filename convention
/// (`vips black .png 8 8`), but it is cheap to confirm and the fallback of
/// going via a temporary file is only a few lines.
pub fn probe_stdout_support(install: &VipsInstall) -> bool {
    // `black` generates an image from nothing, so this needs no input file.
    let mut command = install.command();
    command
        .args(["black", ".png", "8", "8"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    match command.output() {
        Ok(output) => output.status.success() && is_png(&output.stdout),
        Err(_) => false,
    }
}

/// PNG signature check, so a probe cannot be fooled by an error message.
fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
}

/// Produce both previews for one file at one set of settings.
///
/// `scratch` is a directory for the intermediate encoded file. It is created if
/// needed and the file inside it is always removed before returning.
pub fn render(
    install: &VipsInstall,
    settings: &ConversionSettings,
    input: &Path,
    scratch: &Path,
    use_stdout: bool,
    cancel: &AtomicBool,
) -> Result<Preview, PreviewError> {
    let source_bytes = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    let source_dimensions = read_dimensions(install, input);

    // 1. The source, shrunk for display. `thumbnail` shrinks on load, so a
    //    60 megapixel source is not fully decoded just to show a thumbnail.
    let source_png = render_thumbnail(install, input, use_stdout, scratch, cancel)?;

    check_cancelled(cancel)?;

    // 2. The real encode, at the real settings and the real resolution. This
    //    is what makes the predicted size trustworthy.
    if !scratch.is_dir() {
        std::fs::create_dir_all(scratch).map_err(|e| PreviewError::SpawnFailed {
            reason: format!("could not create {}: {e}", scratch.display()),
        })?;
    }
    let encoded = scratch.join(format!("preview-encode.{}", settings.format.extension()));
    let _ = std::fs::remove_file(&encoded);

    let args = command::build(settings, input, &encoded, install.version);
    let encode_result = run(install, &args.0, false, cancel);

    // Whatever happens next, do not leave the intermediate file behind.
    let cleanup = Cleanup(&encoded);

    encode_result?;

    let predicted_bytes = std::fs::metadata(&encoded)
        .map(|m| m.len())
        .map_err(|_| PreviewError::NoOutput)?;
    let result_dimensions = read_dimensions(install, &encoded);

    check_cancelled(cancel)?;

    // 3. The encoded output, shrunk for display. Rendering from the encoded
    //    file rather than the source is the point: this shows the artefacts the
    //    chosen settings actually produce.
    let result_png = render_thumbnail(install, &encoded, use_stdout, scratch, cancel)?;

    drop(cleanup);

    Ok(Preview {
        source: RenderedImage { png: source_png },
        source_dimensions,
        source_bytes,
        result: RenderedImage { png: result_png },
        predicted_bytes,
        result_dimensions,
    })
}

/// Removes a file when dropped, so early returns cannot leak it.
struct Cleanup<'a>(&'a Path);

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}

/// Shrink an image to a displayable PNG.
fn render_thumbnail(
    install: &VipsInstall,
    image: &Path,
    use_stdout: bool,
    scratch: &Path,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, PreviewError> {
    let side = PREVIEW_MAX_SIDE.to_string();

    if use_stdout {
        // `.png` as the output filename means "write to stdout".
        let args: Vec<std::ffi::OsString> = vec![
            "thumbnail".into(),
            image.into(),
            ".png".into(),
            side.clone().into(),
            "--height".into(),
            side.into(),
            "--size".into(),
            "down".into(),
        ];
        let bytes = run(install, &args, true, cancel)?;
        if !is_png(&bytes) {
            return Err(PreviewError::NoOutput);
        }
        return Ok(bytes);
    }

    // Fallback for a libvips that will not write to stdout.
    if !scratch.is_dir() {
        std::fs::create_dir_all(scratch).map_err(|e| PreviewError::SpawnFailed {
            reason: format!("could not create {}: {e}", scratch.display()),
        })?;
    }
    let temp = scratch.join("preview-display.png");
    let _ = std::fs::remove_file(&temp);
    let args: Vec<std::ffi::OsString> = vec![
        "thumbnail".into(),
        image.into(),
        (&temp).into(),
        side.clone().into(),
        "--height".into(),
        side.into(),
        "--size".into(),
        "down".into(),
    ];
    let cleanup = Cleanup(&temp);
    run(install, &args, false, cancel)?;
    let bytes = std::fs::read(&temp).map_err(|_| PreviewError::NoOutput)?;
    drop(cleanup);

    if !is_png(&bytes) {
        return Err(PreviewError::NoOutput);
    }
    Ok(bytes)
}

/// Run a vips command, optionally capturing stdout, abandoning it on cancel.
fn run(
    install: &VipsInstall,
    args: &[std::ffi::OsString],
    capture_stdout: bool,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, PreviewError> {
    check_cancelled(cancel)?;

    let mut command = install.command();
    command
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(if capture_stdout {
            Stdio::piped()
        } else {
            Stdio::null()
        });

    let mut child = command.spawn().map_err(|e| PreviewError::SpawnFailed {
        reason: e.to_string(),
    })?;

    // Drain stdout as it arrives. A preview PNG is far larger than the pipe
    // buffer, so waiting until exit to read it would deadlock.
    let stdout_reader = capture_stdout.then(|| child.stdout.take()).flatten();
    let collected = stdout_reader.map(|mut out| {
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = out.read_to_end(&mut buffer);
            buffer
        })
    });

    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(handle) = collected {
                let _ = handle.join();
            }
            return Err(PreviewError::Superseded);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stderr = String::new();
                if let Some(mut err) = child.stderr.take() {
                    let _ = err.read_to_string(&mut stderr);
                }
                let stdout = match collected {
                    Some(handle) => handle.join().unwrap_or_default(),
                    None => Vec::new(),
                };

                if !status.success() {
                    return Err(PreviewError::VipsFailed {
                        message: last_line(&stderr).unwrap_or("vips failed").to_owned(),
                    });
                }
                return Ok(stdout);
            }
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(e) => {
                return Err(PreviewError::SpawnFailed {
                    reason: e.to_string(),
                });
            }
        }
    }
}

fn check_cancelled(cancel: &AtomicBool) -> Result<(), PreviewError> {
    if cancel.load(Ordering::Relaxed) {
        Err(PreviewError::Superseded)
    } else {
        Ok(())
    }
}

/// The most specific line of a libvips error message.
fn last_line(text: &str) -> Option<&str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .next_back()
}

/// Read an image's pixel dimensions, best effort.
///
/// Uses `vipsheader`, which ships beside `vips` in every distribution. When it
/// is missing the dimensions are simply not shown, which is a much better
/// outcome than failing the whole preview over a cosmetic detail.
fn read_dimensions(install: &VipsInstall, image: &Path) -> Option<(u32, u32)> {
    let exe_name = if cfg!(windows) {
        "vipsheader.exe"
    } else {
        "vipsheader"
    };
    let exe = install.exe.parent()?.join(exe_name);
    if !exe.is_file() {
        return None;
    }

    let read_field = |field: &str| -> Option<u32> {
        let mut command = std::process::Command::new(&exe);
        crate::vips::hide_console(&mut command);
        let output = command
            .args([std::ffi::OsStr::new("-f"), std::ffi::OsStr::new(field), image.as_os_str()])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout).trim().parse().ok()
    };

    Some((read_field("width")?, read_field("height")?))
}

// --- asynchronous engine --------------------------------------------------

/// One preview request.
#[derive(Debug, Clone)]
pub struct Request {
    pub generation: u64,
    pub input: PathBuf,
    pub settings: ConversionSettings,
}

/// A finished request, successful or not.
#[derive(Debug)]
pub struct Outcome {
    pub generation: u64,
    pub input: PathBuf,
    pub result: Result<Preview, PreviewError>,
}

/// A single-slot request mailbox where the newest request wins.
///
/// Holding only the latest request is the whole point: while one preview is
/// rendering, a user dragging a quality slider can generate dozens more, and
/// every one but the last is worthless by the time a worker is free.
struct RequestSlot {
    pending: Mutex<SlotState>,
    ready: Condvar,
}

#[derive(Default)]
struct SlotState {
    request: Option<Request>,
    shutdown: bool,
}

impl RequestSlot {
    fn new() -> Self {
        Self {
            pending: Mutex::new(SlotState::default()),
            ready: Condvar::new(),
        }
    }

    /// Store a request, discarding any that had not started yet.
    ///
    /// Returns true when an unstarted request was displaced.
    fn put(&self, request: Request) -> bool {
        let mut state = self.pending.lock().expect("slot mutex poisoned");
        let displaced = state.request.replace(request).is_some();
        self.ready.notify_one();
        displaced
    }

    /// Wait for a request. `None` means the engine is shutting down.
    fn take_blocking(&self) -> Option<Request> {
        let mut state = self.pending.lock().expect("slot mutex poisoned");
        loop {
            if state.shutdown {
                return None;
            }
            if let Some(request) = state.request.take() {
                return Some(request);
            }
            state = self.ready.wait(state).expect("slot mutex poisoned");
        }
    }

    fn shutdown(&self) {
        let mut state = self.pending.lock().expect("slot mutex poisoned");
        state.shutdown = true;
        state.request = None;
        self.ready.notify_all();
    }
}

/// Renders previews on a background thread, newest request first.
pub struct PreviewEngine {
    slot: Arc<RequestSlot>,
    /// Cancel flag for whatever is rendering right now, so a superseding
    /// request can kill an expensive in-flight encode instead of waiting.
    in_flight: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    outcomes: Receiver<Outcome>,
    handle: Option<JoinHandle<()>>,
    /// Directory for intermediate files, removed on shutdown.
    scratch: PathBuf,
    next_generation: u64,
}

impl PreviewEngine {
    /// Start the engine.
    ///
    /// `repaint` is woken when a preview finishes.
    pub fn new(install: VipsInstall, use_stdout: bool, repaint: Option<egui::Context>) -> Self {
        let slot = Arc::new(RequestSlot::new());
        let in_flight: Arc<Mutex<Option<Arc<AtomicBool>>>> = Arc::new(Mutex::new(None));
        let (tx, outcomes) = channel();

        // Scratch directory keyed on both the process and this engine instance.
        // The process id keeps two copies of the app apart; the instance
        // counter keeps two engines within one process apart, which matters
        // because dropping an engine deletes its directory.
        static INSTANCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let instance = INSTANCE.fetch_add(1, Ordering::Relaxed);
        let scratch = std::env::temp_dir().join(format!(
            "vips-gui-preview-{}-{instance}",
            std::process::id()
        ));

        let handle = std::thread::spawn({
            let slot = Arc::clone(&slot);
            let in_flight = Arc::clone(&in_flight);
            let scratch = scratch.clone();
            move || worker_loop(install, use_stdout, slot, in_flight, scratch, tx, repaint)
        });

        Self {
            slot,
            in_flight,
            outcomes,
            handle: Some(handle),
            scratch,
            next_generation: 0,
        }
    }

    /// Ask for a preview, superseding anything queued or running.
    ///
    /// Returns the generation number assigned, so a caller can ignore outcomes
    /// that arrive out of order.
    pub fn request(&mut self, input: PathBuf, settings: ConversionSettings) -> u64 {
        self.next_generation += 1;
        let generation = self.next_generation;

        // Kill the in-flight render first: its result is already stale.
        if let Some(cancel) = self.in_flight.lock().expect("mutex poisoned").as_ref() {
            cancel.store(true, Ordering::Relaxed);
        }

        self.slot.put(Request {
            generation,
            input,
            settings,
        });

        generation
    }

    /// Collect any finished previews.
    pub fn drain(&self) -> Vec<Outcome> {
        let mut out = Vec::new();
        loop {
            match self.outcomes.try_recv() {
                Ok(outcome) => out.push(outcome),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        out
    }

    /// The most recent generation handed out.
    pub fn current_generation(&self) -> u64 {
        self.next_generation
    }

    /// Where this engine keeps its intermediate files.
    ///
    /// Unique per engine instance, and removed when the engine is dropped.
    pub fn scratch_dir(&self) -> &Path {
        &self.scratch
    }
}

impl Drop for PreviewEngine {
    fn drop(&mut self) {
        // Stop the in-flight render, wake the thread, and clean up after it.
        if let Some(cancel) = self.in_flight.lock().expect("mutex poisoned").as_ref() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.slot.shutdown();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        // Intermediate files are removed as they are used; this catches
        // anything left by an abrupt shutdown.
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

/// The background thread body.
fn worker_loop(
    install: VipsInstall,
    use_stdout: bool,
    slot: Arc<RequestSlot>,
    in_flight: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    scratch: PathBuf,
    tx: Sender<Outcome>,
    repaint: Option<egui::Context>,
) {
    while let Some(request) = slot.take_blocking() {
        let cancel = Arc::new(AtomicBool::new(false));
        *in_flight.lock().expect("mutex poisoned") = Some(Arc::clone(&cancel));

        let result = render(
            &install,
            &request.settings,
            &request.input,
            &scratch,
            use_stdout,
            &cancel,
        );

        *in_flight.lock().expect("mutex poisoned") = None;

        // A superseded render has nothing worth reporting; the replacement is
        // already queued.
        if matches!(result, Err(PreviewError::Superseded)) {
            continue;
        }

        if tx
            .send(Outcome {
                generation: request.generation,
                input: request.input,
                result,
            })
            .is_err()
        {
            // The engine is gone.
            break;
        }
        if let Some(ctx) = &repaint {
            ctx.request_repaint();
        }
    }
}

// --- debouncing -----------------------------------------------------------

/// Collapses a burst of changes into one action after things settle.
///
/// Dragging a quality slider changes the settings on every frame. Without this,
/// each frame would kick off a full encode of the image.
#[derive(Debug, Clone)]
pub struct Debouncer {
    delay: Duration,
    pending_since: Option<Instant>,
}

impl Debouncer {
    pub fn new(delay: Duration) -> Self {
        Self {
            delay,
            pending_since: None,
        }
    }

    /// Note that something changed, restarting the timer.
    pub fn touch(&mut self, now: Instant) {
        self.pending_since = Some(now);
    }

    /// True when a change is waiting to be acted on.
    pub fn is_pending(&self) -> bool {
        self.pending_since.is_some()
    }

    /// Consume the pending change if the delay has elapsed.
    pub fn take_if_ready(&mut self, now: Instant) -> bool {
        match self.pending_since {
            Some(since) if now.duration_since(since) >= self.delay => {
                self.pending_since = None;
                true
            }
            _ => false,
        }
    }

    /// How long until the pending change fires, if one is waiting.
    pub fn time_remaining(&self, now: Instant) -> Option<Duration> {
        let since = self.pending_since?;
        let elapsed = now.duration_since(since);
        Some(self.delay.saturating_sub(elapsed))
    }

    /// Forget any pending change.
    pub fn clear(&mut self) {
        self.pending_since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(generation: u64, name: &str) -> Request {
        Request {
            generation,
            input: PathBuf::from(name),
            settings: ConversionSettings::default(),
        }
    }

    #[test]
    fn png_signature_detection_rejects_non_png_data() {
        assert!(is_png(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00]));
        assert!(!is_png(b""));
        assert!(!is_png(b"not a png at all"));
        // A truncated signature is not a PNG.
        assert!(!is_png(&[0x89, b'P', b'N']));
        // A JPEG must not be mistaken for one.
        assert!(!is_png(&[0xff, 0xd8, 0xff, 0xe0]));
    }

    #[test]
    fn the_newest_request_replaces_an_unstarted_one() {
        let slot = RequestSlot::new();

        assert!(
            !slot.put(request(1, "a.png")),
            "the first request displaces nothing"
        );
        assert!(
            slot.put(request(2, "b.png")),
            "the second should displace the first"
        );
        assert!(slot.put(request(3, "c.png")));

        // Only the newest survives: the two older ones were never worth doing.
        let taken = slot.take_blocking().expect("a request should be waiting");
        assert_eq!(taken.generation, 3);
        assert_eq!(taken.input, PathBuf::from("c.png"));
    }

    #[test]
    fn taking_from_the_slot_leaves_it_empty() {
        let slot = RequestSlot::new();
        slot.put(request(1, "a.png"));
        assert_eq!(slot.take_blocking().unwrap().generation, 1);

        // Nothing left, so a shutdown is what unblocks the next take.
        slot.shutdown();
        assert!(slot.take_blocking().is_none());
    }

    #[test]
    fn shutdown_wakes_a_waiting_worker() {
        let slot = Arc::new(RequestSlot::new());
        let worker_slot = Arc::clone(&slot);

        let handle = std::thread::spawn(move || worker_slot.take_blocking());
        // Give the thread time to block on the condvar.
        std::thread::sleep(Duration::from_millis(50));
        slot.shutdown();

        let taken = handle.join().expect("thread should not panic");
        assert!(taken.is_none(), "shutdown should return None, not a request");
    }

    #[test]
    fn shutdown_discards_a_pending_request() {
        let slot = RequestSlot::new();
        slot.put(request(1, "a.png"));
        slot.shutdown();
        assert!(
            slot.take_blocking().is_none(),
            "a pending request should not be handed out after shutdown"
        );
    }

    #[test]
    fn a_blocked_worker_receives_a_request_posted_later() {
        let slot = Arc::new(RequestSlot::new());
        let worker_slot = Arc::clone(&slot);

        let handle = std::thread::spawn(move || worker_slot.take_blocking());
        std::thread::sleep(Duration::from_millis(30));
        slot.put(request(7, "later.png"));

        let taken = handle.join().unwrap().expect("should receive the request");
        assert_eq!(taken.generation, 7);
    }

    #[test]
    fn debouncer_waits_for_the_delay_before_firing() {
        let mut d = Debouncer::new(Duration::from_millis(300));
        let t0 = Instant::now();

        assert!(!d.is_pending());
        assert!(!d.take_if_ready(t0), "nothing pending, nothing to fire");

        d.touch(t0);
        assert!(d.is_pending());
        assert!(
            !d.take_if_ready(t0 + Duration::from_millis(299)),
            "should not fire one millisecond early"
        );
        assert!(
            d.take_if_ready(t0 + Duration::from_millis(300)),
            "should fire once the delay has elapsed"
        );
        assert!(!d.is_pending(), "firing consumes the pending change");
        assert!(
            !d.take_if_ready(t0 + Duration::from_secs(10)),
            "should not fire twice for one change"
        );
    }

    #[test]
    fn each_change_restarts_the_debounce_timer() {
        let mut d = Debouncer::new(Duration::from_millis(300));
        let t0 = Instant::now();

        // A slider being dragged: a change every 50ms.
        d.touch(t0);
        for step in 1..=5 {
            let now = t0 + Duration::from_millis(50 * step);
            assert!(
                !d.take_if_ready(now),
                "should not fire while changes keep arriving (step {step})"
            );
            d.touch(now);
        }

        // 250ms in, the last touch was at 250ms, so it fires at 550ms.
        let last_touch = t0 + Duration::from_millis(250);
        assert!(!d.take_if_ready(last_touch + Duration::from_millis(299)));
        assert!(d.take_if_ready(last_touch + Duration::from_millis(300)));
    }

    #[test]
    fn debouncer_reports_time_remaining_for_scheduling_a_repaint() {
        let mut d = Debouncer::new(Duration::from_millis(300));
        let t0 = Instant::now();

        assert_eq!(d.time_remaining(t0), None, "nothing pending");

        d.touch(t0);
        assert_eq!(
            d.time_remaining(t0),
            Some(Duration::from_millis(300)),
            "the full delay remains immediately after a change"
        );
        assert_eq!(
            d.time_remaining(t0 + Duration::from_millis(100)),
            Some(Duration::from_millis(200))
        );
        // Never negative, so it can be handed straight to request_repaint_after.
        assert_eq!(
            d.time_remaining(t0 + Duration::from_secs(5)),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn clearing_the_debouncer_cancels_the_pending_change() {
        let mut d = Debouncer::new(Duration::from_millis(100));
        let t0 = Instant::now();
        d.touch(t0);
        d.clear();
        assert!(!d.is_pending());
        assert!(!d.take_if_ready(t0 + Duration::from_secs(1)));
    }

    #[test]
    fn size_delta_is_negative_when_the_output_is_smaller() {
        let preview = Preview {
            source: RenderedImage { png: Vec::new() },
            source_dimensions: Some((100, 100)),
            source_bytes: 1000,
            result: RenderedImage { png: Vec::new() },
            predicted_bytes: 250,
            result_dimensions: Some((100, 100)),
        };
        let delta = preview.size_delta_percent().expect("a delta");
        assert!((delta - (-75.0)).abs() < 1e-9, "got {delta}");
    }

    #[test]
    fn size_delta_is_positive_when_the_output_is_larger() {
        let preview = Preview {
            source: RenderedImage { png: Vec::new() },
            source_dimensions: None,
            source_bytes: 1000,
            result: RenderedImage { png: Vec::new() },
            predicted_bytes: 1500,
            result_dimensions: None,
        };
        let delta = preview.size_delta_percent().expect("a delta");
        assert!((delta - 50.0).abs() < 1e-9, "got {delta}");
    }

    #[test]
    fn size_delta_is_unavailable_for_a_zero_byte_source() {
        let preview = Preview {
            source: RenderedImage { png: Vec::new() },
            source_dimensions: None,
            source_bytes: 0,
            result: RenderedImage { png: Vec::new() },
            predicted_bytes: 100,
            result_dimensions: None,
        };
        assert_eq!(
            preview.size_delta_percent(),
            None,
            "no division by zero, no nonsense percentage"
        );
    }

    #[test]
    fn last_line_picks_the_most_specific_error() {
        // libvips prints general context first and the specific cause last.
        assert_eq!(
            last_line("vips: error\nVipsForeignSave: bad bitdepth\n"),
            Some("VipsForeignSave: bad bitdepth")
        );
        assert_eq!(last_line(""), None);
        assert_eq!(last_line("   \n\n  "), None);
        assert_eq!(last_line("only one"), Some("only one"));
    }
}
