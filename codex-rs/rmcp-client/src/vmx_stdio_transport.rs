//! MCP protocol bytes carried by a fresh hypervisor VM, never a terminal.
use crate::bounded_stdio_transport::MAX_MCP_STDIO_LINE_BYTES;
use crate::local_stdio_transport::Discovery;
use crate::local_stdio_transport::DiscoveryOutcome;
use rmcp::service::RoleClient;
use rmcp::service::RxJsonRpcMessage;
use rmcp::service::TxJsonRpcMessage;
use rmcp::transport::Transport;
use std::future::Future;
use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::Mutex;
const CHUNK: usize = 3000;
const POLL: Duration = Duration::from_millis(10);
trait VmChannel: Send + Sync {
    fn write(&self, bytes: &[u8]) -> io::Result<usize>;
    fn read(&self, bytes: &mut [u8]) -> io::Result<usize>;
    fn running(&self) -> io::Result<bool>;
    fn terminate(&self) -> io::Result<()>;
}
struct Lifetime {
    channel: Box<dyn VmChannel>,
    stopped: AtomicBool,
    writer: Mutex<()>,
}
impl Drop for Lifetime {
    fn drop(&mut self) {
        if !self.stopped.swap(true, Ordering::AcqRel) {
            let _ = self.channel.terminate();
        }
    }
}
#[derive(Clone)]
pub(super) struct VmxProcess(Arc<Lifetime>);
impl VmxProcess {
    pub(super) fn terminate(&self) -> io::Result<()> {
        if !self.0.stopped.swap(true, Ordering::AcqRel) {
            if let Err(error) = self.0.channel.terminate() {
                self.0.stopped.store(false, Ordering::Release);
                return Err(error);
            }
        }
        Ok(())
    }
}
pub(super) struct VmxStdioTransport {
    process: VmxProcess,
    pending: Vec<u8>,
    discovery: Option<Discovery>,
}
impl VmxStdioTransport {
    fn with_channel(channel: Box<dyn VmChannel>) -> Self {
        Self {
            process: VmxProcess(Arc::new(Lifetime {
                channel,
                stopped: AtomicBool::new(false),
                writer: Mutex::new(()),
            })),
            pending: Vec::new(),
            discovery: None,
        }
    }
    #[cfg(any(target_os = "trueos", test))]
    pub(super) fn spawn(request: &[u8]) -> io::Result<Self> {
        let handle = trueos::child_hull::spawn_program(request).map_err(vm_error)?;
        Ok(Self::with_channel(Box::new(KernelChannel(handle))))
    }
    pub(super) fn watch_discovery(
        &mut self,
        timeout: Option<Duration>,
    ) -> tokio::sync::oneshot::Receiver<DiscoveryOutcome> {
        let (discovery, receiver) = Discovery::new(timeout);
        self.discovery = Some(discovery);
        receiver
    }

    pub(super) fn process(&self) -> VmxProcess {
        self.process.clone()
    }
    async fn send_bytes(process: VmxProcess, bytes: &[u8]) -> io::Result<()> {
        let _writer = process.0.writer.lock().await;
        let mut offset = 0;
        while offset < bytes.len() {
            if process.0.stopped.load(Ordering::Acquire) {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            let end = bytes.len().min(offset + CHUNK);
            match process.0.channel.write(&bytes[offset..end]) {
                Ok(n) if n > 0 && n <= end - offset => offset += n,
                Ok(_) => return Err(io::ErrorKind::WriteZero.into()),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => tokio::time::sleep(POLL).await,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    async fn receive_message(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        loop {
            if let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
                if end > MAX_MCP_STDIO_LINE_BYTES {
                    let _ = self.process.terminate();
                    return None;
                }
                let line: Vec<_> = self.pending.drain(..=end).collect();
                if let Ok(message) = serde_json::from_slice(&line) {
                    return Some(message);
                }
                continue;
            }
            if self.pending.len() > MAX_MCP_STDIO_LINE_BYTES {
                let _ = self.process.terminate();
                return None;
            }
            let mut bytes = [0; CHUNK];
            match self.process.0.channel.read(&mut bytes) {
                Ok(n) if n > 0 && n <= bytes.len() => self.pending.extend_from_slice(&bytes[..n]),
                Ok(0) => {
                    if self.process.0.stopped.load(Ordering::Acquire)
                        || !self.process.0.channel.running().ok()?
                    {
                        return serde_json::from_slice(&std::mem::take(&mut self.pending)).ok();
                    }
                    tokio::time::sleep(POLL).await;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => tokio::time::sleep(POLL).await,
                _ => {
                    let _ = self.process.terminate();
                    return None;
                }
            }
        }
    }
}
impl Transport<RoleClient> for VmxStdioTransport {
    type Error = io::Error;
    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = io::Result<()>> + Send + 'static {
        if self.discovery.as_ref().is_some_and(Discovery::is_closed) {
            self.discovery = None;
        }
        if let Some(discovery) = &mut self.discovery {
            discovery.record_outgoing(&item);
        }
        let process = self.process();
        async move {
            let mut bytes = serde_json::to_vec(&item).map_err(io::Error::other)?;
            if bytes.len() > MAX_MCP_STDIO_LINE_BYTES {
                return Err(io::ErrorKind::InvalidInput.into());
            }
            bytes.push(b'\n');
            Self::send_bytes(process, &bytes).await
        }
    }
    fn receive(&mut self) -> impl Future<Output = Option<RxJsonRpcMessage<RoleClient>>> + Send {
        async move {
            loop {
                let retry_at = self.discovery.as_ref().and_then(Discovery::retry_at);
                let receive = self.receive_message();
                let mut message = match retry_at {
                    Some(deadline) => tokio::time::timeout_at(deadline, receive)
                        .await
                        .unwrap_or_default(),
                    None => receive.await,
                };
                if self
                    .discovery
                    .as_mut()
                    .is_some_and(|probe| probe.handle_reply(&mut message))
                {
                    continue;
                }
                return message;
            }
        }
    }
    async fn close(&mut self) -> io::Result<()> {
        self.process.terminate()
    }
}
#[cfg(any(target_os = "trueos", test))]
struct KernelChannel(u64);
#[cfg(any(target_os = "trueos", test))]
fn vm_error(code: i32) -> io::Error {
    let kind = if code == trueos::child_hull::ERR_QUEUE_FULL {
        io::ErrorKind::WouldBlock
    } else {
        io::ErrorKind::BrokenPipe
    };
    io::Error::new(kind, format!("VMX child channel error {code}"))
}
#[cfg(any(target_os = "trueos", test))]
impl VmChannel for KernelChannel {
    fn write(&self, bytes: &[u8]) -> io::Result<usize> {
        trueos::child_hull::send(self.0, bytes).map_err(vm_error)
    }
    fn read(&self, bytes: &mut [u8]) -> io::Result<usize> {
        trueos::child_hull::read(self.0, bytes).map_err(vm_error)
    }
    fn running(&self) -> io::Result<bool> {
        match trueos::child_hull::status(self.0) {
            1 | 2 => Ok(true),
            3 | 4 => Ok(false),
            code => Err(vm_error(code)),
        }
    }
    fn terminate(&self) -> io::Result<()> {
        trueos::child_hull::terminate(self.0).map_err(vm_error)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::AtomicUsize;
    #[derive(Default)]
    struct State {
        input: StdMutex<VecDeque<u8>>,
        output: StdMutex<Vec<u8>>,
        stopped: AtomicUsize,
        block: AtomicBool,
    }
    struct Fake(Arc<State>);
    impl VmChannel for Fake {
        fn write(&self, bytes: &[u8]) -> io::Result<usize> {
            if self.0.block.swap(false, Ordering::Relaxed) {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let n = bytes.len().min(7);
            self.0.output.lock().unwrap().extend_from_slice(&bytes[..n]);
            Ok(n)
        }
        fn read(&self, out: &mut [u8]) -> io::Result<usize> {
            let mut input = self.0.input.lock().unwrap();
            let n = input.len().min(out.len()).min(3);
            for b in &mut out[..n] {
                *b = input.pop_front().unwrap();
            }
            Ok(n)
        }
        fn running(&self) -> io::Result<bool> {
            Ok(false)
        }
        fn terminate(&self) -> io::Result<()> {
            self.0.stopped.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }
    #[tokio::test]
    async fn fragmented_frames_backpressure_and_exact_vm_lifetime() {
        let state = Arc::new(State::default());
        state.block.store(true, Ordering::Relaxed);
        state
            .input
            .lock()
            .unwrap()
            .extend(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n");
        let mut transport = VmxStdioTransport::with_channel(Box::new(Fake(state.clone())));
        let process = transport.process();
        let bytes = vec![b'x'; CHUNK * 3 + 1];
        VmxStdioTransport::send_bytes(process.clone(), &bytes)
            .await
            .unwrap();
        assert_eq!(*state.output.lock().unwrap(), bytes);
        assert!(transport.receive().await.is_some());
        assert!(transport.receive().await.is_none());
        transport.close().await.unwrap();
        process.terminate().unwrap();
        drop(transport);
        drop(process);
        assert_eq!(state.stopped.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn drop_stops_the_owned_vm() {
        let state = Arc::new(State::default());
        drop(VmxStdioTransport::with_channel(Box::new(Fake(
            state.clone(),
        ))));
        assert_eq!(state.stopped.load(Ordering::Relaxed), 1);
    }
    fn discover_request(id: i64) -> TxJsonRpcMessage<RoleClient> {
        serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "server/discover", "params": {},
        }))
        .unwrap()
    }

    fn push_discovery_reply(state: &State, id: i64) {
        let reply = rmcp::model::ServerJsonRpcMessage::response(
            rmcp::model::ServerResult::DiscoverResult(
                rmcp::model::DiscoverResult::from_server_info(
                    vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                    rmcp::model::InitializeResult::default(),
                ),
            ),
            rmcp::model::RequestId::Number(id),
        );
        let mut bytes = serde_json::to_vec(&reply).unwrap();
        bytes.push(b'\n');
        state.input.lock().unwrap().extend(bytes);
    }

    #[tokio::test]
    async fn vmx_discovery_uses_upstream_reply_tracking_and_ignores_unrelated_ids() {
        let state = Arc::new(State::default());
        let mut transport = VmxStdioTransport::with_channel(Box::new(Fake(state.clone())));
        let outcome = transport.watch_discovery(None);
        transport.send(discover_request(7)).await.unwrap();
        state
            .input
            .lock()
            .unwrap()
            .extend(b"{\"jsonrpc\":\"2.0\",\"id\":99,\"result\":{}}\n");
        push_discovery_reply(&state, 7);
        assert!(transport.receive().await.is_some());
        drop(transport);
        assert!(matches!(outcome.await.unwrap(), DiscoveryOutcome::Reply));
        assert_eq!(state.stopped.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn vmx_late_discovery_retains_reply_for_fresh_handshake_selection() {
        let state = Arc::new(State::default());
        let mut transport = VmxStdioTransport::with_channel(Box::new(Fake(state.clone())));
        let outcome = transport.watch_discovery(None);
        transport.send(discover_request(7)).await.unwrap();
        let initialize = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0", "id": 8, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "native-vmx-test", "version": "1"}},
        }))
        .unwrap();
        transport.send(initialize).await.unwrap();
        push_discovery_reply(&state, 7);
        assert!(transport.receive().await.is_none());
        drop(transport);
        assert!(matches!(
            outcome.await.unwrap(),
            DiscoveryOutcome::Late {
                after_reoffer: false,
                ..
            }
        ));
        assert_eq!(state.stopped.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn vmx_discovery_eof_preserves_legacy_retry_signal() {
        let state = Arc::new(State::default());
        let mut transport = VmxStdioTransport::with_channel(Box::new(Fake(state.clone())));
        let outcome = transport.watch_discovery(None);
        transport.send(discover_request(7)).await.unwrap();
        assert!(transport.receive().await.is_none());
        drop(transport);
        assert!(matches!(
            outcome.await.unwrap(),
            DiscoveryOutcome::RetryLegacy
        ));
        assert_eq!(state.stopped.load(Ordering::Relaxed), 1);
    }
}
