use super::*;

#[derive(Default)]
struct CountingWriter {
    bytes: Vec<u8>,
    writes: usize,
    flushes: usize,
    max_write: Option<usize>,
    fail_after: Option<usize>,
}

impl Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writes += 1;
        if self
            .fail_after
            .is_some_and(|limit| self.bytes.len() >= limit)
        {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "terminal closed"));
        }
        let count = bytes.len().min(self.max_write.unwrap_or(usize::MAX));
        self.bytes.extend_from_slice(&bytes[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

fn styled_diff() -> Vec<DrawCommand> {
    // Match the other capture backends before comparing two encodings in parallel tests.
    crossterm::style::force_color_output(true);
    let before = Buffer::empty(Rect::new(0, 0, 80, 4));
    let mut after = before.clone();
    after.set_string(0, 0, "Codex terminal", Style::default().green().bold());
    after.set_string(
        0,
        1,
        "ready λ",
        Style::default().bg(Color::Blue).underlined(),
    );
    after[(10, 2)].set_symbol("\x1b]8;;https://example.com\x07link\x1b]8;;\x07");
    diff_buffers(&before, &after)
}

#[test]
fn buffered_diff_preserves_ansi_with_one_backend_write_and_no_early_flush() {
    let mut direct = CountingWriter::default();
    draw(&mut direct, styled_diff().into_iter()).expect("direct diff");
    let mut buffered = CountingWriter::default();
    let mut scratch = Vec::new();
    draw_buffered(&mut buffered, styled_diff().into_iter(), &mut scratch).expect("buffered diff");

    assert_eq!(buffered.bytes, direct.bytes);
    assert!(
        direct.writes > 20,
        "fixture must exercise fragmented output"
    );
    assert_eq!(buffered.writes, 1);
    assert_eq!(buffered.flushes, 0, "the frame owner controls flushing");

    let mut parser = vt100::Parser::new(4, 80, 0);
    parser.process(&buffered.bytes);
    assert!(
        parser
            .screen()
            .contents()
            .starts_with("Codex terminal\nready λ")
    );
    if !Colored::ansi_color_disabled_memoized() {
        assert_eq!(
            parser.screen().cell(0, 0).expect("first cell").fgcolor(),
            vt100::Color::Idx(2)
        );
    }
    assert!(parser.screen().cell(0, 0).expect("first cell").bold());

    // Reusing the scratch buffer must replace the preceding frame's bytes.
    buffered.bytes.clear();
    let mut direct_next = Vec::new();
    draw(&mut direct_next, std::iter::empty()).expect("next direct diff");
    draw_buffered(&mut buffered, std::iter::empty(), &mut scratch).expect("next buffered diff");
    assert_eq!(buffered.bytes, direct_next);
}

#[test]
fn buffered_diff_handles_short_writes_and_returns_terminal_errors_without_retry() {
    let mut expected = Vec::new();
    draw(&mut expected, styled_diff().into_iter()).expect("direct diff");
    let mut short = CountingWriter {
        max_write: Some(7),
        ..CountingWriter::default()
    };
    let mut scratch = Vec::new();
    draw_buffered(&mut short, styled_diff().into_iter(), &mut scratch).expect("short writes");
    assert_eq!(short.bytes, expected);

    let mut failed = CountingWriter {
        max_write: Some(7),
        fail_after: Some(7),
        ..CountingWriter::default()
    };
    let error = draw_buffered(&mut failed, styled_diff().into_iter(), &mut scratch)
        .expect_err("closed terminal");
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(failed.bytes, expected[..7]);
    assert_eq!(failed.writes, 2);
    assert_eq!(failed.flushes, 0);
}
