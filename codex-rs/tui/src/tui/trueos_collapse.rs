//! Register the leased terminal with UI4's shared collapse/restore menu.

// Native UI4 collapse tile: exact64px codex-tile.svg raster, including its
// translucent gray background. Same straight-alpha pixels are owned by Spirit
// for the procedural hardware-cursor handoff.
const ICON: &[u8; 64 * 64 * 4] = include_bytes!("trueos-collapse.rgba");

pub(super) fn register() {
    // Scene zero selects the caller's leased terminal, as in Edit. UI4 copies
    // the icon and owns the menu, collapse animation, dragging and restoration.
    // set_modes also runs after terminal reentry so a new surface gets the icon.
    let status = unsafe {
        trueos_v::bp_abi::trueos_cabi_ui4_scene_enable_collapse_v1(
            0,
            64,
            64,
            ICON.as_ptr(),
            ICON.len(),
        )
    };
    if status != 0 {
        tracing::debug!(status, "TRUEOS terminal collapse registration unavailable");
    }
}
