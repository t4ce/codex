//! TRUEOS resident Spirit integration information.
//! Conversation image transport remains independent of that presentation.

use super::ChatWidget;

impl ChatWidget {
    pub(crate) fn show_spirit_info(&mut self) {
        self.add_info_message(
            "Spirit is configured automatically at TRUEOS boot, with Lilly as her avatar. This Codex session uses Spirit for native OS interaction and reports its activity to her. Live assistant responses also drive existing Lilly expressions from bounded text snapshots using the warm CPU VNNI classifier. Spirit owns avatar presentation, transitions, and the paired cursor and keyboard."
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

/// Own only response sampling bookkeeping; the process-wide integer model stays
/// warm in lily2Vec and Spirit owns all animation playback/transition behavior.
pub(super) struct ResponsePresentation {
    started: std::time::Instant,
    presenter: lily2vec::response::ResponsePresenter,
    disabled: bool,
}
impl Default for ResponsePresentation {
    fn default() -> Self {
        Self {
            started: std::time::Instant::now(),
            presenter: Default::default(),
            disabled: false,
        }
    }
}
impl ResponsePresentation {
    fn submit(label: &str) -> bool {
        match trueos::spirit::play_expression(label) {
            Ok(()) => true,
            Err(error) => {
                tracing::debug!(?error, label, "Spirit response expression was not queued");
                false
            }
        }
    }
    pub(super) fn delta(&mut self, text: &str) {
        if self.disabled {
            return;
        }
        let result = self
            .presenter
            .push(text, self.started.elapsed(), Self::submit);
        self.check(result);
    }
    pub(super) fn finish(&mut self, response: Option<&str>) {
        if self.disabled {
            return;
        }
        let result = self
            .presenter
            .finish(response, self.started.elapsed(), Self::submit);
        self.check(result);
    }
    pub(super) fn clear(&mut self) {
        self.presenter.clear();
    }
    fn check(&mut self, result: anyhow::Result<()>) {
        if let Err(error) = result {
            // One diagnostic, no alternate arithmetic backend and no effect on
            // terminal output if the current CPU cannot execute AVX-VNNI.
            tracing::warn!(%error, "Spirit response expressions unavailable");
            self.disabled = true;
        }
    }
}
