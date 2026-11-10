//! Host regression checks for the production terminal lifecycle policy.
//! Run with rustc --test tools/terminal-status-validation.rs -o /tmp/codex-terminal-status-tests
//! and then run that binary. No Codex workspace dependencies are required.

#[allow(dead_code)]
mod terminal_program_status {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum ProgramStatus {
        Idle,
        Working,
        Blocked(ProgramStatusKind),
        Done,
        Error,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum ProgramStatusKind {
        Permission,
        Question,
        Auth,
    }
}

#[path = "../codex-rs/tui/src/chatwidget/status_surfaces/program_status.rs"]
mod program_status;

use program_status::acknowledged_status;
use program_status::desired_status;
use terminal_program_status::ProgramStatus;
use terminal_program_status::ProgramStatusKind;

#[test]
fn retained_shell_preserves_completion_acknowledgement_and_follow_up() {
    let mut after_turn = ProgramStatus::Idle;
    // The same open Shell3 session survives all these conversation transitions.
    let native = |blocked, turn_active, after_turn| {
        desired_status(blocked, turn_active, true, false, after_turn, true)
    };

    assert_eq!(native(None, true, after_turn), ProgramStatus::Working);
    after_turn = ProgramStatus::Done;
    let completed = native(None, false, after_turn);
    assert_eq!(completed, ProgramStatus::Done);
    after_turn = acknowledged_status(completed).expect("completion can be acknowledged");
    assert_eq!(native(None, false, after_turn), ProgramStatus::Idle);
    assert_eq!(native(None, true, after_turn), ProgramStatus::Working);
    assert_eq!(
        native(Some(ProgramStatusKind::Permission), true, after_turn),
        ProgramStatus::Blocked(ProgramStatusKind::Permission),
    );
}

#[test]
fn retained_shell_does_not_hide_a_failed_turn() {
    let reported = desired_status(None, false, true, false, ProgramStatus::Error, true);
    assert_eq!(reported, ProgramStatus::Error);
    let acknowledged = acknowledged_status(reported).expect("failure can be acknowledged");
    assert_eq!(
        desired_status(None, false, true, false, acknowledged, true),
        ProgramStatus::Idle,
    );
}

#[test]
fn pending_work_and_active_goals_still_report_working() {
    assert_eq!(
        desired_status(None, true, true, false, ProgramStatus::Done, true),
        ProgramStatus::Working,
    );
    let goal = desired_status(None, false, true, true, ProgramStatus::Done, true);
    assert_eq!(goal, ProgramStatus::Working);
    assert_eq!(acknowledged_status(goal), None);
    assert_eq!(
        desired_status(
            Some(ProgramStatusKind::Question),
            true,
            true,
            true,
            ProgramStatus::Done,
            true,
        ),
        ProgramStatus::Blocked(ProgramStatusKind::Question),
    );
}

#[test]
fn other_platforms_retain_the_existing_background_work_policy() {
    for after_turn in [
        ProgramStatus::Idle,
        ProgramStatus::Done,
        ProgramStatus::Error,
    ] {
        assert_eq!(
            desired_status(None, false, true, false, after_turn, false),
            ProgramStatus::Working,
        );
        assert_eq!(
            desired_status(None, false, false, false, after_turn, false),
            after_turn,
        );
    }
}
