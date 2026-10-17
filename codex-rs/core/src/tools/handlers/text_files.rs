//! Exact UTF-8 file tools. Language syntax is independent of storage.
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::FileSystemSandboxContext;
use codex_exec_server::GetMetadataOptions;
use codex_exec_server::WriteFileOptions;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::FileChange;
use codex_sandboxing::SandboxablePreference;
use codex_sandboxing::policy_transforms::effective_file_system_sandbox_policy;
use codex_sandboxing::policy_transforms::effective_permission_profile;
use codex_tools::JsonSchema;
use codex_tools::JsonToolOutput;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use codex_utils_paths::path_uri::PathUri;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;

use crate::function_tool::FunctionCallError;
use crate::safety::PatchSandboxRoute;
use crate::safety::SafetyCheck;
use crate::safety::assess_write_safety;
use crate::session::turn_context::TurnEnvironment;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::apply_granted_turn_permissions;
use crate::tools::handlers::apply_patch::write_permissions_for_paths;
use crate::tools::handlers::file_system_sandbox_policy_context_for_cwd;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::orchestrator::ToolOrchestrator;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use crate::tools::sandboxing::Approvable;
use crate::tools::sandboxing::ApprovalAction;
use crate::tools::sandboxing::ExecApprovalRequirement;
use crate::tools::sandboxing::SandboxAttempt;
use crate::tools::sandboxing::Sandboxable;
use crate::tools::sandboxing::ToolCtx;
use crate::tools::sandboxing::ToolError;
use crate::tools::sandboxing::ToolRuntime;
use crate::tools::sandboxing::executor_windows_sandbox_selection;
use crate::windows_sandbox::windows_sandbox_level_for_legacy_checks;

const DEFAULT_READ_BYTES: usize = 64 * 1024;
const MAX_TEXT_BYTES: usize = 1024 * 1024;

pub struct ReadFileHandler {
    include_environment_id: bool,
}

pub struct WriteFileHandler {
    include_environment_id: bool,
}

impl ReadFileHandler {
    pub(crate) fn new(include_environment_id: bool) -> Self {
        Self {
            include_environment_id,
        }
    }
}

impl WriteFileHandler {
    pub(crate) fn new(include_environment_id: bool) -> Self {
        Self {
            include_environment_id,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    path: String,
    #[serde(default)]
    max_bytes: Option<usize>,
    #[serde(default)]
    environment_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    path: String,
    content: String,
    #[serde(default)]
    overwrite: bool,
    #[serde(default)]
    expected_sha256: Option<String>,
    #[serde(default)]
    validate: Option<String>,
    #[serde(default)]
    environment_id: Option<String>,
}

fn text_error(error: impl std::fmt::Display) -> FunctionCallError {
    FunctionCallError::RespondToModel(error.to_string())
}

fn parameters(include_environment_id: bool) -> BTreeMap<String, JsonSchema> {
    let mut properties = BTreeMap::from([(
        "path".into(),
        JsonSchema::string(Some(
            "Absolute path, or path relative to the environment cwd.".into(),
        )),
    )]);
    if include_environment_id {
        properties.insert(
            "environment_id".into(),
            JsonSchema::string(Some(
                "Environment id from <environment_context>. Omit for the primary environment."
                    .into(),
            )),
        );
    }
    properties
}

fn read_spec(include_environment_id: bool) -> ToolSpec {
    let mut properties = parameters(include_environment_id);
    properties.insert("max_bytes".into(), JsonSchema::integer(Some(
        "Maximum complete file size to read, 1 through 1048576 bytes; defaults to 65536. Larger files fail explicitly; output is never silently truncated.".into(),
    )));
    ToolSpec::Function(ResponsesApiTool {
        name: "read_file".into(),
        description: "Read an entire UTF-8 text file directly, without a shell or terminal. Works for Rust, HTML, JSON, CSV, plain text and any other extension. Returns exact content, byte count, SHA-256 and native content type when available; rejects invalid UTF-8 or oversized files.".into(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, Some(vec!["path".into()]), Some(false.into())),
        output_schema: Some(json!({"type":"object","properties":{
            "path":{"type":"string"},"content":{"type":"string"},"bytes":{"type":"integer"},
            "sha256":{"type":"string"},"content_type":{"type":["integer","null"]},"truncated":{"type":"boolean"}
        },"required":["path","content","bytes","sha256","content_type","truncated"],"additionalProperties":false}).into()),
    })
}

fn write_spec(include_environment_id: bool) -> ToolSpec {
    let mut properties = parameters(include_environment_id);
    properties.extend([
        ("content".into(), JsonSchema::string(Some("Exact UTF-8 content, at most 1048576 bytes. No implicit newline, formatting or syntax conversion.".into()))),
        ("overwrite".into(), JsonSchema::boolean(Some("Defaults to false: atomically create a new file. To replace an existing file, set true and supply its expected_sha256 from read_file.".into()))),
        ("expected_sha256".into(), JsonSchema::string(Some("Required with overwrite:true. The existing file's 64 hexadecimal SHA-256 characters. Replacement fails if the file has changed.".into()))),
        ("validate".into(), JsonSchema::string_enum(vec![json!("json")], Some("Optional JSON syntax validation without reserialization or number conversion. Omit to save any UTF-8 text, including incomplete code or markup.".into()))),
    ]);
    ToolSpec::Function(ResponsesApiTool {
        name: "write_file".into(),
        description: "Create or explicitly replace a UTF-8 text file directly, without a shell or terminal. Works with any extension, including .rs and .html. Preserves exact bytes and existing native content identity; reads back to verify before returning a SHA-256 receipt. For targeted edits, use apply_patch. Files in temporary storage are not promised to survive reboot.".into(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, Some(vec!["path".into(), "content".into()]), Some(false.into())),
        output_schema: Some(json!({"type":"object","properties":{
            "path":{"type":"string"},"bytes":{"type":"integer"},"sha256":{"type":"string"},
            "content_type":{"type":["integer","null"]},"verified":{"type":"boolean"}
        },"required":["path","bytes","sha256","content_type","verified"],"additionalProperties":false}).into()),
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn write_options(args: &WriteArgs) -> Result<WriteFileOptions, FunctionCallError> {
    if args.content.len() > MAX_TEXT_BYTES {
        return Err(text_error(
            "write_file content exceeds the 1048576-byte limit",
        ));
    }
    match args.validate.as_deref() {
        None => {}
        Some("json") => {
            // RawValue validates JSON syntax without an f64 conversion or reserialization.
            let _: &serde_json::value::RawValue = serde_json::from_str(&args.content)
                .map_err(|error| text_error(format!("JSON validation failed: {error}")))?;
        }
        Some(other) => {
            return Err(text_error(format!(
                "unsupported validator `{other}`; omit validate or use `json`"
            )));
        }
    }
    let expected_sha256 = match (args.overwrite, args.expected_sha256.as_deref()) {
        (false, None) => None,
        (false, Some(_)) => return Err(text_error("expected_sha256 requires overwrite:true")),
        (true, None) => {
            return Err(text_error(
                "overwrite:true requires expected_sha256 from read_file",
            ));
        }
        (true, Some(hex)) => {
            if hex.len() != 64 || !hex.is_ascii() {
                return Err(text_error(
                    "expected_sha256 must contain 64 hexadecimal characters",
                ));
            }
            let mut digest = [0_u8; 32];
            for (index, byte) in digest.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).map_err(|_| {
                    text_error("expected_sha256 must contain 64 hexadecimal characters")
                })?;
            }
            Some(digest)
        }
    };
    Ok(WriteFileOptions {
        follow_symlinks: false,
        create_new: !args.overwrite,
        expected_sha256,
    })
}

async fn read_bounded(
    fs: &dyn ExecutorFileSystem,
    path: &PathUri,
    max_bytes: usize,
    sandbox: Option<&FileSystemSandboxContext>,
) -> io::Result<Vec<u8>> {
    let stream = fs.read_file_stream(path, sandbox).await?;
    collect_bounded(stream, max_bytes).await
}

async fn collect_bounded(
    mut stream: codex_file_system::FileSystemReadStream,
    max_bytes: usize,
) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if chunk.len() > max_bytes.saturating_sub(bytes.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "file exceeds the {max_bytes}-byte read limit; no truncated content was returned"
                ),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

impl ToolExecutor<ToolInvocation> for ReadFileHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("read_file")
    }
    fn spec(&self) -> ToolSpec {
        read_spec(self.include_environment_id)
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
                return Err(text_error("read_file requires function arguments"));
            };
            let args: ReadArgs = parse_arguments(arguments)?;
            let max_bytes = args.max_bytes.unwrap_or(DEFAULT_READ_BYTES);
            if !(1..=MAX_TEXT_BYTES).contains(&max_bytes) {
                return Err(text_error("max_bytes must be between 1 and 1048576"));
            }
            let environment = resolve_tool_environment(
                &invocation.step_context.environments,
                args.environment_id.as_deref(),
            )?;
            let path = environment.cwd().join(&args.path).map_err(text_error)?;
            let granted_permissions = invocation
                .step_context
                .turn
                .granted_permissions(&environment.selection.environment_id);
            let sandbox = environment.sandbox_context(granted_permissions);
            let fs = environment.environment.get_filesystem();
            let metadata = fs
                .get_metadata(&path, GetMetadataOptions::default(), Some(&sandbox))
                .await
                .map_err(text_error)?;
            if !metadata.is_file {
                return Err(text_error("read_file path is not a regular file"));
            }
            if metadata.size > max_bytes as u64 {
                return Err(text_error(format!(
                    "file exceeds the {max_bytes}-byte read limit"
                )));
            }
            let bytes = read_bounded(fs.as_ref(), &path, max_bytes, Some(&sandbox))
                .await
                .map_err(text_error)?;
            let hash = sha256_hex(&bytes);
            let count = bytes.len();
            let content = String::from_utf8(bytes)
                .map_err(|error| text_error(format!("file is not valid UTF-8: {error}")))?;
            Ok(boxed_tool_output(JsonToolOutput::new(json!({
                "path":path.inferred_native_path_string(),"content":content,"bytes":count,
                "sha256":hash,"content_type":metadata.content_type,"truncated":false,
            }))))
        })
    }
}

impl CoreToolRuntime for ReadFileHandler {
    fn is_builtin_control_tool(&self) -> bool {
        true
    }
}

struct WriteRequest {
    environment: TurnEnvironment,
    path: PathUri,
    content: String,
    options: WriteFileOptions,
    approval_requirement: ExecApprovalRequirement,
    additional_permissions: Option<AdditionalPermissionProfile>,
    permissions_preapproved: bool,
    change: FileChange,
}

struct WriteRuntime;

impl Approvable<WriteRequest> for WriteRuntime {
    fn exec_approval_requirement(&self, req: &WriteRequest) -> Option<ExecApprovalRequirement> {
        Some(req.approval_requirement.clone())
    }
    fn wants_no_sandbox_approval(&self, policy: AskForApproval) -> bool {
        !matches!(policy, AskForApproval::Never)
            && !matches!(policy, AskForApproval::Granular(config) if !config.allows_sandbox_approval())
    }
    fn approval_action(&self, req: &WriteRequest, call_id: &str) -> io::Result<ApprovalAction> {
        // Reuse the existing file-mutation review, including per-path approval caching.
        // The JSON operation records exact content and newline semantics; it is never executed as a patch.
        let operation = json!({"operation":"write_file","path":req.path.inferred_native_path_string(),
            "content":req.content,"bytes":req.content.len(),"sha256":sha256_hex(req.content.as_bytes()),
            "create_new":req.options.create_new,"expected_sha256":req.options.expected_sha256.map(|hash| hash.iter().map(|byte| format!("{byte:02x}")).collect::<String>())});
        Ok(ApprovalAction::ApplyPatch {
            id: call_id.to_string(),
            environment_id: req.environment.selection.environment_id.clone(),
            cwd: req.environment.cwd().clone(),
            files: vec![req.path.clone()],
            patch: operation.to_string(),
            changes: Arc::new(HashMap::from([(
                req.path.to_path_buf(),
                req.change.clone(),
            )])),
            permissions_preapproved: req.permissions_preapproved,
        })
    }
}

impl Sandboxable for WriteRuntime {
    fn sandbox_preference(&self) -> SandboxablePreference {
        SandboxablePreference::Auto
    }
    // A failed write or verification must never be replayed: it may have committed already.
    fn escalate_on_failure(&self) -> bool {
        false
    }
}

impl ToolRuntime<WriteRequest, Value> for WriteRuntime {
    fn turn_environment<'a>(&self, req: &'a WriteRequest) -> &'a TurnEnvironment {
        &req.environment
    }
    fn uses_executor_managed_process_sandbox(&self, req: &WriteRequest) -> bool {
        req.environment.environment.is_remote()
            || codex_exec_server::uses_native_file_system_sandbox()
    }
    fn sandbox_cwd<'a>(&self, req: &'a WriteRequest) -> Option<&'a PathUri> {
        Some(req.environment.cwd())
    }
    async fn run(
        &mut self,
        req: &WriteRequest,
        attempt: &SandboxAttempt<'_>,
        _ctx: &ToolCtx,
    ) -> Result<Value, ToolError> {
        let sandbox = attempt.sandbox_requested.then(|| FileSystemSandboxContext {
            sandbox_override: attempt.sandbox_override,
            permissions: effective_permission_profile(
                attempt.exec_server_permissions,
                req.additional_permissions.as_ref(),
            ),
            cwd: attempt.sandbox_cwd.clone(),
            workspace_roots: attempt.workspace_roots.to_vec(),
            user_home_dir: req.environment.user_home_dir.clone(),
            temporary_directories: None,
            windows_sandbox_selection: executor_windows_sandbox_selection(
                attempt.windows_sandbox_type,
                attempt.windows_sandbox_level,
                attempt.sandbox_cwd,
            ),
            windows_sandbox_proxy_settings_mode: None,
            use_legacy_landlock: attempt.use_legacy_landlock,
        });
        let fs = req.environment.environment.get_filesystem();
        fs.write_file(
            &req.path,
            req.content.as_bytes().to_vec(),
            req.options,
            sandbox.as_ref(),
        )
        .await
        .map_err(|error| ToolError::Rejected(format!("write_file failed: {error}")))?;
        // A verification failure is not reported as an uncommitted write.
        let bytes = read_bounded(fs.as_ref(), &req.path, MAX_TEXT_BYTES, sandbox.as_ref())
            .await
            .map_err(|error| {
                ToolError::Rejected(format!(
                    "write committed, but readback verification failed: {error}"
                ))
            })?;
        if bytes != req.content.as_bytes() {
            return Err(ToolError::Rejected("write committed, but readback content differs (the file may have changed concurrently)".into()));
        }
        let metadata = fs
            .get_metadata(
                &req.path,
                GetMetadataOptions {
                    follow_symlinks: false,
                },
                sandbox.as_ref(),
            )
            .await
            .map_err(|error| {
                ToolError::Rejected(format!(
                    "write content verified, but metadata lookup failed: {error}"
                ))
            })?;
        Ok(
            json!({"path":req.path.inferred_native_path_string(),"bytes":bytes.len(),
            "sha256":sha256_hex(&bytes),"content_type":metadata.content_type,"verified":true}),
        )
    }
}

impl ToolExecutor<ToolInvocation> for WriteFileHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("write_file")
    }
    fn spec(&self) -> ToolSpec {
        write_spec(self.include_environment_id)
    }
    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = &invocation.payload else {
                return Err(text_error("write_file requires function arguments"));
            };
            let args: WriteArgs = parse_arguments(arguments)?;
            let options = write_options(&args)?;
            let environment = resolve_tool_environment(
                &invocation.step_context.environments,
                args.environment_id.as_deref(),
            )?
            .clone();
            let path = environment.cwd().join(&args.path).map_err(text_error)?;
            let granted_permissions = invocation
                .step_context
                .turn
                .granted_permissions(&environment.selection.environment_id);
            let sandbox = environment.sandbox_context(granted_permissions);
            let policy_context =
                file_system_sandbox_policy_context_for_cwd(&sandbox, environment.cwd());
            let route = if environment.environment.is_remote()
                || codex_exec_server::uses_native_file_system_sandbox()
            {
                PatchSandboxRoute::ExecutorManaged
            } else {
                PatchSandboxRoute::Platform(windows_sandbox_level_for_legacy_checks(
                    environment.config().windows_sandbox_type,
                    environment.config().windows_sandbox_level,
                ))
            };
            let granted = invocation
                .step_context
                .turn
                .granted_permissions(&environment.selection.environment_id);
            let policy = effective_file_system_sandbox_policy(
                &environment
                    .permission_profile()
                    .file_system_sandbox_policy(),
                granted.as_ref(),
            );
            let matching = route
                .prepare_matching(&policy, &policy_context)
                .map_err(text_error)?;
            let requirement = match assess_write_safety(
                matching.can_write_path(&path).map_err(text_error)?,
                invocation.step_context.settings.approval_policy(),
                environment.permission_profile(),
                &matching,
            )
            .map_err(text_error)?
            {
                SafetyCheck::AutoApprove => ExecApprovalRequirement::Skip {
                    bypass_sandbox: false,
                    proposed_execpolicy_amendment: None,
                },
                SafetyCheck::AskUser => ExecApprovalRequirement::NeedsApproval {
                    reason: None,
                    proposed_execpolicy_amendment: None,
                },
                SafetyCheck::Reject { reason } => return Err(text_error(reason)),
            };
            let additional = write_permissions_for_paths(std::slice::from_ref(&path), &matching)
                .map_err(text_error)?;
            let effective = apply_granted_turn_permissions(
                &invocation.step_context,
                &environment,
                environment.cwd(),
                crate::sandboxing::SandboxPermissions::UseDefault,
                additional,
            );
            let change = if let Some(expected) = options.expected_sha256 {
                let fs = environment.environment.get_filesystem();
                let previous = read_bounded(fs.as_ref(), &path, MAX_TEXT_BYTES, Some(&sandbox))
                    .await
                    .map_err(|error| {
                        text_error(format!(
                            "unable to read replacement target for review: {error}"
                        ))
                    })?;
                let actual: [u8; 32] = Sha256::digest(&previous).into();
                if actual != expected {
                    return Err(text_error(
                        "replacement target changed: expected_sha256 does not match; read_file again before replacing",
                    ));
                }
                let previous = String::from_utf8(previous).map_err(|error| {
                    text_error(format!("replacement target is not UTF-8: {error}"))
                })?;
                let display_path = path.inferred_native_path_string();
                FileChange::Update {
                    unified_diff: similar::TextDiff::configure()
                        .timeout(Duration::from_millis(200))
                        .diff_lines(&previous, &args.content)
                        .unified_diff()
                        .header(&display_path, &display_path)
                        .to_string(),
                    move_path: None,
                }
            } else {
                FileChange::Add {
                    content: args.content.clone(),
                }
            };
            let request = WriteRequest {
                environment,
                path,
                content: args.content,
                options,
                approval_requirement: requirement,
                additional_permissions: effective.additional_permissions,
                permissions_preapproved: effective.permissions_preapproved,
                change,
            };
            let ctx = ToolCtx {
                session: invocation.session,
                step_context: invocation.step_context,
                cancellation_token: invocation.cancellation_token,
                call_id: invocation.call_id,
                tool_name: invocation.tool_name,
            };
            let result = ToolOrchestrator::new()
                .run(&mut WriteRuntime, &request, &ctx)
                .await
                .map_err(|error| match error {
                    ToolError::Rejected(message) => text_error(message),
                    ToolError::Codex(error) => text_error(error),
                })?;
            Ok(boxed_tool_output(JsonToolOutput::new(result.output)))
        })
    }
}

impl CoreToolRuntime for WriteFileHandler {
    fn is_builtin_control_tool(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(content: &str) -> WriteArgs {
        WriteArgs {
            path: "arbitrary.rs".into(),
            content: content.into(),
            overwrite: false,
            expected_sha256: None,
            validate: None,
            environment_id: None,
        }
    }

    #[test]
    fn exact_text_and_unfinished_documents_need_no_language_validator() {
        for content in [
            "",
            "hello codex",
            "hello codex\r\n",
            "fn main() {",
            "<html><body>",
            "a,b\n001,\"x,y\"\n",
            "§你好",
        ] {
            let arguments = args(content);
            assert!(write_options(&arguments).unwrap().create_new);
            let encoded = serde_json::to_string(&json!({"path":"x.rs","content":content})).unwrap();
            let decoded: WriteArgs = parse_arguments(&encoded).unwrap();
            assert_eq!(decoded.content.as_bytes(), content.as_bytes());
        }
    }

    #[test]
    fn json_validation_preserves_large_numbers_and_rejects_trailing_data() {
        let content =
            "{\"big\":9007199254740993123456789,\"decimal\":0.100000000000000000001,\"exp\":1e400}";
        let mut arguments = args(content);
        arguments.validate = Some("json".into());
        assert!(write_options(&arguments).is_ok());
        assert_eq!(arguments.content, content);
        for content in ["{", "{} trailing", "{} {}"] {
            arguments.content = content.into();
            assert!(write_options(&arguments).is_err());
        }
    }

    #[test]
    fn replacement_requires_a_valid_expected_hash() {
        let mut arguments = args("replacement");
        arguments.overwrite = true;
        assert!(write_options(&arguments).is_err());
        arguments.expected_sha256 = Some(sha256_hex(b"old"));
        assert_eq!(
            write_options(&arguments).unwrap().expected_sha256,
            Some(Sha256::digest(b"old").into())
        );
        arguments.expected_sha256 = Some("z".repeat(64));
        assert!(write_options(&arguments).is_err());
        arguments.overwrite = false;
        assert!(write_options(&arguments).is_err());
    }

    #[tokio::test]
    async fn streamed_reads_enforce_the_limit_even_when_metadata_is_stale() {
        let stream = codex_file_system::FileSystemReadStream::new(futures::stream::iter([
            Ok(Vec::from(&b"123"[..]).into()),
            Ok(Vec::from(&b"456"[..]).into()),
        ]));
        let error = collect_bounded(stream, 5).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let stream = codex_file_system::FileSystemReadStream::new(futures::stream::iter([
            Ok(Vec::from(&b"hello"[..]).into()),
            Ok(Vec::from(&b" codex"[..]).into()),
        ]));
        assert_eq!(collect_bounded(stream, 11).await.unwrap(), b"hello codex");
    }

    #[test]
    fn oversized_write_rejected_before_storage() {
        assert!(write_options(&args(&"x".repeat(MAX_TEXT_BYTES + 1))).is_err());
    }
}
