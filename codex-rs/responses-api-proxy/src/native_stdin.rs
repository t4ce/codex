//! Credential input from an actual kernel-owned parent/child byte stream.
use std::io;
use std::time::{Duration, Instant};

unsafe extern "C" {
    fn trueos_cabi_stdio_is_terminal_v1(stream: u32) -> i32;
    fn trueos_cabi_blueprint_child_read_v1(handle: u64, out: *mut u8, capacity: usize) -> isize;
    fn trueos_cabi_blueprint_child_status_v1(handle: u64) -> i32;
}

pub(super) fn read(buffer: &mut [u8]) -> io::Result<usize> {
    read_with_timeout(buffer, Duration::from_secs(30))
}

fn read_with_timeout(buffer: &mut [u8], timeout: Duration) -> io::Result<usize> {
    if std::env::var_os("TRUEOS_MCP_STDIO").as_deref() != Some(std::ffi::OsStr::new("1"))
        || unsafe { trueos_cabi_stdio_is_terminal_v1(0) } != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "credential input requires a kernel-owned parent stdin pipe",
        ));
    }
    // Handle zero is a native capability for this child's captured parent;
    // it is not a Unix descriptor. A closed parent may retain queued bytes.
    if !matches!(unsafe { trueos_cabi_blueprint_child_status_v1(0) }, 1..=4) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "credential input has no native parent stdin endpoint",
        ));
    }
    if buffer.is_empty() {
        return Ok(0);
    }
    let started = Instant::now();
    loop {
        let count = unsafe {
            trueos_cabi_blueprint_child_read_v1(0, buffer.as_mut_ptr(), buffer.len())
        };
        if count < 0 {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "native credential input failed"));
        }
        if count as usize > buffer.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "native credential input returned an invalid length"));
        }
        if count != 0 {
            return Ok(count as usize);
        }
        match unsafe { trueos_cabi_blueprint_child_status_v1(0) } {
            1..=3 => {}
            4 => return Ok(0),
            _ => return Err(io::Error::new(io::ErrorKind::BrokenPipe, "native credential parent endpoint disappeared")),
        }
        let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "native credential input timed out"));
        };
        std::thread::sleep(remaining.min(Duration::from_millis(10)));
    }
}
