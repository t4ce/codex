use codex_utils_rustls_provider::ensure_rustls_crypto_provider;

#[test]
fn ensure_provider_installs_ecdsa_p256_sha256_support() {
    ensure_rustls_crypto_provider();

    let Some(provider) = rustls::crypto::CryptoProvider::get_default() else {
        panic!("rustls provider should be installed");
    };
    assert!(
        provider
            .signature_verification_algorithms
            .supported_schemes()
            .contains(&rustls::SignatureScheme::ECDSA_NISTP256_SHA256)
    );
}
