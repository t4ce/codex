//! Reject Bedrock configuration without linking the AWS SDK.

use std::path::PathBuf;
use std::sync::Arc;

use codex_api::Provider;
use codex_api::SharedAuthProvider;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_model_provider_info::ModelProviderInfo;
use codex_models_manager::manager::SharedModelsManager;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;
use codex_protocol::openai_models::ModelsResponse;

use crate::ModelProvider;
use crate::ModelProviderFuture;
use crate::ProviderAccountError;
use crate::ProviderAccountResult;

const DISABLED: &str = "Amazon Bedrock is disabled in this build; rebuild with --features bedrock";

#[derive(Debug)]
pub(crate) struct AmazonBedrockModelProvider(ModelProviderInfo);

impl AmazonBedrockModelProvider {
    pub(crate) fn new(info: ModelProviderInfo, _auth_manager: Option<Arc<AuthManager>>) -> Self {
        Self(info)
    }
}

impl ModelProvider for AmazonBedrockModelProvider {
    fn info(&self) -> &ModelProviderInfo {
        &self.0
    }

    fn auth_manager(&self) -> Option<Arc<AuthManager>> {
        None
    }

    fn auth(&self) -> ModelProviderFuture<'_, Option<CodexAuth>> {
        Box::pin(async { None })
    }

    fn account_state(&self) -> ProviderAccountResult {
        Err(ProviderAccountError::BedrockDisabled)
    }

    fn api_provider(&self) -> ModelProviderFuture<'_, Result<Provider>> {
        Box::pin(async { Err(CodexErr::Fatal(DISABLED.to_string())) })
    }

    fn api_auth(&self) -> ModelProviderFuture<'_, Result<SharedAuthProvider>> {
        Box::pin(async { Err(CodexErr::Fatal(DISABLED.to_string())) })
    }

    fn models_manager(
        &self,
        _codex_home: PathBuf,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager {
        self.models_manager_without_cache(config_model_catalog)
    }
}

pub fn is_supported_amazon_bedrock_region(_region: &str) -> bool {
    false
}

pub fn is_amazon_bedrock_gov_cloud_region(_region: &str) -> bool {
    false
}

pub async fn resolve_amazon_bedrock_region(
    _provider: ModelProviderInfo,
    _auth_manager: Option<Arc<AuthManager>>,
) -> Result<String> {
    Err(CodexErr::Fatal(DISABLED.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::create_model_provider;

    #[tokio::test]
    async fn disabled_bedrock_rejects_requests_and_does_not_inherit_openai_auth() {
        for info in [
            ModelProviderInfo::create_amazon_bedrock_provider(None),
            ModelProviderInfo::create_amazon_bedrock_runtime_provider(None),
        ] {
            let auth =
                AuthManager::from_auth_for_testing(CodexAuth::from_api_key("test-openai-key"));
            let provider = create_model_provider(info, Some(auth));
            assert!(provider.auth_manager().is_none());
            assert!(provider.auth().await.is_none());
            assert_eq!(
                provider.account_state(),
                Err(ProviderAccountError::BedrockDisabled)
            );
            assert!(
                provider
                    .api_provider()
                    .await
                    .err()
                    .expect("Bedrock request must fail")
                    .to_string()
                    .contains(DISABLED)
            );
            assert!(
                provider
                    .api_auth()
                    .await
                    .err()
                    .expect("Bedrock auth must fail")
                    .to_string()
                    .contains(DISABLED)
            );
        }
    }
}
