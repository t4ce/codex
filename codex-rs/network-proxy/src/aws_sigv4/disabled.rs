//! No signing engine or SDK credentials exist without the explicit build feature.

use super::AwsSigV4Config;
use anyhow::Result;
use anyhow::anyhow;
use codex_utils_paths::absolute_path::AbsolutePathBuf;
use rama_http::Request;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

const DISABLED: &str = "AWS request signing is disabled in this build; rebuild with --features codex-network-proxy/aws-signing";

// Uninhabited types prevent constructing a signer or producing a signature in this build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AwsSigV4Signer {}
pub(crate) enum GuestSignature {}

impl AwsSigV4Config {
    pub(crate) fn compile(
        &self,
        _env: &impl Fn(&str) -> Option<String>,
        _file: &impl Fn(&AbsolutePathBuf) -> Result<String>,
    ) -> Result<AwsSigV4Signer> {
        self.validate()?;
        Err(anyhow!(DISABLED))
    }
}

impl AwsSigV4Signer {
    pub(crate) fn plan(&self, _req: &Request) -> Result<Option<GuestSignature>> {
        match *self {}
    }

    pub(crate) async fn sign(&self, _req: Request, _guest: &GuestSignature) -> Result<Request> {
        match *self {}
    }
}

pub(crate) fn preserve_signing_headers(_req: &mut Request) {}

pub(crate) fn validate_guest_placeholders<'a>(
    signers: impl Iterator<Item = &'a AwsSigV4Signer> + Clone,
) -> Result<()> {
    for signer in signers {
        match *signer {}
    }
    Ok(())
}

pub(crate) fn mask_local_aws_env(
    _signers: &[Arc<AwsSigV4Signer>],
    _env: &mut HashMap<String, String>,
    _ca_path: Option<&Path>,
) {
}

pub(crate) fn mask_local_aws_text(_signers: &[Arc<AwsSigV4Signer>], _text: &mut String) {}
