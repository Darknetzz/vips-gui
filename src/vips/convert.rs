//! Running one conversion as a `vips` subprocess.
//!
//! Each conversion is its own process. That costs a few tens of milliseconds,
//! which is nothing next to an AVIF encode, and buys real isolation: a
//! malformed image that makes libvips abort takes down a child, not the GUI.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::settings::{ConversionSettings, OverwritePolicy};
use crate::vips::{VipsInstall, command};

/// How often to check whether a running conversion should be abandoned.
///
/// Short enough that Cancel feels immediate, long enough not to spin.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How a conversion ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Written successfully to this path.
    Written {
        output: PathBuf,
        /// Size of the written file in bytes.
        bytes: u64,
    },
    /// The output already existed and the policy said to leave it alone.
    Skipped { output: PathBuf },
}

/// Why a conversion failed.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ConvertError {
    #[error("{input} no longer exists")]
    InputMissing { input: String },

    #[error("could not create the output folder {dir}: {reason}")]
    OutputDirUnusable { dir: String, reason: String },

    #[error("could not start vips: {reason}")]
    SpawnFailed { reason: String },

    /// vips ran and reported a problem. `message` is its stderr, cleaned up.
    #[error("{message}")]
    VipsFailed {
        message: String,
        /// The command we ran, for the "copy error" affordance in the UI.
        command: String,
    },

    #[error("vips reported success but wrote no file to {output}")]
    OutputMissing { output: String },

    #[error("cancelled")]
    Cancelled,
}

/// Convert one file, without cancellation.
pub fn convert_one(
    install: &VipsInstall,
    settings: &ConversionSettings,
    input: &Path,
) -> Result<Outcome, ConvertError> {
    // A flag that is never set: the cancellable path with cancellation off.
    static NEVER: AtomicBool = AtomicBool::new(false);
    convert_one_cancellable(install, settings, input, &NEVER, 0)
}

/// Convert one file, aborting if `cancel` is set.
///
/// `vips_concurrency` caps the thread pool inside the child process. Pass 0 to
/// leave libvips to its own default. This matters when several conversions run
/// at once: N parallel children each spawning a full thread pool oversubscribes
/// the machine badly.
pub fn convert_one_cancellable(
    install: &VipsInstall,
    settings: &ConversionSettings,
    input: &Path,
    cancel: &AtomicBool,
    vips_concurrency: usize,
) -> Result<Outcome, ConvertError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(ConvertError::Cancelled);
    }

    if !input.is_file() {
        return Err(ConvertError::InputMissing {
            input: input.display().to_string(),
        });
    }

    let nominal = settings.output_path_for(input);

    // Resolve collisions before spawning, so we never hand vips a path we are
    // not prepared to write.
    let output = match resolve_collision(&nominal, settings.overwrite) {
        Collision::Proceed(path) => path,
        Collision::Skip => return Ok(Outcome::Skipped { output: nominal }),
    };

    if let Some(dir) = output.parent()
        && !dir.as_os_str().is_empty()
        && !dir.is_dir()
    {
        std::fs::create_dir_all(dir).map_err(|e| ConvertError::OutputDirUnusable {
            dir: dir.display().to_string(),
            reason: e.to_string(),
        })?;
    }

    let args = command::build(settings, input, &output, install.version);

    let mut command = install.command();
    command.args(&args.0);
    if let Some((name, value)) = concurrency_env(vips_concurrency) {
        command.env(name, value);
    }
    // stderr is captured for the error message; stdout is unused by these
    // subcommands but is piped so nothing leaks to a console.
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|e| ConvertError::SpawnFailed {
        reason: e.to_string(),
    })?;

    // Poll rather than block, so a cancel request can kill the child promptly.
    // vips writes at most a line or two to stderr, comfortably inside the pipe
    // buffer, so not draining it until exit cannot deadlock here.
    let result = loop {
        if cancel.load(Ordering::Relaxed) {
            // Best effort: if the child already exited, kill is a no-op.
            let _ = child.kill();
            let _ = child.wait();
            // Remove the partial file, so a cancelled batch does not leave
            // truncated images behind that look like successes.
            let _ = std::fs::remove_file(&output);
            return Err(ConvertError::Cancelled);
        }

        match child.try_wait() {
            Ok(Some(_)) => {
                break child
                    .wait_with_output()
                    .map_err(|e| ConvertError::SpawnFailed {
                        reason: e.to_string(),
                    })?;
            }
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(e) => {
                return Err(ConvertError::SpawnFailed {
                    reason: e.to_string(),
                });
            }
        }
    };

    if !result.status.success() {
        return Err(ConvertError::VipsFailed {
            message: clean_stderr(&result.stderr, result.status.code()),
            command: format!("{} {}", install.exe.display(), args.to_display_string()),
        });
    }

    // A zero exit code with no file is not something libvips does, but the
    // caller's progress reporting depends on the file being real, so check.
    let bytes = match std::fs::metadata(&output) {
        Ok(meta) => meta.len(),
        Err(_) => {
            return Err(ConvertError::OutputMissing {
                output: output.display().to_string(),
            });
        }
    };

    Ok(Outcome::Written { output, bytes })
}

/// What to do about an existing output file.
enum Collision {
    /// Write to this path.
    Proceed(PathBuf),
    /// Do nothing.
    Skip,
}

/// Apply the overwrite policy to a candidate output path.
fn resolve_collision(nominal: &Path, policy: OverwritePolicy) -> Collision {
    if !nominal.exists() {
        return Collision::Proceed(nominal.to_path_buf());
    }

    match policy {
        OverwritePolicy::Overwrite => Collision::Proceed(nominal.to_path_buf()),
        OverwritePolicy::Skip => Collision::Skip,
        OverwritePolicy::Rename => Collision::Proceed(next_free_name(nominal)),
    }
}

/// Find `name (1).ext`, `name (2).ext`, ... until one is free.
///
/// The bound stops a pathological directory from hanging the app; hitting it
/// means something is very wrong, so falling back to the nominal path (and
/// letting the write fail loudly) is better than looping forever.
pub fn next_free_name(nominal: &Path) -> PathBuf {
    const MAX_ATTEMPTS: u32 = 10_000;

    let parent = nominal.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = nominal
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = nominal
        .extension()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    for n in 1..=MAX_ATTEMPTS {
        let candidate_name = if ext.is_empty() {
            format!("{stem} ({n})")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        let candidate = parent.join(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
    }

    nominal.to_path_buf()
}

/// The environment variable that caps a child's libvips thread pool.
///
/// Returns `None` for 0, meaning "leave libvips to its own default", so the
/// variable is not set at all rather than being set to something meaningless.
pub fn concurrency_env(vips_concurrency: usize) -> Option<(&'static str, String)> {
    if vips_concurrency == 0 {
        None
    } else {
        Some(("VIPS_CONCURRENCY", vips_concurrency.to_string()))
    }
}

/// Turn libvips' stderr into something worth showing a user.
///
/// libvips messages look like `VipsForeignLoad: "x.jpg" is not a known file
/// format`. The class prefix is noise for a GUI user, so drop it, but keep the
/// message itself verbatim because it is genuinely informative.
fn clean_stderr(stderr: &[u8], exit_code: Option<i32>) -> String {
    let text = String::from_utf8_lossy(stderr);

    // Take the last non-empty line: when libvips chains errors, the most
    // specific one comes last.
    let line = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .next_back();

    match line {
        Some(line) => strip_vips_class_prefix(line).to_owned(),
        None => match exit_code {
            Some(code) => format!("vips exited with code {code} and no message"),
            None => "vips was terminated before it finished".to_owned(),
        },
    }
}

/// Remove a leading `SomeVipsClass: ` prefix, if present.
fn strip_vips_class_prefix(line: &str) -> &str {
    // Only strip when the prefix looks like a libvips class name: no spaces
    // before the colon, and it starts with an upper-case letter. That avoids
    // mangling messages that legitimately contain a colon, such as Windows
    // paths.
    if let Some((prefix, rest)) = line.split_once(": ")
        && !prefix.contains(' ')
        && prefix.starts_with(char::is_uppercase)
        && prefix.len() > 3
        // `C:` and friends are drive letters, not class names.
        && prefix.len() != 2
    {
        return rest.trim();
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{OutputDestination, OutputFormat};

    #[test]
    fn a_free_path_is_used_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.webp");
        match resolve_collision(&target, OverwritePolicy::Rename) {
            Collision::Proceed(p) => assert_eq!(p, target),
            Collision::Skip => panic!("should not skip a free path"),
        }
    }

    #[test]
    fn overwrite_policy_reuses_the_existing_path() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.webp");
        std::fs::write(&target, b"existing").unwrap();

        match resolve_collision(&target, OverwritePolicy::Overwrite) {
            Collision::Proceed(p) => assert_eq!(p, target),
            Collision::Skip => panic!("overwrite should proceed"),
        }
    }

    #[test]
    fn skip_policy_reports_a_skip() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.webp");
        std::fs::write(&target, b"existing").unwrap();

        assert!(matches!(
            resolve_collision(&target, OverwritePolicy::Skip),
            Collision::Skip
        ));
    }

    #[test]
    fn rename_policy_walks_the_numbered_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("photo.webp");
        std::fs::write(&target, b"first").unwrap();

        // First collision becomes "photo (1).webp".
        let first = match resolve_collision(&target, OverwritePolicy::Rename) {
            Collision::Proceed(p) => p,
            Collision::Skip => panic!("rename should proceed"),
        };
        assert_eq!(first.file_name().unwrap(), "photo (1).webp");

        // Once that exists too, the next is "photo (2).webp".
        std::fs::write(&first, b"second").unwrap();
        let second = match resolve_collision(&target, OverwritePolicy::Rename) {
            Collision::Proceed(p) => p,
            Collision::Skip => panic!("rename should proceed"),
        };
        assert_eq!(second.file_name().unwrap(), "photo (2).webp");

        // And it keeps counting.
        std::fs::write(&second, b"third").unwrap();
        let third = match resolve_collision(&target, OverwritePolicy::Rename) {
            Collision::Proceed(p) => p,
            Collision::Skip => panic!("rename should proceed"),
        };
        assert_eq!(third.file_name().unwrap(), "photo (3).webp");
    }

    #[test]
    fn rename_preserves_dots_inside_the_stem() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("my.photo.v2.webp");
        std::fs::write(&target, b"x").unwrap();

        let renamed = next_free_name(&target);
        assert_eq!(renamed.file_name().unwrap(), "my.photo.v2 (1).webp");
    }

    #[test]
    fn rename_handles_a_missing_extension() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("noext");
        std::fs::write(&target, b"x").unwrap();

        let renamed = next_free_name(&target);
        assert_eq!(renamed.file_name().unwrap(), "noext (1)");
    }

    #[test]
    fn a_missing_input_is_rejected_before_spawning_anything() {
        // No VipsInstall needed: the check happens first. Build a stand-in
        // that would fail loudly if it were ever executed.
        let install = VipsInstall {
            exe: std::path::PathBuf::from("this-should-never-run"),
            version: crate::vips::VipsVersion {
                major: 8,
                minor: 18,
                patch: 0,
            },
            capabilities: Default::default(),
            source: crate::vips::discovery::DiscoverySource::Path,
        };

        let err = convert_one(
            &install,
            &ConversionSettings::default(),
            Path::new("definitely-not-a-real-file.jpg"),
        )
        .expect_err("a missing input must fail");

        assert!(matches!(err, ConvertError::InputMissing { .. }), "{err:?}");
    }

    #[test]
    fn skip_policy_short_circuits_before_spawning() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.png");
        std::fs::write(&input, b"pretend png").unwrap();
        let existing = dir.path().join("in.webp");
        std::fs::write(&existing, b"already here").unwrap();

        let mut settings = ConversionSettings::default();
        settings.format = OutputFormat::Webp;
        settings.overwrite = OverwritePolicy::Skip;
        settings.destination = OutputDestination::SameAsSource;

        // A bogus exe proves no process was spawned: if it were, we would get
        // SpawnFailed rather than a clean Skipped.
        let install = VipsInstall {
            exe: std::path::PathBuf::from("this-should-never-run"),
            version: crate::vips::VipsVersion {
                major: 8,
                minor: 18,
                patch: 0,
            },
            capabilities: Default::default(),
            source: crate::vips::discovery::DiscoverySource::Path,
        };

        let outcome = convert_one(&install, &settings, &input).expect("skip is not an error");
        assert_eq!(outcome, Outcome::Skipped { output: existing });
    }

    #[test]
    fn concurrency_zero_means_do_not_set_the_variable() {
        assert_eq!(
            concurrency_env(0),
            None,
            "0 should leave libvips to its own default"
        );
    }

    #[test]
    fn concurrency_is_passed_as_the_vips_environment_variable() {
        assert_eq!(
            concurrency_env(1),
            Some(("VIPS_CONCURRENCY", "1".to_owned()))
        );
        assert_eq!(
            concurrency_env(8),
            Some(("VIPS_CONCURRENCY", "8".to_owned()))
        );
    }

    #[test]
    fn stderr_cleaning_strips_the_libvips_class_prefix() {
        let raw = b"VipsForeignLoad: \"bad.jpg\" is not a known file format\n";
        let cleaned = clean_stderr(raw, Some(1));
        assert_eq!(cleaned, "\"bad.jpg\" is not a known file format");
    }

    #[test]
    fn stderr_cleaning_keeps_the_most_specific_line() {
        // libvips chains context lines; the last is the useful one.
        let raw = b"vips: error calling saver\nVipsForeignSaveHeifFile: unsupported bitdepth\n";
        let cleaned = clean_stderr(raw, Some(1));
        assert_eq!(cleaned, "unsupported bitdepth");
    }

    #[test]
    fn stderr_cleaning_leaves_messages_with_paths_intact() {
        // A Windows path contains a colon but is not a class prefix.
        let raw = b"could not open C:\\Users\\me\\a.jpg for reading\n";
        let cleaned = clean_stderr(raw, Some(1));
        assert_eq!(cleaned, "could not open C:\\Users\\me\\a.jpg for reading");
    }

    #[test]
    fn stderr_cleaning_falls_back_when_vips_says_nothing() {
        assert_eq!(
            clean_stderr(b"", Some(9)),
            "vips exited with code 9 and no message"
        );
        assert_eq!(
            clean_stderr(b"   \n\n", None),
            "vips was terminated before it finished"
        );
    }
}
