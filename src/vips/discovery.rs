//! Locating an installed `vips` executable and working out what it can do.
//!
//! Resolution order, first hit wins:
//!
//! 1. An explicit path the user picked in the UI (persisted across runs).
//! 2. The `VIPS_GUI_VIPS_EXE` environment variable (handy for tests and CI).
//! 3. `PATH`, which is what the official Windows install instructions produce.
//! 4. A short list of common install locations, so a user who unzipped the
//!    release but forgot the `PATH` step still gets a working app.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{MIN_VERSION, hide_console};

/// Environment variable that overrides discovery entirely.
pub const ENV_OVERRIDE: &str = "VIPS_GUI_VIPS_EXE";

/// A parsed `vips` version, e.g. `vips-8.18.6` -> `8.18.6`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VipsVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl VipsVersion {
    /// Parse the output of `vips --version`.
    ///
    /// The expected shape is `vips-8.18.6`, but we also accept a bare
    /// `8.18.6`, a missing patch level, and trailing pre-release junk such as
    /// `8.15.0-rc2`, because different builds and packagers vary.
    pub fn parse(raw: &str) -> Option<Self> {
        let text = raw.trim();
        // Take the first whitespace-separated token, so `vips-8.18.6 (blah)` works.
        let token = text.split_whitespace().next()?;
        // Strip a leading `vips-` / `vips` prefix if present.
        let digits = token
            .strip_prefix("vips-")
            .or_else(|| token.strip_prefix("vips"))
            .unwrap_or(token)
            .trim_start_matches(['-', ' ']);

        // Cut any pre-release or build suffix: `8.15.0-rc2` -> `8.15.0`.
        let core = digits
            .split(['-', '+'])
            .next()
            .filter(|s| !s.is_empty())?;

        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        // A bare `8` is a valid, if unusual, version. Absent parts read as 0.
        let minor = match parts.next() {
            Some(p) => p.parse().ok()?,
            None => 0,
        };
        let patch = match parts.next() {
            Some(p) => p.parse().ok()?,
            None => 0,
        };

        Some(Self {
            major,
            minor,
            patch,
        })
    }

    /// `keep=none` replaced the older `strip` flag in libvips 8.15.
    ///
    /// Both spellings are accepted on 8.18, but only `strip` works before
    /// 8.15, so the command builder needs to know which to emit.
    pub fn uses_keep_option(self) -> bool {
        self >= VipsVersion {
            major: 8,
            minor: 15,
            patch: 0,
        }
    }
}

impl fmt::Display for VipsVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Which savers the discovered build actually has compiled in.
///
/// A libvips built without libheif has no `heifsave`, so AVIF must be
/// disabled in the UI rather than failing at conversion time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub jpeg: bool,
    pub png: bool,
    pub webp: bool,
    pub avif: bool,
}

impl Capabilities {
    /// Parse the operation listing from `vips -l`.
    ///
    /// Lines look like:
    /// `VipsForeignSavePngFile (pngsave), save image to file as png, ...`
    /// so we look for the parenthesised nickname.
    pub fn parse_listing(listing: &str) -> Self {
        Self {
            jpeg: listing.contains("(jpegsave)"),
            png: listing.contains("(pngsave)"),
            webp: listing.contains("(webpsave)"),
            avif: listing.contains("(heifsave)"),
        }
    }

    /// True when every format the UI offers is available.
    pub fn all_present(self) -> bool {
        self.jpeg && self.png && self.webp && self.avif
    }
}

/// A validated `vips` installation, ready to use.
#[derive(Debug, Clone)]
pub struct VipsInstall {
    /// Absolute path to the executable.
    pub exe: PathBuf,
    pub version: VipsVersion,
    pub capabilities: Capabilities,
    /// How we found it, for display in the UI.
    pub source: DiscoverySource,
}

impl VipsInstall {
    /// Start a `vips` command against this install, with the console hidden.
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(&self.exe);
        hide_console(&mut cmd);
        cmd
    }
}

/// Which step of the resolution order produced the executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoverySource {
    /// Explicitly chosen by the user.
    UserOverride,
    /// From `VIPS_GUI_VIPS_EXE`.
    Environment,
    /// Found on `PATH`.
    Path,
    /// Found by scanning common install directories.
    WellKnownDirectory,
}

impl DiscoverySource {
    pub fn describe(self) -> &'static str {
        match self {
            Self::UserOverride => "chosen manually",
            Self::Environment => "from VIPS_GUI_VIPS_EXE",
            Self::Path => "found on PATH",
            Self::WellKnownDirectory => "found in a standard install location",
        }
    }
}

/// Why discovery failed. Each variant maps to specific UI guidance.
#[derive(Debug, Clone, thiserror::Error)]
pub enum DiscoveryError {
    #[error("Could not find vips.exe on your PATH or in the usual install locations.")]
    NotFound,

    #[error("{path} does not exist.")]
    OverrideMissing { path: String },

    #[error("Could not run {path}: {reason}")]
    NotExecutable { path: String, reason: String },

    #[error("Could not read a version number from `{path} --version` (it printed: {output})")]
    UnreadableVersion { path: String, output: String },

    #[error(
        "Found libvips {found} at {path}, but this app needs {min} or newer. \
         Please update libvips."
    )]
    TooOld {
        path: String,
        found: VipsVersion,
        min: VipsVersion,
    },
}

/// Run the full resolution order and validate whatever turns up.
///
/// `user_override` is the persisted path from settings, if any.
pub fn discover(user_override: Option<&Path>) -> Result<VipsInstall, DiscoveryError> {
    // 1. An explicit user choice is authoritative: if it is broken, say so
    //    rather than silently falling back to a different binary, which would
    //    be baffling.
    if let Some(path) = user_override {
        if !path.is_file() {
            return Err(DiscoveryError::OverrideMissing {
                path: path.display().to_string(),
            });
        }
        return validate(path, DiscoverySource::UserOverride);
    }

    // 2. Environment override, same reasoning.
    if let Some(raw) = std::env::var_os(ENV_OVERRIDE) {
        let path = PathBuf::from(&raw);
        if path.as_os_str().is_empty() {
            return Err(DiscoveryError::NotFound);
        }
        if !path.is_file() {
            return Err(DiscoveryError::OverrideMissing {
                path: path.display().to_string(),
            });
        }
        return validate(&path, DiscoverySource::Environment);
    }

    // 3. PATH, the outcome of the documented install procedure.
    if let Ok(found) = which::which(exe_name()) {
        return validate(&found, DiscoverySource::Path);
    }

    // 4. Common install locations, for the "unzipped but forgot PATH" case.
    if let Some(found) = scan_well_known_dirs() {
        return validate(&found, DiscoverySource::WellKnownDirectory);
    }

    Err(DiscoveryError::NotFound)
}

/// Platform-appropriate executable name.
fn exe_name() -> &'static str {
    if cfg!(windows) { "vips.exe" } else { "vips" }
}

/// Confirm a candidate runs, report a usable version, and probe its savers.
pub fn validate(exe: &Path, source: DiscoverySource) -> Result<VipsInstall, DiscoveryError> {
    let mut cmd = Command::new(exe);
    hide_console(&mut cmd);
    let output = cmd
        .arg("--version")
        .output()
        .map_err(|e| DiscoveryError::NotExecutable {
            path: exe.display().to_string(),
            reason: e.to_string(),
        })?;

    // `vips --version` prints to stdout, but be forgiving and check stderr too.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = if stdout.trim().is_empty() {
        stderr.to_string()
    } else {
        stdout.to_string()
    };

    let version =
        VipsVersion::parse(&combined).ok_or_else(|| DiscoveryError::UnreadableVersion {
            path: exe.display().to_string(),
            output: truncate_for_display(combined.trim()),
        })?;

    if version < MIN_VERSION {
        return Err(DiscoveryError::TooOld {
            path: exe.display().to_string(),
            found: version,
            min: MIN_VERSION,
        });
    }

    // Capability probe. A failure here is not fatal: assume everything is
    // present and let individual conversions report their own errors, which is
    // friendlier than refusing to start.
    let capabilities = probe_capabilities(exe).unwrap_or(Capabilities {
        jpeg: true,
        png: true,
        webp: true,
        avif: true,
    });

    Ok(VipsInstall {
        exe: exe.to_path_buf(),
        version,
        capabilities,
        source,
    })
}

/// Ask `vips -l` which savers are compiled in.
fn probe_capabilities(exe: &Path) -> Option<Capabilities> {
    let mut cmd = Command::new(exe);
    hide_console(&mut cmd);
    let output = cmd.arg("-l").output().ok()?;
    // `vips -l` writes the class hierarchy to stdout; some builds use stderr.
    let mut listing = String::from_utf8_lossy(&output.stdout).into_owned();
    if listing.trim().is_empty() {
        listing = String::from_utf8_lossy(&output.stderr).into_owned();
    }
    if listing.trim().is_empty() {
        return None;
    }
    Some(Capabilities::parse_listing(&listing))
}

/// Look through directories where a Windows libvips commonly ends up.
///
/// The official zip unpacks to `vips-<version>\bin\vips.exe`, so each root is
/// scanned one level deep for `vips*` directories.
fn scan_well_known_dirs() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA", "USERPROFILE"] {
        if let Some(base) = std::env::var_os(var) {
            let base = PathBuf::from(base);
            roots.push(base.clone());
            roots.push(base.join("Programs"));
        }
    }
    // Scoop and Chocolatey shim locations.
    if let Some(home) = std::env::var_os("USERPROFILE") {
        roots.push(PathBuf::from(&home).join("scoop").join("apps").join("libvips"));
        roots.push(PathBuf::from(&home).join("scoop").join("shims"));
    }
    if let Some(choco) = std::env::var_os("ChocolateyInstall") {
        roots.push(PathBuf::from(choco).join("bin"));
    }
    // Bare drive-root installs, which is where hand-unzipped copies often land.
    for drive in ["C:\\", "D:\\"] {
        roots.push(PathBuf::from(drive));
        roots.push(PathBuf::from(drive).join("Bin"));
        roots.push(PathBuf::from(drive).join("tools"));
    }

    for root in roots {
        // The executable might sit directly in this directory (shim dirs).
        let direct = root.join(exe_name());
        if direct.is_file() {
            return Some(direct);
        }
        // Or inside a `vips*` subdirectory, with or without a `bin` level.
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy().to_ascii_lowercase();
            if !name.starts_with("vips") && !name.starts_with("libvips") {
                continue;
            }
            let dir = entry.path();
            for candidate in [dir.join("bin").join(exe_name()), dir.join(exe_name())] {
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

/// Keep error text short enough to fit in a dialog.
fn truncate_for_display(text: &str) -> String {
    const LIMIT: usize = 120;
    if text.is_empty() {
        return "<nothing>".to_owned();
    }
    let single_line = text.replace(['\n', '\r'], " ");
    if single_line.chars().count() <= LIMIT {
        return single_line;
    }
    let cut: String = single_line.chars().take(LIMIT).collect();
    format!("{cut}...")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_standard_vips_version_output() {
        let v = VipsVersion::parse("vips-8.18.6").expect("should parse");
        assert_eq!(
            v,
            VipsVersion {
                major: 8,
                minor: 18,
                patch: 6
            }
        );
    }

    #[test]
    fn parses_with_surrounding_whitespace_and_newline() {
        // This is literally what the process gives us: a trailing newline.
        let v = VipsVersion::parse("  vips-8.15.1\n").expect("should parse");
        assert_eq!(v.minor, 15);
        assert_eq!(v.patch, 1);
    }

    #[test]
    fn parses_a_bare_version_without_the_vips_prefix() {
        let v = VipsVersion::parse("8.14.2").expect("should parse");
        assert_eq!(
            v,
            VipsVersion {
                major: 8,
                minor: 14,
                patch: 2
            }
        );
    }

    #[test]
    fn parses_a_prerelease_by_discarding_the_suffix() {
        let v = VipsVersion::parse("vips-8.16.0-rc2").expect("should parse");
        assert_eq!(
            v,
            VipsVersion {
                major: 8,
                minor: 16,
                patch: 0
            }
        );
    }

    #[test]
    fn treats_missing_components_as_zero() {
        assert_eq!(
            VipsVersion::parse("vips-9").expect("should parse"),
            VipsVersion {
                major: 9,
                minor: 0,
                patch: 0
            }
        );
        assert_eq!(
            VipsVersion::parse("vips-8.20").expect("should parse"),
            VipsVersion {
                major: 8,
                minor: 20,
                patch: 0
            }
        );
    }

    #[test]
    fn rejects_output_with_no_version_in_it() {
        assert!(VipsVersion::parse("").is_none());
        assert!(VipsVersion::parse("   ").is_none());
        assert!(VipsVersion::parse("command not found").is_none());
        assert!(VipsVersion::parse("vips-").is_none());
        assert!(VipsVersion::parse("vips-x.y.z").is_none());
    }

    #[test]
    fn version_ordering_is_numeric_not_lexicographic() {
        let older = VipsVersion::parse("vips-8.9.0").unwrap();
        let newer = VipsVersion::parse("vips-8.18.0").unwrap();
        // A string compare would wrongly call "8.9" greater than "8.18".
        assert!(older < newer);
        assert!(older < MIN_VERSION);
        assert!(newer > MIN_VERSION);
    }

    #[test]
    fn min_version_boundary_is_inclusive() {
        let exactly_min = VipsVersion::parse("8.13.0").unwrap();
        assert!(exactly_min >= MIN_VERSION);
        let just_below = VipsVersion::parse("8.12.9").unwrap();
        assert!(just_below < MIN_VERSION);
    }

    #[test]
    fn keep_option_switches_over_at_8_15() {
        assert!(!VipsVersion::parse("8.14.5").unwrap().uses_keep_option());
        assert!(VipsVersion::parse("8.15.0").unwrap().uses_keep_option());
        assert!(VipsVersion::parse("8.18.6").unwrap().uses_keep_option());
    }

    #[test]
    fn detects_savers_from_a_real_vips_listing() {
        // Trimmed from actual `vips -l` output on 8.18.6.
        let listing = "\
        VipsForeignSavePng (pngsave_base), save png (.png), priority=0, mono rgb alpha
          VipsForeignSavePngFile (pngsave), save image to file as png, nocache (.png)
        VipsForeignSaveJpeg (jpegsave_base), save as jpeg (.jpg, .jpeg)
          VipsForeignSaveJpegFile (jpegsave), save as jpeg, nocache (.jpg)
        VipsForeignSaveWebp (webpsave_base), save as WebP (.webp)
          VipsForeignSaveWebpFile (webpsave), save as WebP, nocache (.webp)
        VipsForeignSaveHeif (heifsave_base), save image in HEIF format
          VipsForeignSaveHeifFile (heifsave), save image in HEIF format, nocache (.heic, .heif, .avif)";
        let caps = Capabilities::parse_listing(listing);
        assert!(caps.jpeg && caps.png && caps.webp && caps.avif);
        assert!(caps.all_present());
    }

    #[test]
    fn detects_a_build_without_libheif() {
        // Same listing with the HEIF classes removed, as a no-libheif build.
        let listing = "\
          VipsForeignSavePngFile (pngsave), save image to file as png, nocache (.png)
          VipsForeignSaveJpegFile (jpegsave), save as jpeg, nocache (.jpg)
          VipsForeignSaveWebpFile (webpsave), save as WebP, nocache (.webp)";
        let caps = Capabilities::parse_listing(listing);
        assert!(caps.jpeg && caps.png && caps.webp);
        assert!(!caps.avif, "AVIF must be reported as unavailable");
        assert!(!caps.all_present());
    }

    #[test]
    fn saver_base_classes_alone_do_not_count_as_support() {
        // `heifsave_base` present but no concrete `heifsave` saver.
        let listing = "VipsForeignSaveHeif (heifsave_base), save image in HEIF format";
        let caps = Capabilities::parse_listing(listing);
        assert!(!caps.avif);
    }

    #[test]
    fn a_missing_user_override_is_reported_as_such() {
        let dir = tempfile::tempdir().unwrap();
        let ghost = dir.path().join("not-here.exe");
        let err = discover(Some(&ghost)).expect_err("should fail");
        assert!(
            matches!(err, DiscoveryError::OverrideMissing { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn user_override_takes_priority_over_everything_else() {
        // A file that exists but is not a working vips. Discovery must try it
        // (and fail on it) rather than skipping to PATH, where a real vips may
        // well be installed on this machine.
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("vips.exe");
        std::fs::write(&fake, b"not a real executable").unwrap();

        let result = discover(Some(&fake));
        match result {
            Err(DiscoveryError::NotExecutable { .. } | DiscoveryError::UnreadableVersion { .. }) => {}
            other => panic!("expected the override to be attempted and rejected, got {other:?}"),
        }
    }

    #[test]
    fn display_truncation_keeps_dialogs_readable() {
        assert_eq!(truncate_for_display(""), "<nothing>");
        assert_eq!(truncate_for_display("short"), "short");
        assert_eq!(truncate_for_display("a\nb"), "a b");
        let long = "x".repeat(500);
        let out = truncate_for_display(&long);
        assert!(out.len() <= 124, "was {}", out.len());
        assert!(out.ends_with("..."));
    }
}
