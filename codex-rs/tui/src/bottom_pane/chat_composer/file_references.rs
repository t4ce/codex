//! Stage UI4 paths as ordinary atomic text elements, retaining existing rich draft state.

use super::*;

impl ChatComposer {
    pub(crate) fn attach_file_references(&mut self, paths: &[String]) -> bool {
        let snapshot = self.recovery_snapshot();
        // References belong to a user message, never shell commands or command arguments.
        if !self.input_enabled()
            || self.draft.is_bash_mode
            || snapshot.text.trim_start().starts_with('/')
            || self.query_active()
        {
            return false;
        }
        let Some(labels) = crate::file_references::plan(
            &snapshot
                .text_elements
                .iter()
                .filter_map(|e| e.placeholder(&snapshot.text).map(str::to_owned))
                .collect::<Vec<_>>(),
            paths,
        ) else {
            return false;
        };
        if labels.is_empty() {
            return true;
        }
        self.flush_pending_paste_burst();
        self.dismiss_next_prompt();
        self.dismiss_sparkle();
        let started_vim_edit = self.begin_direct_vim_edit();
        // Append rather than replacing a selection or splitting an existing rich element.
        self.draft
            .textarea
            .set_cursor(self.draft.textarea.text().len());
        if !self.draft.textarea.text().is_empty()
            && !self.draft.textarea.text().ends_with(char::is_whitespace)
        {
            self.draft.textarea.insert_str(" ");
        }
        for label in labels {
            self.draft.textarea.insert_element(&label);
            self.draft.textarea.insert_str(" ");
        }
        self.sync_popups();
        if started_vim_edit {
            self.finish_vim_edit();
        }
        true
    }
}
