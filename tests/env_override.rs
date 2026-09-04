//! The `VIPS_GUI_VIPS_EXE` escape hatch.
//!
//! This lives in its own test binary because it mutates process environment
//! variables, which would otherwise race the other tests running in parallel.
//! One test per process keeps that safe.

use vips_gui::vips::discovery::{self, DiscoveryError, DiscoverySource, ENV_OVERRIDE};

#[test]
fn env_override_pointing_at_a_missing_file_is_reported_not_ignored() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ghost = dir.path().join("nowhere").join("vips.exe");

    // SAFETY: this test binary is single-threaded at this point; no other
    // thread can observe the environment mid-mutation.
    unsafe {
        std::env::set_var(ENV_OVERRIDE, &ghost);
    }

    let err = discovery::discover(None).expect_err("a bad override must not fall through to PATH");
    assert!(
        matches!(err, DiscoveryError::OverrideMissing { .. }),
        "expected OverrideMissing so the UI can explain the bad path, got {err:?}"
    );

    // An empty value means "no override", not "a file called empty string".
    unsafe {
        std::env::set_var(ENV_OVERRIDE, "");
    }
    let empty_result = discovery::discover(None);
    match empty_result {
        Err(DiscoveryError::NotFound) => {
            // No vips installed on this machine, which is a fine outcome.
        }
        Ok(install) => {
            // vips is installed, so an empty override should have been skipped
            // in favour of the normal search order.
            assert_ne!(
                install.source,
                DiscoverySource::Environment,
                "an empty override should not be treated as a real path"
            );
        }
        Err(other) => panic!("an empty override should be ignored, got {other:?}"),
    }

    unsafe {
        std::env::remove_var(ENV_OVERRIDE);
    }
}
