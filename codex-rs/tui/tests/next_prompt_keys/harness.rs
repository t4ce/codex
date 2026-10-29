//! Adapters for the real next_prompt.rs state machine, without a terminal.
#![allow(dead_code)]

extern crate self as uuid;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Uuid(u64);
impl Uuid {
    fn new_v4() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KeyCode {
    Enter,
    Tab,
    Esc,
    Backspace,
    Char(char),
}
#[derive(Clone, Copy)]
struct KeyModifiers(u8);
impl KeyModifiers {
    const NONE: Self = Self(0);
    const SHIFT: Self = Self(1);
    fn is_empty(self) -> bool {
        self.0 == 0
    }
}
#[derive(Clone, Copy)]
struct KeyEvent {
    code: KeyCode,
    modifiers: KeyModifiers,
}
impl From<KeyCode> for KeyEvent {
    fn from(code: KeyCode) -> Self {
        Self {
            code,
            modifiers: KeyModifiers::NONE,
        }
    }
}
#[derive(Debug, Eq, PartialEq)]
enum InputResult {
    None,
}
enum AppEvent {
    DismissNextPrompt { request_id: Uuid },
}
struct Sender;
impl Sender {
    fn send(&self, _: AppEvent) {}
}
#[derive(Default)]
struct Textarea {
    text: String,
    insert_mode: bool,
}
impl Textarea {
    fn enter_vim_insert_mode(&mut self) {
        self.insert_mode = true;
    }
}
struct Draft {
    input_enabled: bool,
    textarea: Textarea,
}
struct ChatComposer {
    next_prompt: next_prompt::NextPromptState,
    draft: Draft,
    app_event_tx: Sender,
    is_task_running: bool,
    blocks_direct_input: bool,
    popup: bool,
    paste: bool,
}
impl ChatComposer {
    fn new() -> Self {
        Self {
            next_prompt: Default::default(),
            draft: Draft {
                input_enabled: true,
                textarea: Default::default(),
            },
            app_event_tx: Sender,
            is_task_running: false,
            blocks_direct_input: false,
            popup: false,
            paste: false,
        }
    }
    fn is_empty(&self) -> bool {
        self.draft.textarea.text.is_empty()
    }
    fn popup_active(&self) -> bool {
        self.popup
    }
    fn is_in_paste_burst(&self) -> bool {
        self.paste
    }
    fn dismiss_sparkle(&mut self) {}
    fn insert_str(&mut self, text: &str) {
        self.draft.textarea.text.push_str(text);
    }
    fn suggest(&mut self) -> Uuid {
        let id = self.begin_next_prompt().unwrap();
        self.apply_next_prompt(id, Some("One complete follow-up §".into()));
        id
    }
}
mod next_prompt;

#[test]
fn platform_key_accepts_once_without_submitting_and_updates_the_hint() {
    let mut composer = ChatComposer::new();
    composer.suggest();
    let (key, label) = if cfg!(target_os = "trueos") {
        (KeyCode::Enter, "Enter to accept")
    } else {
        (KeyCode::Tab, "Tab to accept")
    };
    assert!(composer.next_prompt_placeholder().unwrap().contains(label));
    assert_eq!(
        composer.handle_next_prompt_key(key.into()),
        Some((InputResult::None, true))
    );
    assert_eq!(composer.draft.textarea.text, "One complete follow-up §");
    assert!(composer.draft.textarea.insert_mode);
    assert!(!composer.has_next_prompt());
    // The second Enter is passed to the normal composer submit handler.
    assert_eq!(composer.handle_next_prompt_key(KeyCode::Enter.into()), None);
}

#[test]
fn escape_backspace_and_typing_dismiss_and_reject_late_results() {
    for key in [KeyCode::Esc, KeyCode::Backspace, KeyCode::Char('x')] {
        let mut composer = ChatComposer::new();
        let id = composer.suggest();
        let result = composer.handle_next_prompt_key(key.into());
        assert_eq!(
            result,
            (key == KeyCode::Esc).then_some((InputResult::None, true))
        );
        assert!(composer.is_empty());
        composer.apply_next_prompt(id, Some("Late result".into()));
        assert!(!composer.has_next_prompt());
        assert!(composer.next_prompt_placeholder().is_none());
    }
}

#[test]
fn shifted_enter_does_not_accept_and_an_existing_draft_is_never_replaced() {
    let mut composer = ChatComposer::new();
    composer.suggest();
    assert_eq!(
        composer.handle_next_prompt_key(KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::SHIFT,
        }),
        None
    );
    assert!(composer.is_empty());
    composer.insert_str("My own draft");
    assert!(composer.begin_next_prompt().is_none());
    assert_eq!(composer.draft.textarea.text, "My own draft");
}

#[test]
fn plain_enter_remains_a_nonaccepting_key_on_other_platforms() {
    if cfg!(target_os = "trueos") {
        return;
    }
    let mut composer = ChatComposer::new();
    composer.suggest();
    assert_eq!(composer.handle_next_prompt_key(KeyCode::Enter.into()), None);
    assert!(composer.is_empty());
    assert!(!composer.has_next_prompt());
}
