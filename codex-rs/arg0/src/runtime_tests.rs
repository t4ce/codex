use super::TRUEOS_MAX_BLOCKING_THREADS;
use super::build_runtime_for_platform;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

struct ReleaseOnDrop(Arc<(Mutex<bool>, Condvar)>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        let (released, wake) = &*self.0;
        *released.lock().unwrap() = true;
        wake.notify_all();
    }
}

fn exercise_blocking_pool(trueos: bool, expected_started: usize) {
    let runtime = build_runtime_for_platform(trueos).unwrap();
    // Drop the gate before the runtime even if an assertion fails, so blocked
    // workers cannot keep runtime shutdown waiting forever.
    let gate = ReleaseOnDrop(Arc::new((Mutex::new(false), Condvar::new())));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (started_tx, started_rx) = mpsc::channel();
    let jobs = TRUEOS_MAX_BLOCKING_THREADS * 3;
    let mut handles = Vec::new();
    for index in 0..jobs {
        let gate = Arc::clone(&gate.0);
        let active = Arc::clone(&active);
        let peak = Arc::clone(&peak);
        let started = started_tx.clone();
        handles.push(runtime.spawn_blocking(move || {
            let count = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(count, Ordering::SeqCst);
            started.send(()).unwrap();
            let (released, wake) = &*gate;
            let guard = wake
                .wait_while(released.lock().unwrap(), |released| !*released)
                .unwrap();
            drop(guard);
            active.fetch_sub(1, Ordering::SeqCst);
            index
        }));
    }
    drop(started_tx);

    for _ in 0..expected_started {
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    if trueos {
        assert!(matches!(
            started_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
    }
    assert_eq!(active.load(Ordering::SeqCst), expected_started);

    // A full blocking pool must not prevent the async scheduler from polling.
    let (async_tx, async_rx) = mpsc::channel();
    let async_handle = runtime.spawn(async move { async_tx.send(()).unwrap() });
    async_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    drop(gate);
    runtime.block_on(async {
        async_handle.await.unwrap();
        for (expected, handle) in handles.into_iter().enumerate() {
            assert_eq!(handle.await.unwrap(), expected);
        }
    });
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(peak.load(Ordering::SeqCst), expected_started);
}

#[test]
fn trueos_queues_blocking_fanout_without_starving_async_work() {
    exercise_blocking_pool(true, TRUEOS_MAX_BLOCKING_THREADS);
}

#[test]
fn other_platforms_retain_tokios_blocking_pool_capacity() {
    exercise_blocking_pool(false, TRUEOS_MAX_BLOCKING_THREADS * 3);
}
