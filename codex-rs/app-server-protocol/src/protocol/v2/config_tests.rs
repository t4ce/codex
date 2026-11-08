use super::BrowserUseRequirements;
use super::ConfigLayerSource;
use super::RequestHeader;
use codex_utils_paths::absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use serde_json::json;
use ts_rs::TS;

#[test]
fn packaged_defaults_file_origin_preserves_the_existing_wire_shape() {
    let file = AbsolutePathBuf::from_absolute_path(std::env::current_exe().unwrap()).unwrap();
    let source = ConfigLayerSource::PackagedDefaults {
        file: Some(file.clone()),
    };
    let wire = json!({ "type": "packagedDefaults", "file": file });
    assert_eq!(serde_json::to_value(&source).unwrap(), wire);
    assert_eq!(
        serde_json::from_value::<ConfigLayerSource>(wire).unwrap(),
        source
    );
    assert_eq!(source.precedence(), -10);
}

#[test]
fn packaged_defaults_embedded_origin_is_null_without_an_invented_path() {
    let source = ConfigLayerSource::PackagedDefaults { file: None };
    let wire = json!({ "type": "packagedDefaults", "file": null });
    assert_eq!(serde_json::to_value(&source).unwrap(), wire);
    assert_eq!(
        serde_json::from_value::<ConfigLayerSource>(wire).unwrap(),
        source
    );
    assert_eq!(
        serde_json::from_value::<ConfigLayerSource>(json!({ "type": "packagedDefaults" })).unwrap(),
        source,
    );
    assert_eq!(source.precedence(), -10);
}

#[test]
fn packaged_defaults_schema_and_typescript_allow_a_fileless_origin() {
    let schema = serde_json::to_value(schemars::schema_for!(ConfigLayerSource)).unwrap();
    let packaged_defaults = schema["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|variant| variant["properties"]["type"]["enum"] == json!(["packagedDefaults"]))
        .unwrap();
    assert!(
        packaged_defaults["properties"]["file"]["anyOf"]
            .as_array()
            .unwrap()
            .iter()
            .any(|schema| schema["type"] == "null")
    );
    assert!(
        !packaged_defaults["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field == "file")
    );
    assert!(ConfigLayerSource::decl().contains("file: AbsolutePathBuf | null"));
}

#[test]
fn request_header_debug_redacts_values_without_changing_serialization() {
    let wire = json!({ "name": "x-company-token", "value": "private-header-token" });
    let header: RequestHeader = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(&header).unwrap(), wire);
    assert_eq!(
        format!("{header:?}"),
        r#"RequestHeader { name: "x-company-token", value: "[REDACTED]" }"#
    );
    let requirements: BrowserUseRequirements = serde_json::from_value(json!({
        "extension": { "requestHeaders": [wire] }
    }))
    .unwrap();
    for debug in [format!("{requirements:?}"), format!("{requirements:#?}")] {
        assert!(!debug.contains("private-header-token"));
        assert!(debug.contains("x-company-token"));
        assert!(debug.contains("[REDACTED]"));
    }
    assert_eq!(
        serde_json::to_value(&requirements).unwrap()["extension"]["requestHeaders"],
        json!([wire])
    );
}
