//! Register the leased terminal with UI4's shared collapse/restore menu.

// Rasterized from trueos-collapse.svg (the OPENAI path in
// empty_state_animation/paths.rs), 128x128, transparent black, straight-alpha RGBA8.
// Lily's build-codex-static.py derives its native logo using UI4's same sampling.
const ICON: &[u8; 128 * 128 * 4] = include_bytes!("trueos-collapse.rgba");

pub(super) fn register() {
    // Scene zero selects the caller's leased terminal, as in Edit. UI4 copies
    // the icon and owns the menu, collapse animation, dragging and restoration.
    // set_modes also runs after terminal reentry so a new surface gets the icon.
    let status = unsafe {
        trueos_v::bp_abi::trueos_cabi_ui4_scene_enable_collapse_v1(
            0,
            128,
            128,
            ICON.as_ptr(),
            ICON.len(),
        )
    };
    if status != 0 {
        tracing::debug!(status, "TRUEOS terminal collapse registration unavailable");
    }
}
