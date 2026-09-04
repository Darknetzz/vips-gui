//! The `--check` report: what this build found, and what to do if it found
//! nothing.
//!
//! This exists because "the app says libvips is missing but it's definitely
//! installed" is the one support question a program like this will always get.
//! Having a text-mode answer makes it diagnosable over a chat message.

use crate::settings::OutputFormat;
use crate::vips::{self, DOWNLOAD_URL, MIN_VERSION};

/// Print a description of the libvips installation.
///
/// Returns the process exit code: 0 when a usable libvips was found, 1 when it
/// was not, so the check is scriptable.
pub fn print_check() -> i32 {
    println!("vips-gui {}", env!("CARGO_PKG_VERSION"));
    println!();

    match vips::discovery::discover(None) {
        Ok(install) => {
            println!("libvips:     {} (minimum required: {MIN_VERSION})", install.version);
            println!("executable:  {}", install.exe.display());
            println!("found via:   {}", install.source.describe());

            let caps = install.capabilities;
            println!();
            println!("Output formats:");
            for format in OutputFormat::ALL {
                let available = match format {
                    OutputFormat::Jpeg => caps.jpeg,
                    OutputFormat::Png => caps.png,
                    OutputFormat::Webp => caps.webp,
                    OutputFormat::Avif => caps.avif,
                };
                println!(
                    "  {:<5} {:<10} ({})",
                    if available { "yes" } else { "no" },
                    format.label(),
                    format.saver_name()
                );
            }

            println!();
            let stdout_previews = vips::preview::probe_stdout_support(&install);
            println!(
                "Previews:    {}",
                if stdout_previews {
                    "streaming (this libvips can write to stdout)"
                } else {
                    "via temporary files (this libvips will not write to stdout)"
                }
            );

            if !caps.all_present() {
                println!();
                println!("Note: some formats are unavailable in this libvips build.");
                println!("A build with more format support is available at {DOWNLOAD_URL}");
            }

            0
        }
        Err(err) => {
            println!("libvips:     NOT USABLE");
            println!();
            println!("{err}");
            println!();
            println!("To fix this on Windows:");
            println!("  1. Download vips-dev-w64-web-<version>.zip from");
            println!("     {DOWNLOAD_URL}");
            println!("  2. Unzip it somewhere permanent.");
            println!("  3. Add the vips-<version>\\bin folder to your PATH.");
            println!();
            println!(
                "Alternatively set {} to the full path of vips.exe.",
                vips::discovery::ENV_OVERRIDE
            );
            1
        }
    }
}
