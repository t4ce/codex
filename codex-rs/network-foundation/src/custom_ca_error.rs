use std::io;
use std::path::PathBuf;
use thiserror::Error;

const CA_CERT_HINT: &str = "If you set CODEX_CA_CERTIFICATE or SSL_CERT_FILE, ensure it points to a PEM file containing one or more CERTIFICATE blocks, or unset it to use system roots.";

/// Describes why a transport using shared custom CA support could not be constructed.
///
/// These failure modes apply to both reqwest client construction and websocket TLS
/// configuration. A build can fail because the configured CA file could not be read, could not be
/// parsed as certificates, contained certs that the target TLS stack refused to register, or
/// because the final reqwest client builder failed. Callers that do not care about the
/// distinction can rely on the `From<BuildCustomCaTransportError> for io::Error` conversion.
#[derive(Debug, Error)]
pub enum BuildCustomCaTransportError {
    /// Reading the selected CA file from disk failed before any PEM parsing could happen.
    #[error(
        "Failed to read CA certificate file {} selected by {}: {source}. {hint}",
        path.display(),
        source_env,
        hint = CA_CERT_HINT
    )]
    ReadCaFile {
        source_env: &'static str,
        path: PathBuf,
        source: io::Error,
    },

    /// The selected CA file was readable, but did not produce usable certificate material.
    #[error(
        "Failed to load CA certificates from {} selected by {}: {detail}. {hint}",
        path.display(),
        source_env,
        hint = CA_CERT_HINT
    )]
    InvalidCaFile {
        source_env: &'static str,
        path: PathBuf,
        detail: String,
    },

    /// One parsed certificate block could not be registered with the reqwest client builder.
    #[error(
        "Failed to parse certificate #{certificate_index} from {} selected by {}: {source}. {hint}",
        path.display(),
        source_env,
        hint = CA_CERT_HINT
    )]
    RegisterCertificate {
        source_env: &'static str,
        path: PathBuf,
        certificate_index: usize,
        source: reqwest::Error,
    },

    /// Reqwest rejected the final client configuration after a custom CA bundle was loaded.
    #[error(
        "Failed to build HTTP client while using CA bundle from {} ({}): {source}",
        source_env,
        path.display()
    )]
    BuildClientWithCustomCa {
        source_env: &'static str,
        path: PathBuf,
        #[source]
        source: reqwest::Error,
    },

    /// Reqwest rejected the final client configuration while using only system roots.
    #[error("Failed to build HTTP client while using system root certificates: {0}")]
    BuildClientWithSystemRoots(#[source] reqwest::Error),

    /// One parsed certificate block could not be registered with the websocket TLS root store.
    #[error(
        "Failed to register certificate #{certificate_index} from {} selected by {} in rustls root store: {source}. {hint}",
        path.display(),
        source_env,
        hint = CA_CERT_HINT
    )]
    RegisterRustlsCertificate {
        source_env: &'static str,
        path: PathBuf,
        certificate_index: usize,
        source: rustls::Error,
    },
}

impl From<BuildCustomCaTransportError> for io::Error {
    fn from(error: BuildCustomCaTransportError) -> Self {
        match error {
            BuildCustomCaTransportError::ReadCaFile { ref source, .. } => {
                io::Error::new(source.kind(), error)
            }
            BuildCustomCaTransportError::InvalidCaFile { .. }
            | BuildCustomCaTransportError::RegisterCertificate { .. }
            | BuildCustomCaTransportError::RegisterRustlsCertificate { .. } => {
                io::Error::new(io::ErrorKind::InvalidData, error)
            }
            BuildCustomCaTransportError::BuildClientWithCustomCa { .. }
            | BuildCustomCaTransportError::BuildClientWithSystemRoots(_) => io::Error::other(error),
        }
    }
}
