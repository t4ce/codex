//! Model-facing access to the running Edit workspace.
use std::collections::BTreeMap;

use codex_tools::JsonSchema;
use codex_tools::JsonToolOutput;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde_json::Value;
use serde_json::json;
use trueos_ide::protocol::IdeRequest;

use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

pub struct IdeHandler {
    include_environment_id: bool,
}

impl IdeHandler {
    pub(crate) fn new(include_environment_id: bool) -> Self {
        Self {
            include_environment_id,
        }
    }
}

fn spec(include_environment_id: bool) -> ToolSpec {
    let mut properties = BTreeMap::from([
        ("action".into(), JsonSchema::string_enum(
            ["open", "discover", "context", "read_file", "write_file", "compile", "build_status", "cancel_build", "run"]
                .into_iter().map(|name| json!(name)).collect(),
            Some("Workspace operation.".into()),
        )),
        ("endpoint".into(), JsonSchema::integer(Some("Endpoint returned by discover. Defaults to the workspace that launched Codex, or the sole open workspace for an ordinary Codex launch.".into()))),
        ("path".into(), JsonSchema::string(Some("Project-relative source path for read_file or write_file.".into()))),
        ("expected_revision_id".into(), JsonSchema::integer(Some("Revision returned by context or read_file; required for write_file and compile.".into()))),
        ("expected_text".into(), JsonSchema::string(Some("Previously read live file content; omit only when creating a new file.".into()))),
        ("text".into(), JsonSchema::string(Some("Complete replacement source text for write_file.".into()))),
        ("project".into(), JsonSchema::string(Some("Project name for open.".into()))),
        ("database".into(), JsonSchema::string(Some("Optional project database path for open; defaults to the shared IDE database.".into()))),
        ("compiler".into(), JsonSchema::string_enum(vec![json!("odin-son"), json!("rustc-min")], Some("Optional compiler for open.".into()))),
        ("entry".into(), JsonSchema::string(Some("Optional project-relative compiler entry for open.".into()))),
    ]);
    if include_environment_id {
        properties.insert(
            "environment_id".into(),
            JsonSchema::string(Some(
                "Environment id from <environment_context>; must be local TRUEOS.".into(),
            )),
        );
    }
    ToolSpec::Function(ResponsesApiTool {
        name: "trueos_ide".into(),
        description: "Work with a TRUEOS Edit project. open attaches to or launches a named project and returns its endpoint and context; discover lists workspaces; context returns the current revision and editor state; read_file reads project source; write_file saves complete text using the revision returned by context or read_file; compile starts a build for the expected revision; build_status reports progress; cancel_build requests cancellation; run starts the built project. Use endpoint from open or discover when multiple workspaces are open. Refresh context and source after a revision conflict. Edit owns the project store and build lifecycle.".into(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, Some(vec!["action".into()]), Some(false.into())),
        output_schema: None,
    })
}

impl ToolExecutor<ToolInvocation> for IdeHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("trueos_ide")
    }
    fn spec(&self) -> ToolSpec {
        spec(self.include_environment_id)
    }
    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = &invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "trueos_ide requires function arguments".into(),
                ));
            };
            let mut value: Value = parse_arguments(arguments)?;
            let object = value.as_object_mut().ok_or_else(|| {
                FunctionCallError::RespondToModel("trueos_ide arguments must be an object".into())
            })?;
            let environment_id = match object.remove("environment_id") {
                None | Some(Value::Null) => None,
                Some(Value::String(id)) => Some(id),
                _ => {
                    return Err(FunctionCallError::RespondToModel(
                        "environment_id must be a string".into(),
                    ));
                }
            };
            let environment = resolve_tool_environment(
                &invocation.step_context.environments,
                environment_id.as_deref(),
            )?;
            if environment.environment.is_remote() {
                return Err(FunctionCallError::RespondToModel(
                    "trueos_ide requires a local TRUEOS environment".into(),
                ));
            }
            let endpoint = match object.remove("endpoint") {
                None | Some(Value::Null) => None,
                Some(Value::Number(value)) => Some(value.as_u64().ok_or_else(|| {
                    FunctionCallError::RespondToModel(
                        "endpoint must be a nonnegative integer".into(),
                    )
                })?),
                _ => {
                    return Err(FunctionCallError::RespondToModel(
                        "endpoint must be an integer".into(),
                    ));
                }
            };
            let action = object.get("action").and_then(Value::as_str).unwrap_or("");
            let result = if action == "open" {
                if endpoint.is_some() {
                    return Err(FunctionCallError::RespondToModel(
                        "open does not accept endpoint".into(),
                    ));
                }
                object.remove("action");
                let options: crate::trueos_ide::OpenOptions = serde_json::from_value(value)
                    .map_err(|error| {
                        FunctionCallError::RespondToModel(format!("invalid open request: {error}"))
                    })?;
                let (endpoint, response) = crate::trueos_ide::open(options)
                    .await
                    .map_err(FunctionCallError::RespondToModel)?;
                Ok(json!({"endpoint": endpoint, "response": response}))
            } else if action == "discover" {
                if object.len() != 1 || endpoint.is_some() {
                    return Err(FunctionCallError::RespondToModel(
                        "discover accepts no other fields".into(),
                    ));
                }
                serde_json::to_value(
                    crate::trueos_ide::discover()
                        .await
                        .map_err(FunctionCallError::RespondToModel)?,
                )
            } else {
                let request: IdeRequest = serde_json::from_value(value).map_err(|error| {
                    FunctionCallError::RespondToModel(format!("invalid IDE request: {error}"))
                })?;
                serde_json::to_value(
                    crate::trueos_ide::request(endpoint, request)
                        .await
                        .map_err(FunctionCallError::RespondToModel)?,
                )
            }
            .map_err(|error| {
                FunctionCallError::RespondToModel(format!("failed to encode IDE response: {error}"))
            })?;
            let success = result
                .get("status")
                .or_else(|| {
                    result
                        .get("response")
                        .and_then(|response| response.get("status"))
                })
                .and_then(Value::as_str)
                != Some("error");
            Ok(boxed_tool_output(JsonToolOutput::with_success(
                result,
                Some(success),
            )))
        })
    }
}
impl CoreToolRuntime for IdeHandler {}
