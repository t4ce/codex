//! Local Unix sockets require an explicit native IPC capability. Filesystem
//! key protection cannot stand in for socket creation or peer authentication.
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::io::ReadBuf;

fn unavailable() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "TRUEOS does not provide Unix socket rendezvous or descriptor transport",
    )
}

pub fn shared_daemon_socket_directory() -> io::Result<PathBuf> {
    Err(unavailable())
}
pub fn prepare_shared_daemon_socket_directory() -> io::Result<PathBuf> {
    Err(unavailable())
}
pub(super) async fn prepare_private_socket_directory(_path: &Path) -> io::Result<()> {
    Err(unavailable())
}
pub(super) async fn is_stale_socket_path(_path: &Path) -> io::Result<bool> {
    Err(unavailable())
}

pub(super) struct Listener;
pub(super) struct Stream;

pub(super) async fn bind_listener(_path: &Path) -> io::Result<Listener> {
    Err(unavailable())
}
pub(super) async fn connect_stream(_path: &Path) -> io::Result<Stream> {
    Err(unavailable())
}
impl Listener {
    pub(super) async fn accept(&mut self) -> io::Result<Stream> {
        Err(unavailable())
    }
}
impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Err(unavailable()))
    }
}
impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(unavailable()))
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Err(unavailable()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Err(unavailable()))
    }
}
