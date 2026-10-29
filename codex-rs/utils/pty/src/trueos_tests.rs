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
    stdin_open: bool,
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
        observed.stdin_open = launch.stdin_open;
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

#[tokio::test]
async fn native_input_survives_separate_writes_without_terminal_geometry() {
    tokio::time::timeout(Duration::from_secs(2), async {
        let observed = Arc::new(Mutex::new(Observed::default()));
        let backend = FakeBackend(Arc::clone(&observed));
        let spawned = spawn_with_backend(&backend, "status", None, true)
            .await
            .unwrap();
        assert_eq!(observed.lock().unwrap().geometry, None);
        assert!(observed.lock().unwrap().stdin_open);
        for input in [b"status\n".as_slice(), b"exit\n".as_slice()] {
            // Per-call senders go away after yielding; the retained session owns input.
            spawned
                .session
                .writer_sender()
                .send(input.to_vec())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert_eq!(observed.lock().unwrap().stdin_closed, 0);
        }
        assert_eq!(observed.lock().unwrap().input, b"status\nexit\n");
        spawned.session.close_stdin();
        loop {
            if observed.lock().unwrap().stdin_closed == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            spawned
                .session
                .writer_sender()
                .send(b"later".to_vec())
                .await
                .is_err()
        );
        assert!(observed.lock().unwrap().signals.is_empty());
        observed.lock().unwrap().finish = true;
        assert_eq!(spawned.exit_rx.await.unwrap(), 7);
        drop(spawned.session);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn native_closed_stdin_rejects_writes_without_terminating_session() {
    tokio::time::timeout(Duration::from_secs(2), async {
        let observed = Arc::new(Mutex::new(Observed::default()));
        let backend = FakeBackend(Arc::clone(&observed));
        let spawned = spawn_with_backend(&backend, "status", None, false)
            .await
            .unwrap();
        assert!(!observed.lock().unwrap().stdin_open);
        assert_eq!(observed.lock().unwrap().stdin_closed, 1);
        let writer = spawned.session.writer_sender();
        while !writer.is_closed() {
            tokio::task::yield_now().await;
        }
        assert!(writer.send(b"later".to_vec()).await.is_err());
        assert!(observed.lock().unwrap().signals.is_empty());
        assert!(observed.lock().unwrap().input.is_empty());
        observed.lock().unwrap().finish = true;
        assert_eq!(spawned.exit_rx.await.unwrap(), 7);
        drop(spawned.session);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn captured_batches_and_explicit_sessions_use_their_actual_input_lifetimes() {
    tokio::time::timeout(Duration::from_secs(3), async {
        for (script, tty, input_open) in [
            ("adm ram\nhv status\n", false, false),
            ("ram", false, false),
            (" \n", false, false),
            ("", false, true),
            ("ram", true, true),
            ("", true, true),
        ] {
            let observed = Arc::new(Mutex::new(Observed::default()));
            let backend = FakeBackend(Arc::clone(&observed));
            let size = tty.then_some(TerminalSize {
                rows: 40,
                cols: 120,
            });
            // This is the same policy the production unified-exec branch uses.
            let spawned =
                spawn_with_backend(&backend, script, size, script_stdin_open(script, tty))
                    .await
                    .unwrap();
            {
                let state = observed.lock().unwrap();
                assert_eq!(state.script, script);
                assert_eq!(state.stdin_open, input_open, "{script:?}, tty={tty}");
                assert_eq!(state.geometry.is_some(), tty);
            }
            if input_open {
                spawned
                    .session
                    .writer_sender()
                    .send(b"env\n".to_vec())
                    .await
                    .unwrap();
                while observed.lock().unwrap().input != b"env\n" {
                    tokio::task::yield_now().await;
                }
                assert_eq!(observed.lock().unwrap().stdin_closed, 0);
                spawned.session.close_stdin();
            }
            while observed.lock().unwrap().stdin_closed != 1 {
                tokio::task::yield_now().await;
            }
            let writer = spawned.session.writer_sender();
            while !writer.is_closed() {
                tokio::task::yield_now().await;
            }
            assert!(writer.send(b"later\n".to_vec()).await.is_err());
            assert!(observed.lock().unwrap().signals.is_empty());
            let resized = spawned.session.resize(TerminalSize {
                rows: 50,
                cols: 160,
            });
            assert_eq!(resized.is_ok(), tty);
            assert_eq!(
                observed.lock().unwrap().geometry,
                if tty {
                    Some(Geometry {
                        rows: 50,
                        cols: 160,
                    })
                } else {
                    None
                }
            );
            observed.lock().unwrap().finish = true;
            assert_eq!(spawned.exit_rx.await.unwrap(), 7);
            drop(spawned.session);
        }
    })
    .await
    .unwrap();
}
