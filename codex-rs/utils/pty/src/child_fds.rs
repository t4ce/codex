/// Extra descriptors accompanying a process launch.
#[derive(Clone, Copy)]
pub enum ChildFds<'a> {
    /// Existing escalation sockets, with the legacy PTY I/O and status behavior.
    Inherited(&'a [i32]),
    /// Launch attachments, including CLOEXEC files, with ordinary PTY behavior.
    Attached(&'a [i32]),
}

impl<'a> ChildFds<'a> {
    pub fn as_slice(self) -> &'a [i32] {
        match self {
            Self::Inherited(fds) | Self::Attached(fds) => fds,
        }
    }
}
