//! TRUEOS delegates companion presentation to resident Spirit via lifecycle OSC.
//! Conversation image transport remains independent of that presentation.

use super::ChatWidget;

impl ChatWidget {
    pub(crate) fn open_pets_picker(&mut self) {
        self.add_info_message(
            "Lilly is managed by TRUEOS Spirit and follows this session’s activity automatically."
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
