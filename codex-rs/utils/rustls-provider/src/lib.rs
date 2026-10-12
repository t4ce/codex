use std::sync::Once;

const REQUIRED_SIGNATURE_SCHEME: rustls::SignatureScheme =
    rustls::SignatureScheme::ECDSA_NISTP256_SHA256;

/// Ensures a process-wide rustls crypto provider is installed.
///
/// Selects RustCrypto explicitly even when dependencies also enable `ring`.
pub fn ensure_rustls_crypto_provider() {
    static RUSTLS_PROVIDER_INIT: Once = Once::new();
    RUSTLS_PROVIDER_INIT.call_once(|| {
        // Keep TLS on the vendored RustCrypto implementation.
        if rustls_rustcrypto::provider().install_default().is_err() {
            // Preserve the previous best-effort behavior for embedded hosts that
            // install a process-global provider before Codex can install one.
            return;
        }

        let Some(provider) = rustls::crypto::CryptoProvider::get_default() else {
            panic!("RustCrypto rustls crypto provider should be installed");
        };
        assert!(
            provider_supports_required_signature_scheme(provider),
            "installed rustls crypto provider must support {REQUIRED_SIGNATURE_SCHEME:?}"
        );
    });
}

fn provider_supports_required_signature_scheme(provider: &rustls::crypto::CryptoProvider) -> bool {
    provider
        .signature_verification_algorithms
        .supported_schemes()
        .contains(&REQUIRED_SIGNATURE_SCHEME)
}
