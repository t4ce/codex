//! Small Codex tool for the native TRUEOS calculator ABI.
//!
//! Evaluation stays in the existing synchronous calculator protocol; this
//! module only resolves its public registry and shapes tool inputs/outputs.

use std::collections::BTreeMap;

use codex_tools::JsonSchema;
use codex_tools::JsonToolOutput;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use trueos_v::calculator_base::CALCULATOR_FUNCTIONS;
use trueos_v::calculator_base::CALCULATOR_MAX_ARGUMENTS;
use trueos_v::calculator_base::CALCULATOR_PROTOCOL_VERSION;
use trueos_v::calculator_base::CalculatorOperation;
use trueos_v::calculator_base::CalculatorProtocolError;
use trueos_v::calculator_base::evaluate;

use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

pub struct CalculatorHandler {
    include_environment_id: bool,
}

impl CalculatorHandler {
    pub(crate) fn new(include_environment_id: bool) -> Self {
        Self {
            include_environment_id,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CalculatorArgs {
    #[serde(default)]
    operation: Option<String>,
    #[serde(default)]
    arguments: Vec<f64>,
    #[serde(default)]
    environment_id: Option<String>,
}

fn registry_entries() -> Vec<Value> {
    CALCULATOR_FUNCTIONS
        .iter()
        .map(|function| {
            json!({
                "name": function.name,
                "category": function.category,
                "arguments": function.arguments,
                "arity": function.arity,
            })
        })
        .collect()
}

fn calculator_spec(include_environment_id: bool) -> ToolSpec {
    let mut properties = BTreeMap::from([
        (
            "operation".to_string(),
            JsonSchema::string_enum(
                CALCULATOR_FUNCTIONS
                    .iter()
                    .map(|function| json!(function.name))
                    .collect(),
                Some("Registered calculator operation name. Omit with empty arguments to list available operations.".into()),
            ),
        ),
        (
            "arguments".to_string(),
            JsonSchema::array(
                JsonSchema::number(Some("Finite binary64 (f64) numeric argument in the order shown by the operation registry.".into())),
                Some("Binary64 numeric arguments for the selected operation. Must match its registered arity; at most 8 values. Trigonometric inputs use radians unless an operation name explicitly converts units.".into()),
            ),
        ),
    ]);
    if include_environment_id {
        properties.insert(
            "environment_id".into(),
            JsonSchema::string(Some(
                "Environment id from <environment_context>. Omit for the primary environment."
                    .into(),
            )),
        );
    }

    ToolSpec::Function(ResponsesApiTool {
        name: "calculate".into(),
        description: "Evaluate one registered TRUEOS calculator operation over finite binary64 (f64) numeric arguments using the native calculator ABI. Supply arguments in the order shown by the registry; trigonometric inputs use radians unless an operation explicitly converts units. With no operation and no arguments, list available operations. This is not a textual expression parser and does not start a shell or process.".into(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, Some(Vec::new()), Some(false.into())),
        output_schema: Some(json!({
            "oneOf":[
                {
                    "type":"object",
                    "properties":{
                        "mode":{"const":"registry"},
                        "protocol_version":{"type":"integer"},
                        "functions":{"type":"array","items":{"type":"object","properties":{
                            "name":{"type":"string"},"category":{"type":"string"},
                            "arguments":{"type":"string"},"arity":{"type":"integer"}
                        },"required":["name","category","arguments","arity"],"additionalProperties":false}}
                    },
                    "required":["mode","protocol_version","functions"],
                    "additionalProperties":false
                },
                {
                    "type":"object",
                    "properties":{
                        "mode":{"const":"result"},
                        "protocol_version":{"type":"integer"},
                        "operation":{"type":"string"},
                        "arguments":{"type":"array","items":{"type":"number"}},
                        "result":{"type":["number","null"]},
                        "result_text":{"type":"string"},
                        "finite":{"type":"boolean"},
                        "backend":{"type":"string"}
                    },
                    "required":["mode","protocol_version","operation","arguments","result","result_text","finite","backend"],
                    "additionalProperties":false
                }
            ]
        }).into()),
    })
}

fn protocol_error(error: CalculatorProtocolError) -> String {
    match error {
        CalculatorProtocolError::InvalidPointer => {
            "calculator ABI rejected an invalid pointer".into()
        }
        CalculatorProtocolError::TooManyArguments => {
            format!("calculator accepts at most {CALCULATOR_MAX_ARGUMENTS} arguments")
        }
        CalculatorProtocolError::UnknownOperation => "unknown calculator operation".into(),
        CalculatorProtocolError::WrongArgumentCount => {
            "calculator operation received the wrong number of arguments".into()
        }
        CalculatorProtocolError::InvalidIntegerArgument => {
            "calculator operation requires an integer-valued argument".into()
        }
        CalculatorProtocolError::Kernel(code) => {
            format!("calculator ABI returned unexpected status {code}")
        }
    }
}

fn compute(
    operation: Option<&str>,
    arguments: &[f64],
    evaluate_operation: impl FnOnce(CalculatorOperation, &[f64]) -> Result<f64, CalculatorProtocolError>,
) -> Result<Value, String> {
    let Some(operation) = operation else {
        if !arguments.is_empty() {
            return Err("arguments require an operation; omit both to list operations".into());
        }
        return Ok(json!({
            "mode":"registry",
            "protocol_version":CALCULATOR_PROTOCOL_VERSION,
            "functions":registry_entries(),
        }));
    };

    if arguments.len() > CALCULATOR_MAX_ARGUMENTS {
        return Err(format!(
            "calculator accepts at most {CALCULATOR_MAX_ARGUMENTS} arguments"
        ));
    }
    if arguments.iter().any(|argument| !argument.is_finite()) {
        return Err("calculator arguments must all be finite numbers".into());
    }

    let function = CALCULATOR_FUNCTIONS
        .iter()
        .find(|function| function.name == operation)
        .ok_or_else(|| {
            format!(
                "unknown calculator operation {operation:?}; omit operation to list available names"
            )
        })?;
    if arguments.len() != usize::from(function.arity) {
        return Err(format!(
            "operation {} expects {} arguments ({}), received {}",
            function.name,
            function.arity,
            function.arguments,
            arguments.len()
        ));
    }

    let result = evaluate_operation(function.operation, arguments).map_err(protocol_error)?;
    let finite = result.is_finite();
    let result_text = if result.is_nan() {
        "NaN".to_string()
    } else if result == f64::INFINITY {
        "Infinity".to_string()
    } else if result == f64::NEG_INFINITY {
        "-Infinity".to_string()
    } else {
        result.to_string()
    };
    Ok(json!({
        "mode":"result",
        "protocol_version":CALCULATOR_PROTOCOL_VERSION,
        "operation":function.name,
        "arguments":arguments,
        "result":finite.then_some(result),
        "result_text":result_text,
        "finite":finite,
        "backend":"trueos_calculator_abi",
    }))
}

impl ToolExecutor<ToolInvocation> for CalculatorHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("calculate")
    }

    fn spec(&self) -> ToolSpec {
        calculator_spec(self.include_environment_id)
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = &invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "calculate requires function arguments".into(),
                ));
            };
            let args: CalculatorArgs = parse_arguments(arguments)?;
            let environment = resolve_tool_environment(
                &invocation.step_context.environments,
                args.environment_id.as_deref(),
            )?;
            if environment.environment.is_remote() {
                return Err(FunctionCallError::RespondToModel(
                    "calculate uses the local TRUEOS calculator ABI; the selected environment is remote".into(),
                ));
            }

            let result = compute(args.operation.as_deref(), &args.arguments, evaluate)
                .map_err(FunctionCallError::RespondToModel)?;
            Ok(boxed_tool_output(JsonToolOutput::new(result)))
        })
    }
}

impl CoreToolRuntime for CalculatorHandler {
    fn is_builtin_control_tool(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_compute(operation: Option<&str>, arguments: &[f64]) -> Result<Value, String> {
        compute(operation, arguments, trueos_v::calculator_base::evaluate)
    }

    #[test]
    fn missing_operation_lists_registered_functions() {
        let result = test_compute(None, &[]).expect("registry listing");
        assert_eq!(result["mode"], "registry");
        assert_eq!(result["functions"].as_array().unwrap().len(), 93);
        assert_eq!(result["protocol_version"], CALCULATOR_PROTOCOL_VERSION);
    }

    #[test]
    fn named_operations_use_existing_calculator_abi_wrapper() {
        assert_eq!(
            test_compute(Some("add"), &[20.0, 22.0]).unwrap()["result"],
            42.0
        );
        assert_eq!(
            test_compute(Some("power"), &[2.0, 8.0]).unwrap()["result"],
            256.0
        );
        assert_eq!(
            test_compute(Some("square_root"), &[81.0]).unwrap()["result"],
            9.0
        );
    }

    #[test]
    fn invalid_inputs_and_nonfinite_results_are_explicit() {
        assert!(
            test_compute(None, &[1.0])
                .unwrap_err()
                .contains("require an operation")
        );
        assert!(
            test_compute(Some("missing"), &[])
                .unwrap_err()
                .contains("unknown calculator operation")
        );
        assert!(
            test_compute(Some("add"), &[1.0])
                .unwrap_err()
                .contains("expects 2 arguments")
        );
        assert!(
            test_compute(Some("add"), &[f64::NAN, 1.0])
                .unwrap_err()
                .contains("finite")
        );
        assert!(
            test_compute(Some("add"), &[0.0; 9])
                .unwrap_err()
                .contains("at most 8")
        );
        assert!(
            test_compute(Some("nth_root"), &[-27.0, 2.5])
                .unwrap_err()
                .contains("integer-valued")
        );

        let result = test_compute(Some("divide"), &[1.0, 0.0]).expect("division result");
        assert_eq!(result["finite"], false);
        assert_eq!(result["result"], Value::Null);
        assert_eq!(result["result_text"], "Infinity");
    }
}
