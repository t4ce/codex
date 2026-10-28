//! Host-only fixture for the production native Spirit tool and public API.
#![cfg(test)]
#![allow(
    dead_code,
    unused_imports,
    irrefutable_let_patterns,
    private_interfaces
)]

extern crate self as codex_protocol;
extern crate self as codex_tools;
extern crate self as codex_utils_image;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[path = "../../../codex-rs/tools/src/json_schema/types.rs"]
pub mod schema;
#[path = "../../../codex-rs/core/src/tools/handlers/trueos_spirit.rs"]
pub mod spirit_handler;
pub use schema::JsonSchema;
pub struct ResponsesApiTool {
    pub name: String,
    pub description: String,
    pub strict: bool,
    pub defer_loading: Option<bool>,
    pub parameters: JsonSchema,
    pub output_schema: Option<Value>,
}
pub enum ToolSpec {
    Function(ResponsesApiTool),
    Other,
}
pub struct ToolName(String);
impl ToolName {
    pub fn plain(name: &str) -> Self {
        Self(name.into())
    }
}
pub type ToolExecutorFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<Box<dyn ToolOutput>, function_tool::FunctionCallError>,
            > + Send
            + 'a,
    >,
>;
pub trait ToolExecutor<T>: Send + Sync {
    fn tool_name(&self) -> ToolName;
    fn spec(&self) -> ToolSpec;
    fn supports_parallel_tool_calls(&self) -> bool {
        false
    }
    fn handle<'a>(&'a self, invocation: T) -> ToolExecutorFuture<'a>
    where
        T: 'a;
}
pub trait ToolOutput: Send {
    fn log_output(&self) -> String;
    fn success_for_logging(&self) -> bool;
    fn to_response_item(
        &self,
        call_id: &str,
        payload: &tools::context::ToolPayload,
    ) -> models::ResponseInputItem;
    fn code_mode_result(&self, payload: &tools::context::ToolPayload) -> Value;
}
pub fn data_url_from_bytes(mime: &str, bytes: &[u8]) -> String {
    use base64::Engine;
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}
pub mod models {
    use super::*;
    pub const DEFAULT_IMAGE_DETAIL: ImageDetail = ImageDetail::High;
    #[derive(Serialize)]
    pub enum ImageDetail {
        High,
    }
    #[derive(Serialize)]
    pub enum ImageReference {
        Inline { image_url: String },
    }
    #[derive(Serialize)]
    pub enum FunctionCallOutputContentItem {
        InputText {
            text: String,
        },
        InputImage {
            image: ImageReference,
            detail: Option<ImageDetail>,
        },
    }
    #[derive(Serialize)]
    pub enum FunctionCallOutputBody {
        ContentItems(Vec<FunctionCallOutputContentItem>),
    }
    #[derive(Serialize)]
    pub struct FunctionCallOutputPayload {
        pub body: FunctionCallOutputBody,
        pub success: Option<bool>,
    }
    #[derive(Serialize)]
    pub enum ResponseInputItem {
        FunctionCallOutput {
            call_id: String,
            output: FunctionCallOutputPayload,
        },
    }
}
pub mod openai_models {
    #[derive(PartialEq)]
    pub enum InputModality {
        Image,
    }
}
pub mod function_tool {
    #[derive(Debug)]
    pub enum FunctionCallError {
        RespondToModel(String),
    }
    impl std::fmt::Display for FunctionCallError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::RespondToModel(s) => f.write_str(s),
            }
        }
    }
}
pub mod tools {
    pub mod context {
        pub use crate::ToolOutput;
        pub enum ToolPayload {
            Function { arguments: String },
            Other,
        }
        pub struct ToolInvocation {
            pub payload: ToolPayload,
            pub step_context: StepContext,
        }
        pub struct StepContext {
            pub environments: Vec<Environment>,
            pub settings: Settings,
        }
        pub struct Environment {
            pub environment: Machine,
        }
        pub struct Machine(pub bool);
        impl Machine {
            pub fn is_remote(&self) -> bool {
                self.0
            }
        }
        pub struct Settings {
            pub model_info: ModelInfo,
        }
        pub struct ModelInfo {
            pub input_modalities: Vec<crate::openai_models::InputModality>,
        }
        pub fn boxed_tool_output<T: ToolOutput + 'static>(value: T) -> Box<dyn ToolOutput> {
            Box::new(value)
        }
    }
    pub mod registry {
        pub use crate::ToolExecutor;
        pub trait CoreToolRuntime: ToolExecutor<super::context::ToolInvocation> {
            fn is_builtin_control_tool(&self) -> bool {
                false
            }
        }
    }
    pub mod handlers {
        pub fn parse_arguments<T: serde::de::DeserializeOwned>(
            s: &str,
        ) -> Result<T, crate::function_tool::FunctionCallError> {
            serde_json::from_str(s)
                .map_err(|e| crate::function_tool::FunctionCallError::RespondToModel(e.to_string()))
        }
        pub fn resolve_tool_environment<'a>(
            env: &'a [super::context::Environment],
            _: Option<&str>,
        ) -> Result<&'a super::context::Environment, crate::function_tool::FunctionCallError>
        {
            Ok(&env[0])
        }
        pub use crate::spirit_handler as trueos_spirit;
    }
}

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
static FIXTURE: std::sync::Mutex<Fixture> = std::sync::Mutex::new(Fixture {
    calls: Vec::new(),
    error: 0,
});
struct Fixture {
    calls: Vec<Value>,
    error: i32,
}
fn record(value: Value) -> i32 {
    let mut f = FIXTURE.lock().unwrap();
    f.calls.push(value);
    f.error
}
fn png() -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=").unwrap()
}
unsafe fn copy(bytes: &[u8], out: *mut u8, cap: usize) -> isize {
    if !out.is_null() && cap >= bytes.len() {
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    }
    bytes.len() as isize
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_spirit_emotion_play(p: *const u8, n: usize) -> i32 {
    record(json!([
        "emotion",
        std::str::from_utf8(unsafe { std::slice::from_raw_parts(p, n) }).unwrap()
    ]))
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_spirit_move(x: f32, y: f32) -> i32 {
    record(json!(["move", x, y]))
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_spirit_text_present_silent(_: u64, _: *const u8, _: usize) -> i32 {
    -5
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_dobby_ui4_windows(p: *mut u8, n: usize) -> isize {
    unsafe {
        copy(
            b"{\"windows\":[{\"id\":\"42\",\"name\":\"native\"}],\"truncated\":false}",
            p,
            n,
        )
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_dobby_ui4_focus(id: u64) -> i32 {
    record(json!(["focus", id]))
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_dobby_ui4_observe_prepare() -> isize {
    png().len() as isize
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_dobby_ui4_observe_metadata(p: *mut u8, n: usize) -> isize {
    unsafe { copy(b"{\"id\":\"42\",\"grid_extent\":1000}", p, n) }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_dobby_ui4_observe_read(
    offset: usize,
    p: *mut u8,
    n: usize,
) -> isize {
    let bytes = png();
    let count = n.min(bytes.len() - offset);
    unsafe { std::ptr::copy_nonoverlapping(bytes[offset..].as_ptr(), p, count) };
    count as isize
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_dobby_ui4_pointer(x: u16, y: u16, action: u32) -> i32 {
    record(json!(["pointer", x, y, action]))
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_dobby_ui4_type(p: *const u8, n: usize) -> i32 {
    record(json!([
        "type",
        std::str::from_utf8(unsafe { std::slice::from_raw_parts(p, n) }).unwrap()
    ]))
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_dobby_ui4_key(key: u32) -> i32 {
    record(json!(["key", key]))
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use tools::context::*;
    use tools::handlers::trueos_spirit::SpiritHandler;
    fn run(
        args: Value,
        remote: bool,
        image: bool,
    ) -> Result<Box<dyn ToolOutput>, function_tool::FunctionCallError> {
        futures::executor::block_on(SpiritHandler::new(false).handle(ToolInvocation {
            payload: ToolPayload::Function {
                arguments: args.to_string(),
            },
            step_context: StepContext {
                environments: vec![Environment {
                    environment: Machine(remote),
                }],
                settings: Settings {
                    model_info: ModelInfo {
                        input_modalities: if image {
                            vec![openai_models::InputModality::Image]
                        } else {
                            vec![]
                        },
                    },
                },
            },
        }))
    }
    fn reset() {
        let mut f = FIXTURE.lock().unwrap();
        f.calls.clear();
        f.error = 0;
    }
    #[test]
    fn all_mutations_reach_real_public_wrappers_and_native_abi() {
        let _serial = SERIAL.lock().unwrap();
        reset();
        for args in [
            json!({"action":"focus","window_id":"42"}),
            json!({"action":"pointer","x":123,"y":987,"kind":"click"}),
            json!({"action":"pointer","x":0,"y":1000,"kind":"move"}),
            json!({"action":"type","text":"Lilly §"}),
            json!({"action":"key","key":"escape"}),
            json!({"action":"move","x":0.25,"y":0.75}),
            json!({"action":"emotion","idea":"joy"}),
        ] {
            let output = run(args, false, true).unwrap();
            assert_eq!(
                output.code_mode_result(&ToolPayload::Other)["accepted"],
                true
            );
        }
        assert_eq!(
            FIXTURE.lock().unwrap().calls,
            vec![
                json!(["focus", 42]),
                json!(["pointer", 123, 987, 1]),
                json!(["pointer", 0, 1000, 0]),
                json!(["type", "Lilly §"]),
                json!(["key", 2]),
                json!(["move", 0.25, 0.75]),
                json!(["emotion", "joy"])
            ]
        );
    }
    #[test]
    fn native_inventory_and_capture_reach_model_as_metadata_and_png() {
        let _serial = SERIAL.lock().unwrap();
        reset();
        let output = run(json!({"action":"windows"}), false, true).unwrap();
        assert_eq!(
            output.code_mode_result(&ToolPayload::Other)["inventory"]["windows"][0]["id"],
            "42"
        );
        let output = run(json!({"action":"observe"}), false, true).unwrap();
        let value = output.code_mode_result(&ToolPayload::Other);
        assert_eq!(value["observation"]["grid_extent"], 1000);
        assert_eq!(value["image_url"], data_url_from_bytes("image/png", &png()));
        assert!(!output.log_output().contains("base64"));
    }
    #[test]
    fn rejected_remote_unsupported_image_and_invalid_inputs_make_no_native_calls() {
        let _serial = SERIAL.lock().unwrap();
        reset();
        assert!(run(json!({"action":"key","key":"enter"}), true, true).is_err());
        assert!(run(json!({"action":"observe"}), false, false).is_err());
        assert!(
            run(
                json!({"action":"pointer","x":1001,"y":0,"kind":"click"}),
                false,
                true
            )
            .is_err()
        );
        assert!(FIXTURE.lock().unwrap().calls.is_empty());
    }
    #[test]
    fn busy_and_transport_errors_never_auto_retry_input() {
        let _serial = SERIAL.lock().unwrap();
        for code in [-4, -7] {
            reset();
            FIXTURE.lock().unwrap().error = code;
            let result = run(json!({"action":"type","text":"once"}), false, true);
            assert!(result.is_err());
            assert_eq!(FIXTURE.lock().unwrap().calls.len(), 1);
        }
    }
}
