//! Feature-off contracts: retain configuration and fail closed before accessing secrets.

use super::*;
use crate::NetworkProxyConfig;
use crate::mitm_hook::MitmHookActionsConfig;
use crate::mitm_hook::MitmHookConfig;
use crate::mitm_hook::MitmHookMatchConfig;
use pretty_assertions::assert_eq;

fn signing() -> AwsSigV4Config {
    AwsSigV4Config {
        placeholder_access_key_id: "MASKEDACCESSKEY".into(),
        region: "us-east-1".into(),
        service: "s3".into(),
        access_key_id: ProxySecretSource::EnvVar("AWS_ACCESS_KEY_ID".into()),
        secret_access_key: ProxySecretSource::EnvVar("AWS_SECRET_ACCESS_KEY".into()),
        session_token: None,
        port: 443,
        max_body_bytes: default_max_body_bytes(),
    }
}

#[test]
fn signing_configuration_keeps_its_wire_schema_without_the_sdk() {
    let config = signing();
    config.validate().unwrap();
    let wire = serde_json::to_value(&config).unwrap();
    assert_eq!(wire["service"], "s3");
    assert_eq!(wire["access_key_id"]["env_var"], "AWS_ACCESS_KEY_ID");
    assert_eq!(
        serde_json::from_value::<AwsSigV4Config>(wire).unwrap(),
        config
    );
    let schema = serde_json::to_value(schemars::schema_for!(AwsSigV4Config)).unwrap();
    assert!(schema["properties"].get("secret_access_key").is_some());
}

#[test]
fn signing_rejection_happens_before_secret_resolution() {
    let error = signing()
        .compile(
            &|_| panic!("disabled signing must not read trusted environment credentials"),
            &|_| panic!("disabled signing must not read trusted secret files"),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("AWS request signing is disabled")
    );
    assert!(
        error
            .to_string()
            .contains("--features codex-network-proxy/aws-signing")
    );
}

#[test]
fn configured_signing_hook_fails_instead_of_forwarding_unsigned() {
    let mut config = NetworkProxyConfig {
        mitm: true,
        mitm_hooks: vec![MitmHookConfig {
            host: "bucket.s3.amazonaws.com".into(),
            matcher: MitmHookMatchConfig {
                schemes: vec!["https".into()],
                ports: vec![443],
                methods: vec!["GET".into()],
                path_prefixes: vec!["/".into()],
                ..Default::default()
            },
            actions: MitmHookActionsConfig {
                aws_sigv4: Some(signing()),
                ..Default::default()
            },
        }],
        ..Default::default()
    };
    config.set_credential_broker_enabled(true);
    let error = crate::mitm_hook::compile_mitm_hooks(&config).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("AWS request signing is disabled"),
        "{error:#}"
    );
}
