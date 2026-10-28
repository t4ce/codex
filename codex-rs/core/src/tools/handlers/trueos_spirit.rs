//! Direct access to Spirit's resident avatar and paired UI4 input devices.

use std::collections::BTreeMap;

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
use trueos::spirit;

use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

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
    Focus { window_id: String },
    Observe {},
    Pointer { x: u16, y: u16, kind: PointerKind },
    Type { text: String },
    Key { key: Key },
    Move { x: f32, y: f32 },
    Emotion { idea: Emotion },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PointerKind {
    Move,
    Click,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Key {
    Enter,
    Escape,
    Tab,
    Space,
    Backspace,
    Up,
    Down,
    Left,
    Right,
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
            Self::Observe {} => "observe",
            Self::Pointer { .. } => "pointer",
            Self::Type { .. } => "type",
            Self::Key { .. } => "key",
            Self::Move { .. } => "move",
            Self::Emotion { .. } => "emotion",
        }
    }

    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Focus { window_id } if window_id.parse::<u32>().ok().is_none_or(|id| id == 0) =>
                Err("window_id must be a nonzero decimal id returned by spirit action=windows".into()),
            Self::Pointer { x, y, .. } if *x > 1000 || *y > 1000 =>
                Err("pointer coordinates use the observed selected-window grid, inclusive 0..1000".into()),
            Self::Move { x, y } if !x.is_finite() || !y.is_finite() || !(0.0..=1.0).contains(x) || !(0.0..=1.0).contains(y) =>
                Err("move coordinates position the hardware avatar on the scanout, inclusive 0..1".into()),
            Self::Type { text } if text.is_empty() || text.len() > 1024 || text.chars().count() > 64 =>
                Err("type accepts 1..64 Unicode scalars and at most 1024 UTF-8 bytes per call; send longer text in ordered chunks".into()),
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

fn names(values: &[&str], description: &str) -> JsonSchema {
    JsonSchema::string_enum(
        values.iter().map(|value| json!(value)).collect(),
        Some(description.into()),
    )
}

fn spirit_spec(include_environment_id: bool) -> ToolSpec {
    let mut variants = Vec::new();
    for (action, fields) in [
        ("windows", vec![]),
        ("focus", vec![("window_id", JsonSchema::string(Some("Decimal window id from windows inventory.".into())))]),
        ("observe", vec![]),
        ("pointer", vec![
            ("x", JsonSchema::integer(Some("Selected-window horizontal grid coordinate, inclusive 0..1000.".into()))),
            ("y", JsonSchema::integer(Some("Selected-window vertical grid coordinate, inclusive 0..1000.".into()))),
            ("kind", names(&["move", "click"], "Move Spirit's software pointer or queue one primary click.")),
        ]),
        ("type", vec![("text", JsonSchema::string(Some("1..64 Unicode scalars, at most 1024 UTF-8 bytes. Input is queued through Spirit's paired keyboard.".into())))]),
        ("key", vec![("key", names(&["enter", "escape", "tab", "space", "backspace", "up", "down", "left", "right"], "One key on Spirit's paired keyboard."))]),
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
            required.push(name.into());
        }
        if include_environment_id {
            properties.insert("environment_id".into(), JsonSchema::string(Some("Environment id from <environment_context>; the environment must be local TRUEOS.".into())));
        }
        variants.push(JsonSchema::object(properties, Some(required), Some(false.into())));
    }
    // Keep the root an object; per-action objects reject fields from other actions.
    let mut parameters = JsonSchema::object(BTreeMap::new(), None, None);
    parameters.one_of = Some(variants);
    ToolSpec::Function(ResponsesApiTool {
        name: "spirit".into(),
        description: "Use resident TRUEOS Spirit directly for native OS interaction. Spirit is configured automatically at OS boot, with Lilly as her avatar. /spirit shows the native integration information. Spirit manages avatar presentation, animation transitions, and paired input sequencing. windows lists live UI4 windows; focus selects one; observe returns its actual PNG and coordinate metadata. Before pointer/type/key, focus then observe the current target; observe again after focus or window geometry changes. pointer uses selected-window grid coordinates 0..1000. move positions Lilly's hardware avatar on the scanout using 0..1 coordinates. emotion requests an expression. Input actions are queued, not proof of completion: observe to verify results. Calls are serial; BUSY means nothing from this call was queued, so wait and retry it. Do not replay accepted text or clicks. This tool is available automatically in every TRUEOS Codex session.".into(),
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
            "focus the target and observe it again before sending pointer or keyboard input"
        }
        spirit::UI4_NOT_FOUND => {
            "target is no longer available; refresh windows and select a live target"
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

fn execute(action: &SpiritAction) -> Result<SpiritOutput, FunctionCallError> {
    let mut value = json!({"backend":"trueos_spirit", "action":action.name()});
    let mut image_url = None;
    match action {
        SpiritAction::Windows {} => {
            let inventory = spirit::ui4_windows().map_err(|code| native_error(action, code))?;
            value["inventory"] = serde_json::from_str(&inventory).map_err(error)?;
        }
        SpiritAction::Observe {} => {
            let observation = spirit::ui4_observe().map_err(|code| native_error(action, code))?;
            if !observation.png.starts_with(b"\x89PNG\r\n\x1a\n") {
                return Err(error(
                    "native Spirit observation did not contain a PNG image",
                ));
            }
            value["observation"] = serde_json::from_str(&observation.metadata).map_err(error)?;
            image_url = Some(data_url_from_bytes("image/png", &observation.png));
        }
        _ => {
            let result = match action {
                SpiritAction::Focus { window_id } => {
                    spirit::ui4_focus(window_id.parse().map_err(error)?)
                }
                SpiritAction::Pointer { x, y, kind } => spirit::ui4_pointer(
                    *x,
                    *y,
                    match kind {
                        PointerKind::Move => spirit::Ui4PointerAction::Move,
                        PointerKind::Click => spirit::Ui4PointerAction::Click,
                    },
                ),
                SpiritAction::Type { text } => spirit::ui4_type(text),
                SpiritAction::Key { key } => spirit::ui4_key(match key {
                    Key::Enter => spirit::Ui4Key::Enter,
                    Key::Escape => spirit::Ui4Key::Escape,
                    Key::Tab => spirit::Ui4Key::Tab,
                    Key::Space => spirit::Ui4Key::Space,
                    Key::Backspace => spirit::Ui4Key::Backspace,
                    Key::Up => spirit::Ui4Key::Up,
                    Key::Down => spirit::Ui4Key::Down,
                    Key::Left => spirit::Ui4Key::Left,
                    Key::Right => spirit::Ui4Key::Right,
                }),
                SpiritAction::Move { x, y } => spirit::move_to(*x, *y),
                SpiritAction::Emotion { idea } => spirit::play_emotion(match idea {
                    Emotion::Anger => "anger",
                    Emotion::Disgust => "disgust",
                    Emotion::Fear => "fear",
                    Emotion::Joy => "joy",
                    Emotion::Sadness => "sadness",
                    Emotion::Surprise => "surprise",
                }),
                SpiritAction::Windows {} | SpiritAction::Observe {} => unreachable!(),
            };
            result.map_err(|code| native_error(action, code))?;
            value["accepted"] = json!(true);
            value["completion"] = json!("queued; observe to verify the visible result");
        }
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
            let environment = resolve_tool_environment(
                &invocation.step_context.environments,
                environment_id.as_deref(),
            )?;
            if environment.environment.is_remote() {
                return Err(error(
                    "spirit uses local TRUEOS hardware; the selected environment is remote",
                ));
            }
            if matches!(action, SpiritAction::Observe {})
                && !invocation
                    .step_context
                    .settings
                    .model_info
                    .input_modalities
                    .contains(&InputModality::Image)
            {
                return Err(error(
                    "spirit observe requires a model that supports image inputs",
                ));
            }
            Ok(boxed_tool_output(execute(&action)?))
        })
    }
}

impl CoreToolRuntime for SpiritHandler {
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
        ] {
            assert!(parse_request(&input.to_string()).is_ok(), "{input}");
        }
        for input in [
            json!({"action":"pointer","x":1001,"y":0,"kind":"click"}),
            json!({"action":"move","x":500,"y":1}),
            json!({"action":"windows","text":"unexpected"}),
            json!({"action":"focus","window_id":"0"}),
            json!({"action":"emotion","idea":"arbitrary animation"}),
            json!({"action":"key","key":"unsupported"}),
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
            assert_eq!(variants.len(), 8);
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
}
