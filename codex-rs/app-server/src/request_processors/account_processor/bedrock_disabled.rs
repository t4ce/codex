//! Keep protocol compatibility while rejecting unsupported Bedrock setup.

use super::AccountRequestProcessor;
use codex_app_server_protocol::BedrockDiscoverParams;
use codex_app_server_protocol::BedrockSetupParams;
use codex_app_server_protocol::ClientResponsePayload;
use codex_app_server_protocol::JSONRPCErrorError;

impl AccountRequestProcessor {
    pub(crate) async fn bedrock_discover(
        &self,
        _params: BedrockDiscoverParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.ensure_bedrock_login_allowed()?;
        Ok(None)
    }

    pub(crate) async fn bedrock_setup(
        &self,
        _params: BedrockSetupParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.ensure_bedrock_login_allowed()?;
        Ok(None)
    }
}
