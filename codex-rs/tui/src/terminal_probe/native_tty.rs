//! Exclusive terminal probes use the attached native console, not Unix files.
use std::io;
use std::time::{Duration, Instant};

unsafe extern "C" {
    fn trueos_cabi_blueprint_terminal_lease_current_v1(expected: u64, epoch: *mut u64) -> i32;
    fn trueos_cabi_shell_attached_read_v1(out: *mut u8, capacity: usize) -> isize;
    fn trueos_cabi_shell_attached_write(bytes: *const u8, len: usize) -> usize;
    fn trueos_cabi_shell_attached_observe_v1() -> u32;
    fn trueos_cabi_shell_attached_wait_after_v1(observed: u32, timeout_ms: u64) -> i32;
}

pub(super) struct Tty {
    lease: u64,
    pending: Vec<u8>,
}

fn current_lease(expected: u64) -> io::Result<u64> {
    let mut epoch = 0;
    let code = unsafe { trueos_cabi_blueprint_terminal_lease_current_v1(expected, &mut epoch) };
    if code == 0 && epoch != 0 && (expected == 0 || expected == epoch) {
        Ok(epoch)
    } else {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            format!("native terminal probe lease is unavailable or changed ({code})"),
        ))
    }
}

impl Tty {
    pub(super) fn open() -> io::Result<Self> {
        Ok(Self {
            lease: current_lease(0)?,
            pending: Vec::new(),
        })
    }

    pub(super) fn write_all(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            current_lease(self.lease)?;
            let count = unsafe { trueos_cabi_shell_attached_write(bytes.as_ptr(), bytes.len()) };
            if count == 0 || count > bytes.len() {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "native terminal probe could not write its query",
                ));
            }
            bytes = &bytes[count..];
        }
        Ok(())
    }

    fn read(&self, bytes: &mut [u8]) -> io::Result<usize> {
        current_lease(self.lease)?;
        let count = unsafe { trueos_cabi_shell_attached_read_v1(bytes.as_mut_ptr(), bytes.len()) };
        if count < 0 || count as usize > bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("native terminal probe read failed ({count})"),
            ));
        }
        current_lease(self.lease)?;
        Ok(count as usize)
    }

    pub(super) fn read_available(
        &mut self,
        buffer: &mut Vec<u8>,
        deadline: Instant,
        max_bytes: usize,
    ) -> io::Result<()> {
        current_lease(self.lease)?;
        let count = self.pending.len().min(max_bytes.saturating_sub(buffer.len()));
        buffer.extend(self.pending.drain(..count));
        let mut chunk = [0; 256];
        while buffer.len() < max_bytes && Instant::now() < deadline {
            let capacity = chunk.len().min(max_bytes - buffer.len());
            let count = self.read(&mut chunk[..capacity])?;
            if count == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..count]);
        }
        Ok(())
    }

    pub(super) fn poll_readable(&mut self, timeout: Duration) -> io::Result<bool> {
        if !self.pending.is_empty() {
            current_lease(self.lease)?;
            return Ok(true);
        }
        let deadline = Instant::now() + timeout;
        loop {
            // Capture before the readiness probe so arrivals cannot be lost
            // between reading the queue and registering the native wait.
            let observed = unsafe { trueos_cabi_shell_attached_observe_v1() };
            let mut chunk = [0; 1];
            let count = self.read(&mut chunk)?;
            if count != 0 {
                self.pending.extend_from_slice(&chunk[..count]);
                return Ok(true);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            let millis = remaining.as_nanos().div_ceil(1_000_000).min(u128::from(u64::MAX)) as u64;
            let code = unsafe { trueos_cabi_shell_attached_wait_after_v1(observed, millis) };
            if code < 0 {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("native terminal probe wait failed ({code})"),
                ));
            }
        }
    }

    pub(super) fn replay_input(&self, input: &[u8]) -> io::Result<()> {
        crossterm::event::buffer_input_for_lease(input, self.lease)
    }
}
