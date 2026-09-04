//! Embeds the Windows icon and version information into the executable.
//!
//! Only does anything on Windows. On other platforms the resource script is
//! irrelevant and this is a no-op, so the crate still builds elsewhere.

fn main() {
    // Rerun when the resources change, not on every source edit.
    println!("cargo:rerun-if-changed=assets/vips-gui.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");

    #[cfg(windows)]
    {
        // `compile` needs the resource compiler from the Windows SDK. If it is
        // missing, fail loudly rather than silently shipping an unbranded
        // binary, since a missing icon is easy to overlook.
        embed_resource::compile("assets/vips-gui.rc", embed_resource::NONE)
            .manifest_required()
            .expect("failed to compile assets/vips-gui.rc");
    }
}
