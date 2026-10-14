//! OSS provider utilities shared between TUI and exec.

use codex_core::config::Config;
#[cfg(any(feature = "local-models", test))]
use codex_model_provider_info::LMSTUDIO_OSS_PROVIDER_ID;
#[cfg(any(feature = "local-models", test))]
use codex_model_provider_info::OLLAMA_OSS_PROVIDER_ID;

/// Returns the default model for a given OSS provider.
pub fn get_default_model_for_oss_provider(provider_id: &str) -> Option<&'static str> {
    #[cfg(feature = "local-models")]
    match provider_id {
        LMSTUDIO_OSS_PROVIDER_ID => Some(codex_lmstudio::DEFAULT_OSS_MODEL),
        OLLAMA_OSS_PROVIDER_ID => Some(codex_ollama::DEFAULT_OSS_MODEL),
        _ => None,
    }
    #[cfg(not(feature = "local-models"))]
    {
        let _ = provider_id;
        None
    }
}

/// Checks whether this build includes local-model providers.
pub fn ensure_local_models_enabled() -> Result<(), std::io::Error> {
    if cfg!(feature = "local-models") {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "Local model providers are disabled in this build. Rebuild with the `local-models` Cargo feature enabled.",
        ))
    }
}

/// Ensures the specified OSS provider is ready (models downloaded, service reachable).
pub async fn ensure_oss_provider_ready(
    provider_id: &str,
    config: &Config,
) -> Result<(), std::io::Error> {
    #[cfg(not(feature = "local-models"))]
    {
        let _ = (provider_id, config);
        ensure_local_models_enabled()
    }
    #[cfg(feature = "local-models")]
    {
        match provider_id {
            LMSTUDIO_OSS_PROVIDER_ID => {
                codex_lmstudio::ensure_oss_ready(config)
                    .await
                    .map_err(|e| std::io::Error::other(format!("OSS setup failed: {e}")))?;
            }
            OLLAMA_OSS_PROVIDER_ID => {
                let client = codex_ollama::OllamaClient::try_from_oss_provider(config).await?;
                codex_ollama::ensure_responses_supported(&client).await?;
                codex_ollama::ensure_oss_ready(config, &client)
                    .await
                    .map_err(|e| std::io::Error::other(format!("OSS setup failed: {e}")))?;
            }
            _ => {
                // Unknown provider, skip setup
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_default_model_for_provider_lmstudio() {
        #[cfg(feature = "local-models")]
        let result = get_default_model_for_oss_provider(LMSTUDIO_OSS_PROVIDER_ID);
        #[cfg(feature = "local-models")]
        assert_eq!(result, Some(codex_lmstudio::DEFAULT_OSS_MODEL));
        #[cfg(not(feature = "local-models"))]
        assert_eq!(
            get_default_model_for_oss_provider(LMSTUDIO_OSS_PROVIDER_ID),
            None
        );
    }

    #[test]
    fn test_get_default_model_for_provider_ollama() {
        #[cfg(feature = "local-models")]
        let result = get_default_model_for_oss_provider(OLLAMA_OSS_PROVIDER_ID);
        #[cfg(feature = "local-models")]
        assert_eq!(result, Some(codex_ollama::DEFAULT_OSS_MODEL));
        #[cfg(not(feature = "local-models"))]
        assert_eq!(
            get_default_model_for_oss_provider(OLLAMA_OSS_PROVIDER_ID),
            None
        );
    }

    #[test]
    fn test_get_default_model_for_provider_unknown() {
        let result = get_default_model_for_oss_provider("unknown-provider");
        assert_eq!(result, None);
    }

    #[test]
    fn test_local_models_enabled_check_matches_build_feature() {
        assert_eq!(
            ensure_local_models_enabled().is_ok(),
            cfg!(feature = "local-models")
        );
    }
}
