//! Discovery against a real libvips installation.

mod common;

use vips_gui::vips::{MIN_VERSION, discovery};

#[test]
fn finds_and_validates_the_installed_vips() {
    let Some(install) = common::vips() else {
        common::skip("finds_and_validates_the_installed_vips");
        return;
    };

    assert!(
        install.exe.is_file(),
        "discovered path should be a real file: {}",
        install.exe.display()
    );
    assert!(
        install.version >= MIN_VERSION,
        "discovered {} but the minimum is {MIN_VERSION}",
        install.version
    );
    // Any real build can write at least PNG and JPEG.
    assert!(install.capabilities.png, "expected pngsave to be available");
    assert!(
        install.capabilities.jpeg,
        "expected jpegsave to be available"
    );
}

#[test]
fn validating_a_non_vips_executable_fails_cleanly() {
    let Some(_install) = common::vips() else {
        common::skip("validating_a_non_vips_executable_fails_cleanly");
        return;
    };

    // Point discovery at a real executable that is definitely not vips. It
    // must produce a typed error rather than panicking or hanging.
    let not_vips = if cfg!(windows) {
        std::path::PathBuf::from(r"C:\Windows\System32\where.exe")
    } else {
        std::path::PathBuf::from("/bin/true")
    };
    if !not_vips.is_file() {
        eprintln!("SKIPPING: no stand-in executable at {}", not_vips.display());
        return;
    }

    let err = discovery::discover(Some(&not_vips))
        .expect_err("a non-vips executable must not validate");
    assert!(
        matches!(
            err,
            discovery::DiscoveryError::UnreadableVersion { .. }
                | discovery::DiscoveryError::NotExecutable { .. }
        ),
        "unexpected error variant: {err:?}"
    );
}

#[test]
fn the_environment_override_is_honoured() {
    let Some(install) = common::vips() else {
        common::skip("the_environment_override_is_honoured");
        return;
    };

    // Validate the discovered exe directly through the same code path the
    // env-var branch uses, confirming an explicit path is accepted.
    let via_override = discovery::discover(Some(&install.exe))
        .expect("the discovered executable should validate when passed explicitly");

    assert_eq!(via_override.exe, install.exe);
    assert_eq!(via_override.version, install.version);
    assert_eq!(
        via_override.source,
        discovery::DiscoverySource::UserOverride,
        "an explicit path should be reported as a user override"
    );
}
