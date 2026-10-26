use super::super::tests::new_test_composer;
use super::*;
use crate::render::renderable::Renderable;
use ratatui::buffer::Buffer;

#[test]
fn tab_accepts_without_submitting_and_the_draft_can_be_edited() {
    let (mut composer, _events) = new_test_composer();
    let request = composer.begin_next_prompt().unwrap();
    composer.apply_next_prompt(request, Some("Add the CLI feature".into()));
    assert!(composer.current_text().is_empty());
    assert!(composer.draft_snapshot().text.is_empty());
    let (result, redraw) = composer.handle_key_event(KeyCode::Tab.into());
    assert!(matches!(result, InputResult::None));
    assert!(redraw);
    assert_eq!(composer.current_text(), "Add the CLI feature");
    assert!(!composer.has_next_prompt());
    composer.handle_key_event(KeyCode::Backspace.into());
    assert_eq!(composer.current_text(), "Add the CLI featur");
    let (result, _) = composer.handle_key_event(KeyCode::Enter.into());
    assert!(matches!(result, InputResult::Submitted { text, .. } if text == "Add the CLI featur"));
}

#[test]
fn typing_paste_escape_and_new_turn_invalidate_late_results() {
    for action in 0..5 {
        let (mut composer, _events) = new_test_composer();
        composer.set_disable_paste_burst(true);
        let request = composer.begin_next_prompt().unwrap();
        composer.apply_next_prompt(request, Some("Old suggestion".into()));
        match action {
            0 => {
                composer.handle_key_event(KeyCode::Char('x').into());
            }
            1 => {
                composer.handle_paste("pasted".into());
            }
            2 => {
                composer.handle_key_event(KeyCode::Esc.into());
            }
            3 => {
                composer.set_task_running(true);
                composer.set_task_running(false);
            }
            _ => {
                composer.apply_external_edit("edited".into());
            }
        }
        let before = composer.current_text();
        composer.apply_next_prompt(request, Some("Late suggestion".into()));
        assert_eq!(composer.current_text(), before);
        assert!(composer.next_prompt_text().is_none());
        assert!(!composer.has_next_prompt());
    }
}

#[test]
fn pending_generation_is_invalidated_even_if_typed_text_is_deleted() {
    let (mut composer, _events) = new_test_composer();
    composer.set_disable_paste_burst(true);
    let request = composer.begin_next_prompt().unwrap();
    composer.handle_key_event(KeyCode::Char('x').into());
    composer.handle_key_event(KeyCode::Backspace.into());
    assert!(composer.is_empty());
    composer.apply_next_prompt(request, Some("Stale suggestion".into()));
    assert!(composer.next_prompt_text().is_none());
    let newer = composer.begin_next_prompt().unwrap();
    composer.apply_next_prompt(request, Some("Older result".into()));
    assert!(composer.next_prompt_text().is_none());
    composer.apply_next_prompt(newer, Some("Current result".into()));
    assert_eq!(composer.next_prompt_text(), Some("Current result"));
}

#[test]
fn enter_never_submits_placeholder_and_existing_drafts_are_preserved() {
    let (mut composer, _events) = new_test_composer();
    let request = composer.begin_next_prompt().unwrap();
    composer.apply_next_prompt(request, Some("Not yet accepted".into()));
    assert!(matches!(
        composer.handle_key_event(KeyCode::Enter.into()).0,
        InputResult::None
    ));
    assert!(composer.is_empty());
    composer.apply_external_edit("My draft".into());
    assert!(composer.begin_next_prompt().is_none());
    assert_eq!(composer.current_text(), "My draft");
}

#[test]
fn suggestion_wraps_in_the_composer_without_becoming_draft_text() {
    let (mut composer, _events) = new_test_composer();
    let request = composer.begin_next_prompt().unwrap();
    composer.apply_next_prompt(
        request,
        Some("Add next prompt suggestions for both systems".into()),
    );
    for width in [32, 80, 120] {
        let area = Rect::new(0, 0, width, composer.desired_height(width));
        let mut buffer = Buffer::empty(area);
        composer.render(area, &mut buffer);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Add next prompt"));
        if width >= 80 {
            assert!(text.contains("Tab to accept"));
        }
        assert!(composer.current_text().is_empty());
    }
}
