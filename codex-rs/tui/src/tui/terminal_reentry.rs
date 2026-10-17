//! Restore presentation after Shell3 hands the terminal back to this app.

use std::io;

use super::Tui;

unsafe extern "C" {
    fn trueos_cabi_blueprint_terminal_surface_snapshot_v1(
        generation: *mut u64,
        columns: *mut u32,
        rows: *mut u32,
    ) -> i32;
}

pub(super) fn current_generation() -> io::Result<Option<u64>> {
    let (mut generation, mut columns, mut rows) = (0, 0, 0);
    let status = unsafe {
        trueos_cabi_blueprint_terminal_surface_snapshot_v1(&mut generation, &mut columns, &mut rows)
    };
    match status {
        0 if generation != 0 && columns != 0 && rows != 0 => Ok(Some(generation)),
        // Parking leaves Shell3 interactive until an explicit reentry request.
        -5..=-1 => Ok(None),
        _ => Err(io::Error::other(format!(
            "invalid TRUEOS terminal surface (status {status}, generation {generation})"
        ))),
    }
}

impl Tui {
    pub(super) fn restore_terminal_surface(&mut self) -> io::Result<()> {
        let Some(generation) = current_generation()? else {
            return Ok(());
        };
        if self
            .terminal_generation
            .is_some_and(|previous| previous != generation)
        {
            // Shell3 resets modes and clears the display at the ownership
            // boundary. Pair the old keyboard stacks before restoring modes,
            // then invalidate retained cells even when dimensions are equal.
            let was_alt_screen = self.is_alt_screen_active();
            if was_alt_screen {
                self.leave_alt_screen_for_handoff()?;
            }
            super::restore()?;
            super::set_modes()?;
            if was_alt_screen {
                self.enter_alt_screen()?;
            }
            self.terminal.invalidate_cursor_state();
            self.terminal.clear()?;
            crate::terminal_program_status::invalidate_terminal_program_status();
            self.frame_requester.schedule_frame();
        }
        self.terminal_generation = Some(generation);
        Ok(())
    }
}
