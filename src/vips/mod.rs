//! Everything that talks to the external `vips` executable.
//!
//! The app never links libvips. It locates an installed `vips` binary and
//! drives it as a subprocess. That keeps our own build a single static
//! executable with no DLLs to ship, at the cost of one process spawn per
//! conversion (negligible next to the encode itself).

pub mod command;
pub mod convert;
pub mod discovery;
pub mod preview;

pub use discovery::{DiscoveryError, VipsInstall, VipsVersion};

/// Where users can get libvips for Windows.
///
/// The documented install is: download `vips-dev-w64-web-<version>.zip`, unzip
/// it, and put the `vips-<version>\bin` directory on `PATH`.
pub const DOWNLOAD_URL: &str = "https://github.com/libvips/libvips/releases";

/// Oldest release we accept.
///
/// 8.13 is the floor for dependable AVIF `effort` handling in `heifsave`.
pub const MIN_VERSION: VipsVersion = VipsVersion {
    major: 8,
    minor: 13,
    patch: 0,
};

/// Configure a [`std::process::Command`] so it never flashes a console window.
///
/// Without this, every spawned `vips.exe` briefly pops a black window, which
/// looks broken in a GUI app that is itself compiled with
/// `windows_subsystem = "windows"`.
pub fn hide_console(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// `CREATE_NO_WINDOW` from the Win32 process creation flags.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}
