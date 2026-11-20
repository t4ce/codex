//! TRUEOS resident Spirit integration information.
//! Conversation image transport remains independent of that presentation.

use super::ChatWidget;

impl ChatWidget {
    pub(crate) fn show_spirit_info(&mut self) {
        self.add_info_message(
            "Codex uses Spirit to see and operate shared TRUEOS windows through Lilly’s cursor and keyboard. To share a window, right-drag it onto the green circle around Lilly and release. The window’s AI badge confirms access. Repeat the gesture to revoke it. You can then ask Codex for the result you want in that window."
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

/// The response tee performs no classification or Spirit calls in this Hull.
pub(super) type ResponsePresentation = crate::trueos_lily::ResponsePresentation;
