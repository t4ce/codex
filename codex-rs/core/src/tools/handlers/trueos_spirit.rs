//! Direct access to Spirit's resident avatar and paired UI4 input devices.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use codex_protocol::models::DEFAULT_IMAGE_DETAIL;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageReference;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::openai_models::InputModality;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use codex_utils_image::data_url_from_bytes;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use tokio::time::Instant;
use trueos::spirit;

use super::trueos_help::SPIRIT_SHARE_WINDOW;
use super::trueos_spirit_filmstrip::FilmstripRequest;
use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::selected_tool_environment;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

// Observe reads several pieces of a VM-scoped snapshot. Serialize it with
// focus and other Spirit calls in this process, releasing between samples.
static SPIRIT_CALL: Mutex<()> = Mutex::new(());
const UI4_RETRY_DELAY: Duration = Duration::from_millis(50);
const UI4_RETRY_TIMEOUT: Duration = Duration::from_secs(8);
const NAMED_KEYS: &[(&str, spirit::Ui4Key)] = &[
    ("enter", spirit::Ui4Key::Enter),
    ("escape", spirit::Ui4Key::Escape),
    ("tab", spirit::Ui4Key::Tab),
    ("space", spirit::Ui4Key::Space),
    ("backspace", spirit::Ui4Key::Backspace),
    ("up", spirit::Ui4Key::Up),
    ("down", spirit::Ui4Key::Down),
    ("left", spirit::Ui4Key::Left),
    ("right", spirit::Ui4Key::Right),
    ("delete", spirit::Ui4Key::Delete),
    ("home", spirit::Ui4Key::Home),
    ("end", spirit::Ui4Key::End),
    ("page_up", spirit::Ui4Key::PageUp),
    ("page_down", spirit::Ui4Key::PageDown),
];

pub struct SpiritHandler {
    include_environment_id: bool,
}

impl SpiritHandler {
    pub(crate) fn new(include_environment_id: bool) -> Self {
        Self {
            include_environment_id,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum SpiritAction {
    Windows {},
    Focus {
        window_id: String,
    },
    Observe {
        window_id: Option<String>,
    },
    Filmstrip(FilmstripRequest),
    Pointer {
        x: u16,
        y: u16,
        kind: PointerKind,
    },
    Type {
        text: String,
    },
    Key {
        key: String,
        #[serde(default)]
        modifiers: Vec<KeyModifier>,
    },
    Move {
        x: f32,
        y: f32,
    },
    Emotion {
        idea: Emotion,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PointerKind {
    Move,
    Click,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum KeyModifier {
    Control,
    Shift,
    Alt,
    Meta,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Emotion {
    Anger,
    Disgust,
    Fear,
    Joy,
    Sadness,
    Surprise,
}

impl SpiritAction {
    fn name(&self) -> &'static str {
        match self {
            Self::Windows {} => "windows",
            Self::Focus { .. } => "focus",
            Self::Observe { .. } => "observe",
            Self::Filmstrip(_) => "filmstrip",
            Self::Pointer { .. } => "pointer",
            Self::Type { .. } => "type",
            Self::Key { .. } => "key",
            Self::Move { .. } => "move",
            Self::Emotion { .. } => "emotion",
        }
    }

    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Focus { window_id }
            | Self::Observe { window_id: Some(window_id) }
                if window_id.parse::<u32>().ok().is_none_or(|id| id == 0) =>
                Err("window_id must be a nonzero decimal id returned by spirit action=windows".into()),
            Self::Pointer { x, y, .. } if *x > 1000 || *y > 1000 =>
                Err("pointer coordinates use the observed selected-window grid, inclusive 0..1000".into()),
            Self::Move { x, y } if !x.is_finite() || !y.is_finite() || !(0.0..=1.0).contains(x) || !(0.0..=1.0).contains(y) =>
                Err("move coordinates position the hardware avatar on the scanout, inclusive 0..1".into()),
            Self::Type { text } if text.is_empty() || text.len() > 1024 || text.chars().count() > 64 =>
                Err("type accepts 1..64 Unicode scalars and at most 1024 UTF-8 bytes per call; send longer text in ordered chunks".into()),
            Self::Key { key, .. } if native_key(key).is_none() =>
                Err("key must be a named key or a lowercase ASCII letter a..z".into()),
            _ => Ok(()),
        }
    }
}

fn parse_request(arguments: &str) -> Result<(SpiritAction, Option<String>), FunctionCallError> {
    let mut value: Value = parse_arguments(arguments)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| error("spirit arguments must be an object"))?;
    let environment_id = match object.remove("environment_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) => Some(id),
        _ => return Err(error("environment_id must be a string")),
    };
    let action: SpiritAction = serde_json::from_value(value).map_err(error)?;
    action.validate().map_err(error)?;
    Ok((action, environment_id))
}

fn native_key(key: &str) -> Option<spirit::Ui4Key> {
    if let [letter @ b'a'..=b'z'] = key.as_bytes() {
        return Some(spirit::Ui4Key::Letter(char::from(*letter)));
    }
    NAMED_KEYS
        .iter()
        .find_map(|(name, native)| (*name == key).then_some(*native))
}

fn names(values: &[&str], description: &str) -> JsonSchema {
    JsonSchema::string_enum(
        values.iter().map(|value| json!(value)).collect(),
        Some(description.into()),
    )
}

fn spirit_spec(include_environment_id: bool) -> ToolSpec {
    let mut variants = Vec::new();
    let keys = NAMED_KEYS
        .iter()
        .map(|(name, _)| json!(name))
        .chain(('a'..='z').map(|letter| json!(letter)))
        .collect();
    for (action, fields) in [
        ("windows", vec![]),
        ("focus", vec![("window_id", JsonSchema::string(Some("Decimal window id from windows inventory.".into())))]),
        ("observe", vec![("window_id", JsonSchema::string(Some("Decimal window id from windows. Omit to refresh the selected window. When provided, select the window before capturing it.".into())))]),
        ("filmstrip", vec![
            ("duration_seconds", JsonSchema::integer(Some("Capture duration from 1 to 10 seconds.".into()))),
            ("frames", JsonSchema::integer(Some("8 to 16 evenly spaced samples. Defaults to 12. Fewer frames give each tile more space.".into()))),
        ]),
        ("pointer", vec![
            ("x", JsonSchema::integer(Some("Selected-window horizontal grid coordinate, inclusive 0..1000.".into()))),
            ("y", JsonSchema::integer(Some("Selected-window vertical grid coordinate, inclusive 0..1000.".into()))),
            ("kind", names(&["move", "click"], "Move Spirit's software pointer or queue one primary click.")),
        ]),
        ("type", vec![("text", JsonSchema::string(Some("1..64 Unicode scalars, at most 1024 UTF-8 bytes. Input is queued through Spirit's paired keyboard.".into())))]),
        ("key", vec![
            ("key", JsonSchema::string_enum(keys, Some("One named key or lowercase letter on Spirit's paired keyboard.".into()))),
            ("modifiers", JsonSchema::array(
                names(&["control", "shift", "alt", "meta"], "Keyboard modifier."),
                Some("Optional held modifiers. For Alt+R use key r and modifiers [alt].".into()),
            )),
        ]),
        ("move", vec![
            ("x", JsonSchema::number(Some("Hardware avatar horizontal scanout position, inclusive 0..1.".into()))),
            ("y", JsonSchema::number(Some("Hardware avatar vertical scanout position, inclusive 0..1.".into()))),
        ]),
        ("emotion", vec![("idea", names(&["anger", "disgust", "fear", "joy", "sadness", "surprise"], "Semantic emotion; Spirit chooses Lilly's expression."))]),
    ] {
        let mut properties = BTreeMap::from([("action".into(), names(&[action], "Native Spirit action."))]);
        let mut required = vec!["action".into()];
        for (name, schema) in fields {
            properties.insert(name.into(), schema);
            if !matches!((action, name), ("filmstrip", "frames") | ("observe", "window_id") | ("key", "modifiers")) {
                required.push(name.into());
            }
        }
        if include_environment_id {
            properties.insert("environment_id".into(), JsonSchema::string(Some("Environment id from <environment_context>; the environment must be local TRUEOS.".into())));
        }
        variants.push(JsonSchema::object(properties, Some(required), Some(false.into())));
    }
    // Keep the root an object; per-action objects reject fields from other actions.
    let mut parameters = JsonSchema::object(BTreeMap::new(), None, None);
    parameters.one_of = Some(variants.into());
    ToolSpec::Function(ResponsesApiTool {
        name: "spirit".into(),
        description: "Use this native tool first for open TRUEOS apps. Call windows for visible UI4 windows shared with Spirit, then observe with window_id to select and see the target in one call. Send pointer/type/key as needed and observe again to verify the visible result. Pointer moves or clicks within the observed window's 0..1000 grid. The separate move action positions Lilly herself in 0..1 scanout coordinates. For a shortcut, send key with modifiers. Filmstrip samples the selected window over 1 to 10 seconds, and emotion requests a Lilly expression. Focus is also available on its own. Observe again after a selection or geometry change. Input actions queue once. Calls wait briefly for earlier input to drain, and BUSY means this call queued nothing. Only one agent should drive the shared UI at a time. Spirit works independently of app MCP and Document Control.".into(),
        strict: false,
        defer_loading: None,
        parameters,
        output_schema: None,
    })
}

fn error(message: impl std::fmt::Display) -> FunctionCallError {
    FunctionCallError::RespondToModel(message.to_string())
}

fn native_error(action: &SpiritAction, code: spirit::Error) -> FunctionCallError {
    let advice = match code {
        spirit::UI4_BUSY => {
            "earlier Spirit input is still draining; this call queued nothing; wait and retry this call"
        }
        spirit::UI4_BAD_STATE => {
            "call observe with the target window_id again before sending pointer or keyboard input"
        }
        spirit::UI4_NOT_FOUND => {
            "target is no longer available; refresh windows and select a live target"
        }
        spirit::UI4_DENIED
            if !matches!(
                action,
                SpiritAction::Windows {} | SpiritAction::Move { .. } | SpiritAction::Emotion { .. }
            ) =>
        {
            return error(format!(
                "spirit {} lost or was denied native access (code {}). Check windows again. If the target is missing, share it from the desktop. {SPIRIT_SHARE_WINDOW}",
                action.name(),
                code.0,
            ));
        }
        spirit::UI4_DENIED => "the running Blueprint does not have native Spirit UI access",
        spirit::UI4_BAD_INPUT => "the native API rejected this action's input",
        spirit::UI4_UNAVAILABLE => "the native Spirit service or target capture is unavailable",
        _ => {
            "native operation failed; inspect state before retrying input because completion is unknown"
        }
    };
    error(format!(
        "spirit {} failed (code {}): {advice}",
        action.name(),
        code.0
    ))
}

struct SpiritOutput {
    value: Value,
    image_url: Option<String>,
}

fn with_spirit<T>(call: impl FnOnce() -> T) -> T {
    let _call = SPIRIT_CALL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    call()
}

async fn wait_for_ui4<T>(
    mut call: impl FnMut() -> Result<T, spirit::Error>,
) -> Result<T, spirit::Error> {
    let deadline = Instant::now() + UI4_RETRY_TIMEOUT;
    loop {
        match with_spirit(&mut call) {
            Err(spirit::UI4_BUSY) if Instant::now() < deadline => {
                tokio::time::sleep_until((Instant::now() + UI4_RETRY_DELAY).min(deadline)).await;
            }
            result => return result,
        }
    }
}

async fn execute(action: &SpiritAction) -> Result<SpiritOutput, FunctionCallError> {
    let mut value = json!({"backend":"trueos_spirit", "action":action.name()});
    let mut image_url = None;
    match action {
        SpiritAction::Windows {} => {
            let inventory =
                with_spirit(spirit::ui4_windows).map_err(|code| native_error(action, code))?;
            value["inventory"] = serde_json::from_str(&inventory).map_err(error)?;
            value["scope"] = json!("Visible UI4 windows shared with Spirit");
            value["share_window"] = json!(SPIRIT_SHARE_WINDOW);
        }
        SpiritAction::Observe { window_id } => {
            let window_id = window_id
                .as_deref()
                .map(str::parse::<u64>)
                .transpose()
                .map_err(error)?;
            let observation = wait_for_ui4(|| {
                if let Some(id) = window_id {
                    spirit::ui4_focus(id)?;
                }
                spirit::ui4_observe()
            })
            .await
            .map_err(|code| native_error(action, code))?;
            if !observation.png.starts_with(b"\x89PNG\r\n\x1a\n") {
                return Err(error(
                    "native Spirit observation did not contain a PNG image",
                ));
            }
            let metadata: Value = serde_json::from_str(&observation.metadata).map_err(error)?;
            if let Some(id) = window_id
                && metadata["id"].as_str().and_then(|id| id.parse().ok()) != Some(id)
            {
                return Err(error(
                    "Spirit's selected window changed during capture; observe again with the target window_id",
                ));
            }
            value["observation"] = metadata;
            image_url = Some(data_url_from_bytes("image/png", &observation.png));
        }
        SpiritAction::Focus { window_id } => {
            let window_id = window_id.parse().map_err(error)?;
            wait_for_ui4(|| spirit::ui4_focus(window_id))
                .await
                .map_err(|code| native_error(action, code))?;
            value["accepted"] = json!(true);
            value["completion"] = json!("selected; observe before sending input");
        }
        SpiritAction::Move { x, y } => {
            with_spirit(|| spirit::move_to(*x, *y)).map_err(|code| native_error(action, code))?;
            value["accepted"] = json!(true);
            value["completion"] = json!("queued");
        }
        SpiritAction::Emotion { idea } => {
            with_spirit(|| {
                spirit::play_emotion(match idea {
                    Emotion::Anger => "anger",
                    Emotion::Disgust => "disgust",
                    Emotion::Fear => "fear",
                    Emotion::Joy => "joy",
                    Emotion::Sadness => "sadness",
                    Emotion::Surprise => "surprise",
                })
            })
            .map_err(|code| native_error(action, code))?;
            value["accepted"] = json!(true);
            value["completion"] = json!("queued");
        }
        SpiritAction::Pointer { .. } | SpiritAction::Type { .. } | SpiritAction::Key { .. } => {
            wait_for_ui4(|| match action {
                SpiritAction::Pointer { x, y, kind } => spirit::ui4_pointer(
                    *x,
                    *y,
                    match kind {
                        PointerKind::Move => spirit::Ui4PointerAction::Move,
                        PointerKind::Click => spirit::Ui4PointerAction::Click,
                    },
                ),
                SpiritAction::Type { text } => spirit::ui4_type(text),
                SpiritAction::Key { key, modifiers } => {
                    let key = native_key(key).ok_or(spirit::UI4_BAD_INPUT)?;
                    let modifiers = modifiers.iter().fold(0, |bits, modifier| {
                        bits | match modifier {
                            KeyModifier::Control => spirit::UI4_KEY_CONTROL,
                            KeyModifier::Shift => spirit::UI4_KEY_SHIFT,
                            KeyModifier::Alt => spirit::UI4_KEY_ALT,
                            KeyModifier::Meta => spirit::UI4_KEY_META,
                        }
                    });
                    spirit::ui4_key_with_modifiers(key, modifiers)
                }
                _ => unreachable!("handled before the input queue"),
            })
            .await
            .map_err(|code| native_error(action, code))?;
            value["accepted"] = json!(true);
            value["completion"] = json!("queued; observe to verify the visible result");
        }
        SpiritAction::Filmstrip(_) => unreachable!("filmstrip is handled separately"),
    }
    Ok(SpiritOutput { value, image_url })
}

impl ToolExecutor<ToolInvocation> for SpiritHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("spirit")
    }

    fn spec(&self) -> ToolSpec {
        spirit_spec(self.include_environment_id)
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = &invocation.payload else {
                return Err(error("spirit requires function arguments"));
            };
            let (action, environment_id) = parse_request(arguments)?;
            let environment = selected_tool_environment(
                &invocation.environments,
                &invocation.step_context.environments,
                environment_id.as_deref(),
            )?;
            if environment.environment().is_remote() {
                return Err(error(
                    "spirit uses local TRUEOS hardware; the selected environment is remote",
                ));
            }
            if matches!(
                action,
                SpiritAction::Observe { .. } | SpiritAction::Filmstrip(_)
            ) && !invocation
                .step_context
                .settings
                .model_info
                .input_modalities
                .contains(&InputModality::Image)
            {
                return Err(error(format!(
                    "spirit {} requires a model that supports image inputs",
                    action.name(),
                )));
            }
            if let SpiritAction::Filmstrip(request) = &action {
                let capture = request.capture(|| {
                    let observation = with_spirit(spirit::ui4_observe)
                        .map_err(|code| native_error(&action, code))?;
                    Ok((observation.metadata, observation.png))
                });
                let (metadata, png) = tokio::select! {
                    biased;
                    _ = invocation.cancellation_token.cancelled() => {
                        return Err(error("Spirit filmstrip capture was cancelled"));
                    }
                    result = capture => result?,
                };
                return Ok(boxed_tool_output(SpiritOutput {
                    value: json!({"backend":"trueos_spirit", "action":"filmstrip", "filmstrip":metadata}),
                    image_url: Some(data_url_from_bytes("image/png", &png)),
                }));
            }
            let output = tokio::select! {
                biased;
                _ = invocation.cancellation_token.cancelled() => {
                    return Err(error("Spirit action was cancelled"));
                }
                result = execute(&action) => result?,
            };
            Ok(boxed_tool_output(output))
        })
    }
}

impl CoreToolRuntime for SpiritHandler {
    fn environments_for_input(
        &self,
        invocation: &ToolInvocation,
    ) -> Result<crate::tools::context::ToolEnvironments, FunctionCallError> {
        crate::tools::handlers::function_tool_environment(
            invocation,
            |args: crate::tools::handlers::ToolEnvironmentArgs| args.environment_id,
        )
    }

    fn is_builtin_control_tool(&self) -> bool {
        true
    }
}

impl ToolOutput for SpiritOutput {
    fn log_output(&self) -> String {
        self.value.to_string()
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, _: &ToolPayload) -> ResponseInputItem {
        let mut items = vec![FunctionCallOutputContentItem::InputText {
            text: self.value.to_string(),
        }];
        if let Some(image_url) = &self.image_url {
            items.push(FunctionCallOutputContentItem::InputImage {
                image: ImageReference::Inline {
                    image_url: image_url.clone(),
                },
                detail: Some(DEFAULT_IMAGE_DETAIL),
            });
        }
        ResponseInputItem::FunctionCallOutput {
            call_id: call_id.into(),
            output: FunctionCallOutputPayload {
                body: FunctionCallOutputBody::ContentItems(items),
                success: Some(true),
            },
        }
    }

    fn code_mode_result(&self, _: &ToolPayload) -> Value {
        let mut value = self.value.clone();
        if let Some(image_url) = &self.image_url {
            value["image_url"] = json!(image_url);
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_keep_coordinate_spaces_and_fields_distinct() {
        for input in [
            json!({"action":"pointer","x":1000,"y":0,"kind":"click"}),
            json!({"action":"move","x":0.25,"y":1.0}),
            json!({"action":"focus","window_id":"42","environment_id":"local"}),
            json!({"action":"observe"}),
            json!({"action":"observe","window_id":"42"}),
            json!({"action":"key","key":"r","modifiers":["alt"]}),
            json!({"action":"key","key":"enter","modifiers":["control"]}),
            json!({"action":"filmstrip","duration_seconds":2}),
        ] {
            assert!(parse_request(&input.to_string()).is_ok(), "{input}");
        }
        for input in [
            json!({"action":"pointer","x":1001,"y":0,"kind":"click"}),
            json!({"action":"move","x":500,"y":1}),
            json!({"action":"windows","text":"unexpected"}),
            json!({"action":"focus","window_id":"0"}),
            json!({"action":"observe","window_id":"expired"}),
            json!({"action":"emotion","idea":"arbitrary animation"}),
            json!({"action":"key","key":"unsupported"}),
            json!({"action":"key","key":"R"}),
            json!({"action":"key","key":"r","modifiers":["unknown"]}),
        ] {
            assert!(parse_request(&input.to_string()).is_err(), "{input}");
        }
    }

    #[test]
    fn text_limits_count_unicode_scalars_and_reject_empty_input() {
        for text in ["a".repeat(64), "🦋".repeat(64)] {
            assert!(parse_request(&json!({"action":"type","text":text}).to_string()).is_ok());
        }
        for text in [String::new(), "a".repeat(65), "🦋".repeat(65)] {
            assert!(parse_request(&json!({"action":"type","text":text}).to_string()).is_err());
        }
    }

    #[test]
    fn schema_lists_all_actions_and_environment_selection() {
        for include_environment in [false, true] {
            let ToolSpec::Function(spec) = spirit_spec(include_environment) else {
                panic!("function spec")
            };
            let variants = spec.parameters.one_of.unwrap();
            assert_eq!(variants.len(), 9);
            for variant in variants {
                assert_eq!(
                    variant.properties.unwrap().contains_key("environment_id"),
                    include_environment
                );
            }
        }
        assert!(!SpiritHandler::new(false).supports_parallel_tool_calls());
    }

    #[test]
    fn observation_emits_metadata_and_real_image_without_logging_base64() {
        let output = SpiritOutput {
            value: json!({"observation":{"id":"42"}}),
            image_url: Some("data:image/png;base64,ABC".into()),
        };
        assert!(!output.log_output().contains("base64"));
        let payload = ToolPayload::Function {
            arguments: "{}".into(),
        };
        let ResponseInputItem::FunctionCallOutput {
            output: response, ..
        } = output.to_response_item("call", &payload)
        else {
            panic!("function output")
        };
        let FunctionCallOutputBody::ContentItems(items) = response.body else {
            panic!("content items")
        };
        assert_eq!(items.len(), 2);
        assert!(matches!(
            items[1],
            FunctionCallOutputContentItem::InputImage { .. }
        ));
        assert_eq!(
            output.code_mode_result(&payload)["image_url"],
            "data:image/png;base64,ABC"
        );
    }

    #[test]
    fn only_busy_errors_promise_that_nothing_was_queued() {
        let action = SpiritAction::Type {
            text: "hello".into(),
        };
        let FunctionCallError::RespondToModel(message) = native_error(&action, spirit::UI4_BUSY)
        else {
            panic!("model error")
        };
        assert!(message.contains("queued nothing"));
        let FunctionCallError::RespondToModel(message) =
            native_error(&action, spirit::UI4_TRANSPORT)
        else {
            panic!("model error")
        };
        assert!(message.contains("completion is unknown"));
    }

    #[tokio::test(start_paused = true)]
    async fn waits_for_busy_and_stops_at_the_first_other_result() {
        for last_result in [Ok(()), Err(spirit::UI4_TRANSPORT)] {
            let mut calls = 0;
            let result = wait_for_ui4(|| {
                calls += 1;
                if calls < 3 {
                    Err(spirit::UI4_BUSY)
                } else {
                    last_result
                }
            })
            .await;
            assert_eq!(calls, 3);
            assert_eq!(result, last_result);
        }
        let start = Instant::now();
        assert_eq!(
            wait_for_ui4(|| Err::<(), _>(spirit::UI4_BUSY)).await,
            Err(spirit::UI4_BUSY)
        );
        assert_eq!(start.elapsed(), UI4_RETRY_TIMEOUT);
    }
}
