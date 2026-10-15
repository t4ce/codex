//! Native interpreter-backed Code Mode. Each cell owns a real QuickJS module;
//! session store/load values persist without exposing filesystem or network APIs.
mod engine;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use codex_code_mode_protocol::*;
use serde_json::Value;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct QuickJsCodeModeSessionProvider;

impl CodeModeSessionProvider for QuickJsCodeModeSessionProvider {
    fn create_session(&self) -> CodeModeSessionProviderFuture<'_> {
        self.create_session_with_limits(CodeModeSessionCellExecutionLimits::default())
    }

    fn create_session_with_limits<'a>(
        &'a self,
        limits: CodeModeSessionCellExecutionLimits,
    ) -> CodeModeSessionProviderFuture<'a> {
        Box::pin(async move {
            if limits.max_heap_size_bytes == Some(0) {
                return Err("QuickJS heap size limit must be positive".into());
            }
            Ok(Arc::new(Session {
                cells: Mutex::new(HashMap::new()),
                values: Arc::new(Mutex::new(HashMap::new())),
                next: AtomicU64::new(1),
                shutdown: CancellationToken::new(),
                limits,
            }) as Arc<dyn CodeModeSession>)
        })
    }
}

struct Session {
    cells: Mutex<HashMap<CellId, Arc<Cell>>>,
    values: Arc<Mutex<HashMap<String, Value>>>,
    next: AtomicU64,
    shutdown: CancellationToken,
    limits: CodeModeSessionCellExecutionLimits,
}

#[derive(Clone)]
enum Completion {
    Result(Option<String>),
    Terminated,
}

struct CellState {
    output: Vec<FunctionCallOutputContentItem>,
    completion: Option<Completion>,
    yield_requested: bool,
}

struct Cell {
    id: CellId,
    state: Mutex<CellState>,
    changed: watch::Sender<u64>,
    cancel: CancellationToken,
}

impl Cell {
    fn output(&self, item: FunctionCallOutputContentItem) {
        self.state.lock().unwrap().output.push(item);
        self.changed.send_modify(|n| *n = n.wrapping_add(1));
    }

    fn yield_control(&self) {
        self.state.lock().unwrap().yield_requested = true;
        self.changed.send_modify(|n| *n = n.wrapping_add(1));
    }

    fn finish(&self, completion: Completion) {
        self.state.lock().unwrap().completion = Some(completion);
        self.changed.send_modify(|n| *n = n.wrapping_add(1));
    }

    async fn observe(
        &self,
        timeout: Duration,
        preempt: Option<CancellationToken>,
    ) -> RuntimeResponse {
        let started = Instant::now();
        let mut changed = self.changed.subscribe();
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        loop {
            let completion = self.state.lock().unwrap().completion.clone();
            if completion.is_some() || self.state.lock().unwrap().yield_requested {
                break;
            }
            tokio::select! {
                _ = &mut deadline => break,
                _ = async { match &preempt {Some(token) => token.cancelled().await, None => std::future::pending().await} } => break,
                result = changed.changed() => { if result.is_err() { break; } }
            }
        }
        let mut state = self.state.lock().unwrap();
        state.yield_requested = false;
        let content_items = std::mem::take(&mut state.output);
        let cell_id = self.id.clone();
        let response = match &state.completion {
            Some(Completion::Result(error_text)) => RuntimeResponse::Result {
                cell_id,
                content_items,
                error_text: error_text.clone(),
                code_mode_host_duration: None,
            },
            Some(Completion::Terminated) => RuntimeResponse::Terminated {
                cell_id,
                content_items,
                code_mode_host_duration: None,
            },
            None => RuntimeResponse::Yielded {
                cell_id,
                content_items,
                code_mode_host_duration: None,
            },
        };
        response.with_code_mode_host_duration(started.elapsed())
    }

    async fn closed(&self) {
        let mut changed = self.changed.subscribe();
        while self.state.lock().unwrap().completion.is_none() {
            if changed.changed().await.is_err() {
                break;
            }
        }
    }
}

impl Session {
    fn duration(&self, millis: u64) -> Duration {
        Duration::from_millis(millis.min(self.limits.max_yield_time_ms.unwrap_or(u64::MAX)))
    }
}

impl CodeModeSession for Session {
    fn execute<'a>(
        &'a self,
        request: ExecuteRequest,
        delegate: Arc<dyn CodeModeSessionDelegate>,
        preempt: Option<CancellationToken>,
    ) -> CodeModeSessionResultFuture<'a, StartedCell> {
        Box::pin(async move {
            if self.shutdown.is_cancelled() {
                return Err("code mode session is shutting down".into());
            }
            let id = CellId::new(format!(
                "quickjs-{}",
                self.next.fetch_add(1, Ordering::Relaxed)
            ));
            let (changed, _) = watch::channel(0);
            let cell = Arc::new(Cell {
                id: id.clone(),
                changed,
                cancel: self.shutdown.child_token(),
                state: Mutex::new(CellState {
                    output: Vec::new(),
                    completion: None,
                    yield_requested: false,
                }),
            });
            {
                let mut cells = self.cells.lock().unwrap();
                if self.shutdown.is_cancelled() {
                    return Err("code mode session is shutting down".into());
                }
                if cells.len() >= 128 {
                    cells.retain(|_, cell| cell.state.lock().unwrap().completion.is_none());
                    if cells.len() >= 128 {
                        return Err("QuickJS session cell limit reached".into());
                    }
                }
                cells.insert(id.clone(), cell.clone());
            }
            let runtime = tokio::runtime::Handle::current();
            let values = self.values.clone();
            let worker_cell = cell.clone();
            let heap_limit = self.limits.max_heap_size_bytes.unwrap_or(64 * 1024 * 1024);
            let duration =
                self.duration(request.yield_time_ms.unwrap_or(DEFAULT_EXEC_YIELD_TIME_MS));
            if let Err(error) = std::thread::Builder::new()
                .name("code-mode-quickjs".into())
                .stack_size(2 * 1024 * 1024)
                .spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        engine::run(
                            request,
                            delegate.clone(),
                            runtime,
                            values,
                            worker_cell.clone(),
                            heap_limit,
                        )
                    }));
                    let cancelled = worker_cell.cancel.is_cancelled();
                    worker_cell.cancel.cancel();
                    let cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        delegate.cell_closed(&worker_cell.id);
                    }));
                    let completion = match result {
                        Ok(Ok(())) => Completion::Result(None),
                        Ok(Err(error)) => Completion::Result(Some(error)),
                        Err(_) => {
                            Completion::Result(Some("QuickJS execution thread panicked".into()))
                        }
                    };
                    let completion = if cleanup.is_err() {
                        Completion::Result(Some("code-mode delegate cleanup panicked".into()))
                    } else {
                        completion
                    };
                    worker_cell.finish(if cancelled {
                        Completion::Terminated
                    } else {
                        completion
                    });
                })
            {
                self.cells.lock().unwrap().remove(&id);
                return Err(format!("failed to start QuickJS execution thread: {error}"));
            }
            Ok(StartedCell::from_future(id, async move {
                Ok(cell.observe(duration, preempt).await)
            }))
        })
    }

    fn wait<'a>(
        &'a self,
        request: WaitRequest,
        preempt: Option<CancellationToken>,
    ) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(async move {
            let cell = self.cells.lock().unwrap().get(&request.cell_id).cloned();
            let Some(cell) = cell else {
                return Err(format!("unknown QuickJS cell {}", request.cell_id));
            };
            Ok(WaitOutcome::LiveCell(
                cell.observe(self.duration(request.yield_time_ms), preempt)
                    .await,
            ))
        })
    }

    fn terminate<'a>(&'a self, cell_id: CellId) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(async move {
            let cell = self
                .cells
                .lock()
                .unwrap()
                .get(&cell_id)
                .cloned()
                .ok_or_else(|| format!("unknown QuickJS cell {cell_id}"))?;
            cell.cancel.cancel();
            cell.closed().await;
            {
                cell.state.lock().unwrap().completion = Some(Completion::Terminated);
            }
            Ok(WaitOutcome::LiveCell(
                cell.observe(Duration::ZERO, None).await,
            ))
        })
    }

    fn shutdown<'a>(&'a self) -> CodeModeSessionResultFuture<'a, ()> {
        Box::pin(async move {
            self.shutdown.cancel();
            let cells: Vec<_> = self.cells.lock().unwrap().values().cloned().collect();
            for cell in cells {
                cell.closed().await;
            }
            self.cells.lock().unwrap().clear();
            Ok(())
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

#[cfg(test)]
mod tests;
