//! Clipboard backend boundary for builds without system clipboard integration.
//! Keep the worker and UI handlers intact so a platform API can replace these operations.

use super::CopyFormat;
use super::CopyOutcome;

const UNAVAILABLE: &str = "system clipboard backend unavailable in this build";

pub(super) fn copy_to_clipboard(
    _text: &str,
    _format: CopyFormat,
    _begin_delivery: impl Fn() -> Result<(), String>,
    _osc52: impl Fn(&str) -> Result<(), String>,
) -> Result<CopyOutcome, String> {
    Err(UNAVAILABLE.into())
}

pub(super) fn osc52_sequence(_text: &str, _tmux: bool) -> Result<String, String> {
    Err(UNAVAILABLE.into())
}

pub(super) fn osc52_copy(_text: &str) -> Result<(), String> {
    Err(UNAVAILABLE.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_copy_never_calls_delivery_or_terminal_backends() {
        let result = copy_to_clipboard(
            "hello",
            CopyFormat::PlainText,
            || panic!("disabled backend must not start delivery"),
            |_| panic!("disabled backend must not write to the terminal"),
        );
        assert_eq!(result.err().as_deref(), Some(UNAVAILABLE));
        assert!(osc52_sequence("hello", false).is_err());
        assert!(osc52_copy("hello").is_err());
        assert!(!crate::clipboard_copy::primary::available());
    }

    #[test]
    fn disabled_reads_and_image_paste_report_unavailable() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut read = crate::clipboard_paste::text::reader();
        assert_eq!(read(deadline).err().as_deref(), Some(UNAVAILABLE));
        assert!(crate::clipboard_copy::primary::read(deadline).is_err());
        assert!(crate::clipboard_paste::paste_image_as_png().is_err());
        assert!(crate::clipboard_paste::paste_image_to_temp_png().is_err());
    }
}
