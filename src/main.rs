//! vips-gui: a small desktop app for batch image conversion via libvips.
//!
//! libvips is not bundled. It is discovered at startup from `PATH` (or a
//! user-supplied location) and driven as a subprocess, which keeps this
//! program a single self-contained executable.

// Suppress the console window on Windows release builds. Debug builds keep it,
// because `dbg!` output and panic messages are worth more than a tidy taskbar.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use vips_gui::app;

fn main() -> eframe::Result<()> {
    // A couple of command-line flags, so a user can diagnose a libvips problem
    // without the GUI and a packaging test can confirm the binary starts.
    match Cli::from_args() {
        Cli::Check => {
            attach_parent_console();
            let code = vips_gui::diagnostics::print_check();
            std::process::exit(code);
        }
        Cli::Version => {
            attach_parent_console();
            println!("vips-gui {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        Cli::Help => {
            attach_parent_console();
            print!("{}", Cli::usage());
            std::process::exit(0);
        }
        Cli::Gui => {}
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([820.0, 520.0])
            .with_title("vips-gui")
            .with_app_id("vips-gui"),
        ..Default::default()
    };

    eframe::run_native(
        "vips-gui",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}

/// What the user asked for on the command line.
enum Cli {
    Gui,
    Check,
    Version,
    Help,
}

impl Cli {
    fn from_args() -> Self {
        for arg in std::env::args().skip(1) {
            match arg.as_str() {
                "--check" | "-c" => return Self::Check,
                "--version" | "-V" => return Self::Version,
                "--help" | "-h" | "-?" => return Self::Help,
                // Anything else is ignored: the GUI takes no arguments, and
                // refusing to start over a stray flag would be unhelpful.
                _ => {}
            }
        }
        Self::Gui
    }

    fn usage() -> String {
        format!(
            "vips-gui {version} - batch image conversion with libvips\n\
             \n\
             Usage: vips-gui [OPTIONS]\n\
             \n\
             With no options, opens the graphical interface.\n\
             \n\
             Options:\n\
             \x20 -c, --check      Report the libvips installation and exit\n\
             \x20 -V, --version    Print the version and exit\n\
             \x20 -h, --help       Print this help and exit\n\
             \n\
             libvips must be installed separately and on your PATH.\n\
             See {url}\n",
            version = env!("CARGO_PKG_VERSION"),
            url = vips_gui::vips::DOWNLOAD_URL,
        )
    }
}

/// Attach to the parent console so `--check` output is visible.
///
/// Release builds are linked as a GUI subsystem application, which means they
/// start with no console attached and `println!` goes nowhere when run from an
/// existing terminal. Attaching to the parent's console fixes that without
/// giving the GUI a stray black window.
fn attach_parent_console() {
    #[cfg(windows)]
    {
        // SAFETY: `AttachConsole` takes a process id and has no preconditions
        // beyond that. Failure (no parent console, or one already attached) is
        // expected and ignored.
        unsafe {
            const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
            unsafe extern "system" {
                fn AttachConsole(dwProcessId: u32) -> i32;
            }
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}
