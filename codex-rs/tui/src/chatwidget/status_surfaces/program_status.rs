//! Terminal lifecycle status, independent of rendering and terminal transport.

use crate::terminal_program_status::ProgramStatus;
use crate::terminal_program_status::ProgramStatusKind;

pub(super) fn desired_status(
    blocked: Option<ProgramStatusKind>,
    turn_active: bool,
    has_background_terminals: bool,
    goal_active: bool,
    after_turn: ProgramStatus,
    native_shell: bool,
) -> ProgramStatus {
    if let Some(kind) = blocked {
        ProgramStatus::Blocked(kind)
    } else if turn_active
        // A retained Shell3 session can be waiting for input after the turn.
        // Its lifetime must not hide completion or acknowledgement from Spirit.
        || (!native_shell && has_background_terminals)
        || (after_turn == ProgramStatus::Done && goal_active)
    {
        ProgramStatus::Working
    } else {
        after_turn
    }
}

pub(super) fn acknowledged_status(status: ProgramStatus) -> Option<ProgramStatus> {
    matches!(status, ProgramStatus::Done | ProgramStatus::Error).then_some(ProgramStatus::Idle)
}
