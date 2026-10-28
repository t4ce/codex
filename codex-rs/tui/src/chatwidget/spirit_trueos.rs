//! TRUEOS resident Spirit integration information.
//! Conversation image transport remains independent of that presentation.

use super::ChatWidget;

impl ChatWidget {
    pub(crate) fn show_spirit_info(&mut self) {
        self.add_info_message(
            "Spirit is configured automatically at TRUEOS boot, with Lilly as her avatar. This Codex session uses Spirit for native OS interaction and reports its activity to her. Spirit owns avatar presentation, transitions, and the paired cursor and keyboard."
                .to_string(),
            None,
        );
    }

    pub(super) fn ambient_pet_wrap_reserved_cols(&self) -> u16 {
        0
    }

    pub(crate) fn history_wrap_width(&self, width: u16) -> u16 {
        width.max(1)
    }

    pub(crate) fn pet_image_support(&self) -> crate::terminal_image::PetImageSupport {
        crate::terminal_image::detect_pet_image_support()
    }
}
