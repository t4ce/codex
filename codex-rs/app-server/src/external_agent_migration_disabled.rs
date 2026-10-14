use crate::error_code::invalid_request;
use crate::outgoing_message::ConnectionRequestId;
use codex_app_server_protocol::ExternalAgentConfigDetectParams;
use codex_app_server_protocol::ExternalAgentConfigDetectResponse;
use codex_app_server_protocol::ExternalAgentConfigImportHistoriesReadResponse;
use codex_app_server_protocol::ExternalAgentConfigImportHistoryRecordParams;
use codex_app_server_protocol::ExternalAgentConfigImportHistoryRecordResponse;
use codex_app_server_protocol::ExternalAgentConfigImportParams;
use codex_app_server_protocol::JSONRPCErrorError;

const DISABLED_MESSAGE: &str = "External agent config import is disabled in this build. Rebuild with the `external-agent-imports` Cargo feature enabled.";

#[derive(Clone)]
pub(crate) struct ExternalAgentConfigRequestProcessor;

impl ExternalAgentConfigRequestProcessor {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) async fn detect(
        &self,
        _params: ExternalAgentConfigDetectParams,
    ) -> Result<ExternalAgentConfigDetectResponse, JSONRPCErrorError> {
        Err(invalid_request(DISABLED_MESSAGE))
    }

    pub(crate) async fn import(
        &self,
        _request_id: ConnectionRequestId,
        _params: ExternalAgentConfigImportParams,
    ) -> Result<(), JSONRPCErrorError> {
        Err(invalid_request(DISABLED_MESSAGE))
    }

    pub(crate) async fn record_import_history(
        &self,
        _params: ExternalAgentConfigImportHistoryRecordParams,
    ) -> Result<ExternalAgentConfigImportHistoryRecordResponse, JSONRPCErrorError> {
        Err(invalid_request(DISABLED_MESSAGE))
    }

    pub(crate) async fn read_import_histories(
        &self,
    ) -> Result<ExternalAgentConfigImportHistoriesReadResponse, JSONRPCErrorError> {
        Err(invalid_request(DISABLED_MESSAGE))
    }
}
