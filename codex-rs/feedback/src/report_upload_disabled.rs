//! External-reporting API compatibility for text-only builds.

use std::collections::BTreeMap;

use anyhow::Result;
use anyhow::anyhow;
use codex_protocol::protocol::SessionSource;

use crate::FeedbackAttachment;
use crate::FeedbackSnapshot;

/// The upstream HTTP result, not proof of durable storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedbackDelivery {
    Accepted {
        retry_after: Option<std::time::Duration>,
    },
    Rejected {
        status: u16,
        retry_after: Option<std::time::Duration>,
    },
    Unconfirmed,
}

/// Inert transport retained to keep the public API available without reporting enabled.
pub struct FeedbackTransport {
    _private: (),
}

impl FeedbackTransport {
    pub fn new<T>(_: T) -> Result<Self> {
        Err(anyhow!(
            "external feedback reporting is disabled in this build"
        ))
    }

    pub async fn send(&self, _: Vec<u8>) -> FeedbackDelivery {
        FeedbackDelivery::Unconfirmed
    }
}

impl FeedbackSnapshot {
    pub fn prepare_report_event(
        &self,
        _: &str,
        _: &str,
        _: Option<&str>,
        _: Option<&BTreeMap<String, String>>,
        _: Option<&SessionSource>,
    ) -> Result<Vec<u8>> {
        Err(anyhow!(
            "external feedback reporting is disabled in this build"
        ))
    }
}

pub fn prepare_report_attachment(_: &str, _: FeedbackAttachment) -> Result<Vec<u8>> {
    Err(anyhow!(
        "external feedback reporting is disabled in this build"
    ))
}

#[cfg(test)]
mod tests {
    use crate::CodexFeedback;
    use crate::FeedbackUploadOptions;
    use codex_state::LogWriteFailureReporter;

    #[tokio::test]
    async fn disabled_upload_returns_error_without_an_http_client() {
        let feedback = CodexFeedback::new();
        feedback.report_failure("local crash diagnostic");
        let snapshot = feedback.snapshot(None);
        assert_eq!(snapshot.bytes, b"local crash diagnostic");
        let options = FeedbackUploadOptions {
            classification: "bug",
            reason: None,
            tags: None,
            include_logs: false,
            extra_attachments: Vec::new(),
            extra_attachment_paths: &[],
            session_source: None,
            logs_override: None,
        };

        let error = snapshot
            .upload_feedback(options, &())
            .await
            .expect_err("disabled reporting should return an error");
        assert!(error.to_string().contains("disabled"));
    }
}
