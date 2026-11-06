//! AWS signing configuration and wire types remain available in every build.
//! The SDK-backed signing engine is opt-in; disabled builds reject signing hooks before
//! resolving trusted credentials.

use crate::credential_broker::env_value;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::ensure;
use codex_utils_paths::absolute_path::AbsolutePathBuf;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;

#[cfg(feature = "aws-signing")]
mod enabled;
#[cfg(feature = "aws-signing")]
pub(crate) use enabled::*;
#[cfg(not(feature = "aws-signing"))]
mod disabled;
#[cfg(not(feature = "aws-signing"))]
pub(crate) use disabled::*;

/// A credential source in the trusted proxy process, never in the executor environment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProxySecretSource {
    EnvVar(String),
    File(String),
}

impl ProxySecretSource {
    pub(crate) fn validate(&self) -> Result<()> {
        match self {
            Self::EnvVar(name) if !name.is_empty() && !name.contains(['=', '\0']) => Ok(()),
            Self::File(path) if std::path::Path::new(path).is_absolute() => Ok(()),
            Self::EnvVar(_) | Self::File(_) => Err(anyhow!("invalid proxy secret source")),
        }
    }

    pub(crate) fn resolve(
        &self,
        env: &impl Fn(&str) -> Option<String>,
        file: &impl Fn(&AbsolutePathBuf) -> Result<String>,
    ) -> Result<String> {
        self.validate()?;
        let value = match self {
            Self::EnvVar(name) => env(name).ok_or_else(|| anyhow!("missing proxy credential"))?,
            Self::File(path) => file(&AbsolutePathBuf::from_absolute_path(path)?)?,
        };
        if value.is_empty() || value.len() > 65_536 {
            return Err(anyhow!("proxy credential is empty or exceeds 64 KiB"));
        }
        Ok(value)
    }
}

/// AWS SigV4 action selected by an exact-host, method, and path MITM hook.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AwsSigV4Config {
    /// Nonsecret access key ID installed in the guest AWS profile.
    pub placeholder_access_key_id: String,
    pub region: String,
    pub service: String,
    pub access_key_id: ProxySecretSource,
    pub secret_access_key: ProxySecretSource,
    pub session_token: Option<ProxySecretSource>,
    #[serde(default = "https_port")]
    pub port: u16,
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
}

pub(crate) fn https_port() -> u16 {
    443
}
pub(crate) fn default_max_body_bytes() -> usize {
    8 * 1024 * 1024
}

impl AwsSigV4Config {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            !self.placeholder_access_key_id.is_empty()
                && self.placeholder_access_key_id.len() <= 128
                && self
                    .placeholder_access_key_id
                    .bytes()
                    .all(|byte| { byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-') }),
            "invalid AWS placeholder access key ID"
        );
        for value in [&self.region, &self.service] {
            if value.is_empty()
                || !value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
            {
                return Err(anyhow!("invalid AWS signing region or service"));
            }
        }
        ensure!(self.port != 0, "invalid AWS signing port");
        ensure!(
            self.max_body_bytes <= 16 * 1024 * 1024,
            "AWS signing body limit exceeds 16 MiB"
        );
        self.access_key_id.validate()?;
        self.secret_access_key.validate()?;
        if let Some(token) = &self.session_token {
            token.validate()?;
        }
        Ok(())
    }
}

const DUMMY_SECRET: &str = "codex-local-aws-secret";
#[cfg(feature = "aws-signing")]
const DUMMY_TOKEN: &str = "codex-local-aws-token";
pub(crate) const AWS_CA_BUNDLE: &str = "AWS_CA_BUNDLE";

pub(crate) fn snapshot_env_keys(env: &HashMap<String, String>) -> Option<[&'static str; 5]> {
    (env_value(env, "AWS_SECRET_ACCESS_KEY") == Some(DUMMY_SECRET)
        || env_value(env, AWS_CA_BUNDLE)
            .is_some_and(crate::certs::is_managed_mitm_ca_trust_bundle_path))
    .then_some([
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AWS_SECURITY_TOKEN",
        AWS_CA_BUNDLE,
    ])
}

#[cfg(all(test, not(feature = "aws-signing")))]
#[path = "aws_sigv4_disabled_tests.rs"]
mod disabled_tests;
