//! User-authored native grid input shares the normal message queue, never the clipboard.

use super::*;

impl ChatWidget {
    pub(crate) fn trueos_prompt_ready(&self) -> bool {
        self.is_session_configured()
            && !self.blocks_direct_input
            && !self.has_misalignment_policy_violation()
            && !self
                .effective_collaboration_mode()
                .model()
                .trim()
                .is_empty()
    }

    pub(crate) fn submit_trueos_prompt(&mut self, text: String) -> bool {
        if !self.trueos_prompt_ready() || text.trim().is_empty() {
            return false;
        }
        // The OS frame submits one complete user-authored statement. Reuse the
        // existing atomic text action without borrowing draft attachments or
        // interpreting its first line as a terminal command.
        let accepted = self.queue_user_message_with_options(
            text.into(),
            QueuedInputAction::Literal,
            Vec::new(),
        );
        if accepted {
            self.app_event_tx.send(AppEvent::FollowTranscript);
            self.request_redraw();
        }
        accepted
    }
}
