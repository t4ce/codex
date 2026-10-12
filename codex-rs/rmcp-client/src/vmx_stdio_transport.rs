//! MCP protocol bytes carried by a fresh hypervisor VM, never a terminal.
use crate::bounded_stdio_transport::MAX_MCP_STDIO_LINE_BYTES;
use rmcp::{
    service::{RoleClient, RxJsonRpcMessage, TxJsonRpcMessage},
    transport::Transport,
};
use std::{
    future::Future,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
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
        }
    }
    #[cfg(any(target_os = "trueos", test))]
    pub(super) fn spawn(request: &[u8]) -> io::Result<Self> {
        let handle = trueos::child_hull::spawn_program(request).map_err(vm_error)?;
        Ok(Self::with_channel(Box::new(KernelChannel(handle))))
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
        self.receive_message()
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
    use std::{
        collections::VecDeque,
        sync::{Mutex as StdMutex, atomic::AtomicUsize},
    };
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
}
