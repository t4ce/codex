use super::*;
use std::collections::VecDeque;
use trueos::command_session::Error;
use trueos::command_session::Read;
use trueos::command_session::Transport;

#[derive(Default)]
struct Observed {
    script: String,
    input: Vec<u8>,
    geometry: Option<Geometry>,
    signals: Vec<Signal>,
    stdin_closed: usize,
    closed: usize,
    finish: bool,
    stall_write: bool,
}
struct FakeBackend(Arc<Mutex<Observed>>);
struct FakeTransport {
    observed: Arc<Mutex<Observed>>,
    output: VecDeque<(Stream, Vec<u8>)>,
}
impl Backend for FakeBackend {
    fn open(&self, launch: Launch) -> std::result::Result<Box<dyn Transport>, Error> {
        let mut observed = self.0.lock().unwrap();
        observed.script = launch.script;
        observed.geometry = launch.terminal;
        drop(observed);
        Ok(Box::new(FakeTransport {
            observed: Arc::clone(&self.0),
            output: [
                (Stream::Stdout, b"first".to_vec()),
                (Stream::Stderr, b"diagnostic".to_vec()),
            ]
            .into(),
        }))
    }
}
impl Transport for FakeTransport {
    fn read(&mut self, out: &mut [u8]) -> std::result::Result<Read, Error> {
        if let Some((stream, bytes)) = self.output.pop_front() {
            out[..bytes.len()].copy_from_slice(&bytes);
            return Ok(Read {
                len: bytes.len(),
                stream,
                state: State::Running,
            });
        }
        let state = self.observed.lock().unwrap();
        if state.finish {
            let bytes = b"tail";
            out[..bytes.len()].copy_from_slice(bytes);
            Ok(Read {
                len: bytes.len(),
                stream: Stream::Stdout,
                state: State::Exited(7),
            })
        } else {
            Ok(Read {
                len: 0,
                stream: Stream::Stdout,
                state: State::Running,
            })
        }
    }
    fn write(&mut self, bytes: &[u8]) -> std::result::Result<usize, Error> {
        let mut state = self.observed.lock().unwrap();
        if std::mem::take(&mut state.stall_write) {
            return Ok(0);
        }
        let count = bytes.len().min(2);
        state.input.extend_from_slice(&bytes[..count]);
        Ok(count)
    }
    fn close_stdin(&mut self) -> std::result::Result<(), Error> {
        self.observed.lock().unwrap().stdin_closed += 1;
        Ok(())
    }
    fn resize(&mut self, geometry: Geometry) -> std::result::Result<(), Error> {
        self.observed.lock().unwrap().geometry = Some(geometry);
        Ok(())
    }
    fn signal(&mut self, signal: Signal) -> std::result::Result<(), Error> {
        self.observed.lock().unwrap().signals.push(signal);
        Ok(())
    }
    fn close(&mut self) {
        self.observed.lock().unwrap().closed += 1;
    }
}

#[tokio::test]
async fn native_script_input_controls_and_final_output_survive_the_adapter() {
    tokio::time::timeout(Duration::from_secs(2), async {
        let observed = Arc::new(Mutex::new(Observed {
            stall_write: true,
            ..Default::default()
        }));
        let backend = FakeBackend(Arc::clone(&observed));
        let mut spawned = spawn_with_backend(
            &backend,
            "\tstatus\n",
            Some(TerminalSize {
                rows: 25,
                cols: 100,
            }),
            true,
        )
        .await
        .unwrap();
        assert_eq!(observed.lock().unwrap().script, "\tstatus\n");
        assert_eq!(spawned.stdout_rx.recv().await.unwrap(), b"first");
        assert_eq!(spawned.stderr_rx.recv().await.unwrap(), b"diagnostic");
        spawned
            .session
            .writer_sender()
            .send(b"input".to_vec())
            .await
            .unwrap();
        loop {
            if observed.lock().unwrap().input == b"input" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        spawned
            .session
            .resize(TerminalSize {
                rows: 30,
                cols: 120,
            })
            .unwrap();
        spawned.session.signal(ProcessSignal::Interrupt).unwrap();
        assert_eq!(
            observed.lock().unwrap().geometry,
            Some(Geometry {
                rows: 30,
                cols: 120
            })
        );
        assert_eq!(observed.lock().unwrap().signals, [Signal::Interrupt]);
        spawned.session.close_stdin();
        loop {
            if observed.lock().unwrap().stdin_closed == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        observed.lock().unwrap().finish = true;
        assert_eq!(spawned.exit_rx.await.unwrap(), 7);
        assert_eq!(spawned.stdout_rx.recv().await.unwrap(), b"tail");
        assert!(spawned.stdout_rx.recv().await.is_none());
        assert!(spawned.stderr_rx.recv().await.is_none());
        assert_eq!(spawned.session.exit_code(), Some(7));
        drop(spawned.session);
        loop {
            if observed.lock().unwrap().closed == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn rejected_script_does_not_allocate_a_session() {
    let observed = Arc::new(Mutex::new(Observed::default()));
    let backend = FakeBackend(Arc::clone(&observed));
    assert!(
        spawn_with_backend(&backend, "invalid\0script", None, false)
            .await
            .is_err()
    );
    assert!(observed.lock().unwrap().script.is_empty());
}

#[tokio::test]
async fn missing_backend_is_an_explicit_error() {
    let error = spawn_script("\tstatus", None, false).await.unwrap_err();
    assert!(error.to_string().contains("not installed"));
}
