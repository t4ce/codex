//! Transient next-prompt placeholders are independent of the editable draft and its history.

use super::*;
use uuid::Uuid;

// Shell3 reserves Tab for leaving the VM terminal on TRUEOS.
const ACCEPT_KEY: KeyCode = if cfg!(target_os = "trueos") {
    KeyCode::Enter
} else {
    KeyCode::Tab
};

#[derive(Default)]
pub(super) struct NextPromptState {
    request_id: Option<Uuid>,
    text: Option<String>,
}

impl ChatComposer {
    pub(crate) fn begin_next_prompt(&mut self) -> Option<Uuid> {
        self.dismiss_next_prompt();
        if !self.is_empty()
            || self.is_task_running
            || !self.draft.input_enabled
            || self.blocks_direct_input
            || self.popup_active()
            || self.is_in_paste_burst()
        {
            return None;
        }
        let request_id = Uuid::new_v4();
        self.next_prompt.request_id = Some(request_id);
        Some(request_id)
    }

    pub(crate) fn next_prompt_request_matches(&self, request_id: Uuid) -> bool {
        self.next_prompt.request_id == Some(request_id)
            && self.is_empty()
            && !self.is_task_running
            && self.draft.input_enabled
            && !self.blocks_direct_input
            && !self.popup_active()
            && !self.is_in_paste_burst()
    }

    pub(crate) fn apply_next_prompt(&mut self, request_id: Uuid, text: Option<String>) {
        if self.next_prompt.request_id == Some(request_id) && text.is_none() {
            self.dismiss_next_prompt();
            return;
        }
        if self.next_prompt_request_matches(request_id) {
            self.dismiss_sparkle();
            self.next_prompt.text = text;
        }
    }

    pub(crate) fn dismiss_next_prompt(&mut self) {
        if let Some(request_id) = self.next_prompt.request_id.take() {
            self.app_event_tx
                .send(AppEvent::DismissNextPrompt { request_id });
        }
        self.next_prompt.text = None;
    }

    pub(crate) fn has_next_prompt(&self) -> bool {
        self.next_prompt.request_id.is_some()
    }

    pub(super) fn next_prompt_text(&self) -> Option<&str> {
        self.next_prompt
            .request_id
            .filter(|request_id| self.next_prompt_request_matches(*request_id))
            .and(self.next_prompt.text.as_deref())
    }

    pub(super) fn next_prompt_placeholder(&self) -> Option<String> {
        let accept = if ACCEPT_KEY == KeyCode::Enter {
            "Enter"
        } else {
            "Tab"
        };
        self.next_prompt_text()
            .map(|text| format!("{text}  [{accept} to accept · Esc to dismiss]"))
    }

    /// Accept before normal submit/completion/queueing; never submit a suggestion.
    pub(super) fn handle_next_prompt_key(&mut self, key: KeyEvent) -> Option<(InputResult, bool)> {
        if !self.has_next_prompt() {
            return None;
        }
        if key.code == ACCEPT_KEY
            && key.modifiers.is_empty()
            && let Some(text) = self.next_prompt_text().map(str::to_owned)
        {
            self.dismiss_next_prompt();
            self.draft.textarea.enter_vim_insert_mode();
            self.insert_str(&text);
            return Some((InputResult::None, true));
        }
        let dismiss = key.code == KeyCode::Esc && key.modifiers.is_empty();
        self.dismiss_next_prompt();
        dismiss.then_some((InputResult::None, true))
    }
}

#[cfg(test)]
#[path = "next_prompt_tests.rs"]
mod tests;
