//! Client for the native Edit workspace broker. The broker routes by Blueprint
//! identity, so only one request may be outstanding from this VM at a time.
#[cfg(target_os = "trueos")]
use std::sync::atomic::AtomicU64;
#[cfg(target_os = "trueos")]
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;
use trueos_ide::protocol::ClientRequest;
use trueos_ide::protocol::Endpoint;
use trueos_ide::protocol::IdeRequest;
use trueos_ide::protocol::IdeResponse;
use trueos_ide::protocol::OwnerReply;
use trueos_ide::protocol::{self};

const DEFAULT_DATABASE: &str = "/apps/common/ide/projects.redb";

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenOptions {
    pub project: String,
    pub database: Option<String>,
    pub compiler: Option<String>,
    pub entry: Option<String>,
}

impl OpenOptions {
    fn database(&self) -> &str {
        self.database.as_deref().unwrap_or(DEFAULT_DATABASE)
    }

    fn script(&self) -> Result<String, String> {
        fn literal(value: &str) -> bool {
            !value.is_empty() && !value.contains(['\0', '\n', '\r'])
        }
        if !literal(&self.project) || !literal(self.database()) {
            return Err(
                "project and database must be nonempty literal values without newline or NUL"
                    .into(),
            );
        }
        if let Some(compiler) = &self.compiler {
            if !matches!(compiler.as_str(), "odin-son" | "rustc-min") {
                return Err("compiler must be odin-son or rustc-min".into());
            }
        }
        if let Some(entry) = &self.entry {
            if !literal(entry)
                || entry.starts_with('/')
                || entry.contains(['\\', ':'])
                || entry.split('/').any(|part| matches!(part, "" | "." | ".."))
            {
                return Err(
                    "entry must be a relative project source path without newline or NUL".into(),
                );
            }
        }
        let mut script = String::new();
        let db = self.database();
        if db != DEFAULT_DATABASE && (db.starts_with('/') || db.starts_with("trueosfs:")) {
            script.push_str("fs-scope trueosfs\n");
        }
        script.push_str(&format!("ide-project {}\n", self.project));
        if let Some(database) = &self.database {
            script.push_str(&format!("ide-db {database}\n"));
        }
        if let Some(compiler) = &self.compiler {
            script.push_str(&format!("ide-compiler {compiler}\n"));
        }
        if let Some(entry) = &self.entry {
            script.push_str(&format!("ide-entry {entry}\n"));
        }
        Ok(script)
    }
}

static REQUEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
#[cfg(target_os = "trueos")]
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
#[cfg(target_os = "trueos")]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

trait Transport {
    async fn read(&self, path: &str) -> Result<Vec<u8>, String>;
    async fn write(&self, path: &str, data: &[u8]) -> Result<(), String>;
}

#[cfg(target_os = "trueos")]
struct NativeTransport;
#[cfg(target_os = "trueos")]
impl Transport for NativeTransport {
    async fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        trueos_v::vfs_async::read_file(path.as_bytes())
            .await
            .map_err(|status| format!("IDE broker read failed ({status})"))
    }
    async fn write(&self, path: &str, data: &[u8]) -> Result<(), String> {
        trueos_v::vfs_async::write_file(path.as_bytes(), data)
            .await
            .map_err(|status| format!("IDE broker write failed ({status})"))
    }
}

async fn read<T: Transport>(transport: &T, path: &str) -> Result<Vec<u8>, String> {
    let bytes = transport.read(path).await?;
    if bytes.len() > protocol::MAX_WIRE_BYTES {
        return Err("IDE broker response is too large".into());
    }
    Ok(bytes)
}
async fn discover_impl<T: Transport>(transport: &T) -> Result<Vec<Endpoint>, String> {
    let bytes = read(transport, protocol::DISCOVER).await?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid IDE discovery response: {error}"))
}
#[cfg(target_os = "trueos")]
pub async fn discover() -> Result<Vec<Endpoint>, String> {
    discover_impl(&NativeTransport).await
}

/// Attach to an existing workspace or launch Edit using its startup script.
#[cfg(target_os = "trueos")]
pub async fn open(options: OpenOptions) -> Result<(u64, IdeResponse), String> {
    let _guard = REQUEST_LOCK.lock().await;
    open_impl(
        &NativeTransport,
        &options,
        |script| {
            trueos_v::vshell::launch_with_destination(
                "edit",
                script,
                trueos_v::vshell::LaunchDestination::NewShell3,
            )
            .map_err(|status| format!("Could not launch Edit ({status})"))
        },
        NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed),
        Duration::from_secs(15),
    )
    .await
}

async fn open_impl<T: Transport>(
    transport: &T,
    options: &OpenOptions,
    launch: impl FnOnce(&str) -> Result<(), String>,
    request_id: u64,
    timeout: Duration,
) -> Result<(u64, IdeResponse), String> {
    let script = options.script()?;
    let mut launch = Some(launch);
    let deadline = Instant::now() + timeout;
    let mut launched = false;
    loop {
        let endpoints = discover_impl(transport).await?;
        let matches: Vec<_> = endpoints
            .iter()
            .filter(|endpoint| {
                endpoint.project == options.project && endpoint.database == options.database()
            })
            .collect();
        if matches.len() > 1 {
            return Err(
                "Multiple matching Edit workspaces are open; discover and select an endpoint"
                    .into(),
            );
        }
        if let Some(endpoint) = matches.first() {
            let (id, reply) = request_impl(
                transport,
                Some(endpoint.endpoint),
                IdeRequest::Context {},
                request_id,
                REQUEST_CONTEXT_TIMEOUT,
            )
            .await?;
            if let IdeResponse::Context { context } = &reply {
                let compiler = match context.compiler {
                    protocol::CompilerBackend::OdinSon => "odin-son",
                    protocol::CompilerBackend::RustcMin => "rustc-min",
                };
                if options
                    .compiler
                    .as_deref()
                    .is_some_and(|requested| requested != compiler)
                    || options
                        .entry
                        .as_deref()
                        .is_some_and(|requested| requested != context.entry)
                {
                    return Err("The matching Edit workspace has a different compiler or entry; use its current context or close it before opening with new settings".into());
                }
            }
            return Ok((id, reply));
        }
        if !launched {
            launch.take().expect("launch called once")(&script)?;
            launched = true;
        }
        if Instant::now() >= deadline {
            return Err("Edit launch was queued but its workspace did not appear before timeout; discover before retrying".into());
        }
        tokio::task::yield_now().await;
    }
}

const REQUEST_CONTEXT_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(target_os = "trueos")]
pub async fn request(endpoint: Option<u64>, command: IdeRequest) -> Result<IdeResponse, String> {
    request_with_endpoint(endpoint, command)
        .await
        .map(|(_, reply)| reply)
}
#[cfg(target_os = "trueos")]
pub async fn request_with_endpoint(
    endpoint: Option<u64>,
    command: IdeRequest,
) -> Result<(u64, IdeResponse), String> {
    let _guard = REQUEST_LOCK.lock().await;
    request_impl(
        &NativeTransport,
        endpoint,
        command,
        NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed),
        REQUEST_TIMEOUT,
    )
    .await
}

fn try_request_lock() -> Result<tokio::sync::MutexGuard<'static, ()>, String> {
    REQUEST_LOCK
        .try_lock()
        .map_err(|_| "An IDE request is already in progress; try /ide again shortly".into())
}

/// Fetch context without waiting for a tool sharing the TUI's executor.
#[cfg(target_os = "trueos")]
pub fn context_now() -> Result<(u64, IdeResponse), String> {
    let _guard = try_request_lock()?;
    trueos_v::vfs_async::block_on(request_impl(
        &NativeTransport,
        None,
        IdeRequest::Context {},
        NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed),
        REQUEST_TIMEOUT,
    ))
}

async fn request_impl<T: Transport>(
    transport: &T,
    endpoint: Option<u64>,
    command: IdeRequest,
    request_id: u64,
    timeout: Duration,
) -> Result<(u64, IdeResponse), String> {
    // Clear any ticket left by a previously dropped future before admission.
    transport
        .write(protocol::CANCEL, b"")
        .await
        .map_err(|error| format!("IDE broker could not clear a previous request: {error}"))?;
    let endpoints = discover_impl(transport).await?;
    let selected = match endpoint {
        Some(id) if endpoints.iter().any(|entry| entry.endpoint == id) => id,
        Some(_) => {
            return Err("IDE endpoint is no longer available; discover workspaces again".into());
        }
        None if endpoints.len() == 1 => endpoints[0].endpoint,
        None if endpoints.is_empty() => return Err("No Edit workspace is open".into()),
        None => {
            return Err(
                "Multiple Edit workspaces are open; discover them and select an endpoint".into(),
            );
        }
    };
    let data = protocol::encode(&ClientRequest {
        endpoint: selected,
        request_id,
        command,
    })?;
    transport
        .write(protocol::REQUEST, &data)
        .await
        .map_err(|error| format!("IDE request was not accepted: {error}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            let _ = transport.write(protocol::CANCEL, b"").await;
            return Err("IDE request timed out; its outcome may be unknown. Refresh context before retrying a mutation".into());
        }
        let bytes = match read(transport, protocol::REPLY).await {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = transport.write(protocol::CANCEL, b"").await;
                return Err(format!("{error}; request outcome may be unknown"));
            }
        };
        if bytes.is_empty() {
            tokio::task::yield_now().await;
            continue;
        }
        let reply: OwnerReply = match serde_json::from_slice(&bytes) {
            Ok(reply) => reply,
            Err(error) => {
                let _ = transport.write(protocol::CANCEL, b"").await;
                return Err(format!(
                    "invalid IDE reply: {error}; request outcome may be unknown"
                ));
            }
        };
        if reply.endpoint != selected || reply.request_id != request_id {
            let _ = transport.write(protocol::CANCEL, b"").await;
            return Err(
                "IDE broker returned a mismatched reply; refresh context before retrying".into(),
            );
        }
        return Ok((selected, reply.reply));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    fn options() -> OpenOptions {
        OpenOptions {
            project: "p".into(),
            database: Some("d".into()),
            compiler: None,
            entry: None,
        }
    }
    #[test]
    fn startup_script_rejects_injection_and_invalid_entry() {
        for project in ["", "p\nide-db injected", "p\r", "p\0"] {
            let mut opt = options();
            opt.project = project.into();
            assert!(opt.script().is_err());
        }
        for entry in ["../main.rs", "/main.rs", "src//main.rs", "main.rs\nopen x"] {
            let mut opt = options();
            opt.entry = Some(entry.into());
            assert!(opt.script().is_err());
        }
        let mut opt = options();
        opt.database = Some("trueosfs:disc7/path db.redb".into());
        opt.compiler = Some("rustc-min".into());
        opt.entry = Some("src/main.rs".into());
        assert_eq!(
            opt.script().unwrap(),
            "fs-scope trueosfs\nide-project p\nide-db trueosfs:disc7/path db.redb\nide-compiler rustc-min\nide-entry src/main.rs\n"
        );
    }
    #[tokio::test]
    async fn open_attaches_existing_without_relaunch() {
        let context = protocol::WorkspaceContext {
            project: "p".into(),
            database: "d".into(),
            compiler: protocol::CompilerBackend::OdinSon,
            entry: "main.odin".into(),
            revision_id: 1,
            active_file: None,
            open_files: vec![],
            selected_text: String::new(),
            build: protocol::BuildState::Idle,
        };
        let reply = OwnerReply {
            endpoint: 7,
            client_token: 12,
            request_id: 9,
            reply: IdeResponse::Context { context },
        };
        let mock = Mock {
            reads: RefCell::new(VecDeque::from([
                endpoints(&[7]),
                endpoints(&[7]),
                protocol::encode(&reply).unwrap(),
            ])),
            ..Default::default()
        };
        let result = open_impl(
            &mock,
            &options(),
            |_| panic!("existing workspace must not launch"),
            9,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(result.0, 7);
        assert!(matches!(result.1, IdeResponse::Context { .. }));
    }
    #[tokio::test]
    async fn open_launches_then_connects_to_registered_workspace() {
        let context = protocol::WorkspaceContext {
            project: "p".into(),
            database: "d".into(),
            compiler: protocol::CompilerBackend::OdinSon,
            entry: "main.odin".into(),
            revision_id: 1,
            active_file: None,
            open_files: vec![],
            selected_text: String::new(),
            build: protocol::BuildState::Idle,
        };
        let reply = OwnerReply {
            endpoint: 7,
            client_token: 12,
            request_id: 9,
            reply: IdeResponse::Context { context },
        };
        let mock = Mock {
            reads: RefCell::new(VecDeque::from([
                endpoints(&[]),
                endpoints(&[7]),
                endpoints(&[7]),
                protocol::encode(&reply).unwrap(),
            ])),
            ..Default::default()
        };
        let launches = std::cell::Cell::new(0);
        let result = open_impl(
            &mock,
            &options(),
            |script| {
                launches.set(launches.get() + 1);
                assert!(script.contains("ide-project p\n"));
                Ok(())
            },
            9,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(launches.get(), 1);
        assert_eq!(result.0, 7);
    }
    #[tokio::test]
    async fn open_launches_once_then_reports_unknown_on_timeout() {
        let mock = Mock {
            reads: RefCell::new(VecDeque::from([endpoints(&[])])),
            ..Default::default()
        };
        let count = std::cell::Cell::new(0);
        let error = open_impl(
            &mock,
            &options(),
            |_| {
                count.set(count.get() + 1);
                Ok(())
            },
            9,
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert_eq!(count.get(), 1);
        assert!(error.contains("discover before retrying"));
    }
    #[test]
    fn synchronous_context_does_not_wait_for_active_tool() {
        let _guard = REQUEST_LOCK.try_lock().unwrap();
        assert!(
            try_request_lock()
                .unwrap_err()
                .contains("already in progress")
        );
    }
    #[derive(Default)]
    struct Mock {
        reads: RefCell<VecDeque<Vec<u8>>>,
        writes: RefCell<Vec<(String, Vec<u8>)>>,
    }
    impl Transport for Mock {
        async fn read(&self, path: &str) -> Result<Vec<u8>, String> {
            assert!(path == protocol::DISCOVER || path == protocol::REPLY);
            Ok(self.reads.borrow_mut().pop_front().unwrap_or_default())
        }
        async fn write(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
            self.writes.borrow_mut().push((path.into(), bytes.into()));
            Ok(())
        }
    }
    fn endpoints(ids: &[u64]) -> Vec<u8> {
        protocol::encode(
            &ids.iter()
                .map(|id| Endpoint {
                    endpoint: *id,
                    project: "p".into(),
                    database: "d".into(),
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }
    #[tokio::test]
    async fn request_routes_and_checks_reply_identity() {
        let reply = OwnerReply {
            endpoint: 7,
            client_token: 12,
            request_id: 9,
            reply: IdeResponse::Saved { revision_id: 3 },
        };
        let mock = Mock {
            reads: RefCell::new(VecDeque::from([
                endpoints(&[7]),
                Vec::new(),
                protocol::encode(&reply).unwrap(),
            ])),
            ..Default::default()
        };
        assert_eq!(
            request_impl(
                &mock,
                None,
                IdeRequest::Context {},
                9,
                Duration::from_secs(1)
            )
            .await
            .unwrap(),
            (7, reply.reply)
        );
        let writes = mock.writes.borrow();
        assert_eq!(writes[0].0, protocol::CANCEL);
        assert_eq!(writes[1].0, protocol::REQUEST);
        let sent: ClientRequest = protocol::decode(&writes[1].1).unwrap();
        assert_eq!((sent.endpoint, sent.request_id), (7, 9));
    }
    #[tokio::test]
    async fn ambiguity_and_mismatched_reply_do_not_silently_route() {
        let mock = Mock {
            reads: RefCell::new(VecDeque::from([endpoints(&[7, 8])])),
            ..Default::default()
        };
        assert!(
            request_impl(
                &mock,
                None,
                IdeRequest::Context {},
                9,
                Duration::from_secs(1)
            )
            .await
            .unwrap_err()
            .contains("Multiple")
        );
        assert_eq!(mock.writes.borrow().len(), 1);
        let reply = OwnerReply {
            endpoint: 8,
            client_token: 12,
            request_id: 9,
            reply: IdeResponse::Cancelled {},
        };
        let mock = Mock {
            reads: RefCell::new(VecDeque::from([
                endpoints(&[7]),
                protocol::encode(&reply).unwrap(),
            ])),
            ..Default::default()
        };
        assert!(
            request_impl(
                &mock,
                Some(7),
                IdeRequest::Context {},
                9,
                Duration::from_secs(1)
            )
            .await
            .unwrap_err()
            .contains("mismatched")
        );
        assert_eq!(mock.writes.borrow().last().unwrap().0, protocol::CANCEL);
    }
    #[tokio::test]
    async fn timeout_cancels_ticket_and_reports_unknown_outcome() {
        let mock = Mock {
            reads: RefCell::new(VecDeque::from([endpoints(&[7])])),
            ..Default::default()
        };
        let error = request_impl(&mock, None, IdeRequest::Run {}, 9, Duration::ZERO)
            .await
            .unwrap_err();
        assert!(error.contains("outcome may be unknown"));
        assert_eq!(mock.writes.borrow().last().unwrap().0, protocol::CANCEL);
    }
}
