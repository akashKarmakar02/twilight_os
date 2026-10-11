//! Internet socket blocking policy. Each attempt holds the descriptor lock only
//! for a nonblocking operation. Publish/recheck/await closes the lost-wakeup race;
//! an Arc keeps the descriptor alive without retaining a Process reference.

use crate::sys::net::service::SOCKET_WAIT;
use crate::sys::net::socket::SocketFile;
use crate::sys::proc::{OpenFile, OpenFileKind};
use crate::utils::sync::Mutex;
use alloc::sync::Arc;
use twilight_common::syscall::types::{EAGAIN, EINTR, ENOTSOCK, ETIMEDOUT};

/// Retry socket work after event wakes, without any spinlock held while asleep.
pub fn retry<T>(
    file: &Arc<Mutex<OpenFile>>,
    nonblock: bool,
    deadline: Option<u64>,
    mut attempt: impl FnMut(&mut SocketFile) -> Result<T, i32>,
) -> Result<T, i32> {
    loop {
        let pid = SOCKET_WAIT.prepare_current();
        let result = {
            let mut guard = file.lock();
            match &mut guard.kind {
                OpenFileKind::Socket(socket) => attempt(socket),
                _ => Err(ENOTSOCK),
            }
        };
        if !matches!(result, Err(EAGAIN)) || nonblock {
            SOCKET_WAIT.finish_wait(pid);
            return result;
        }
        if crate::sys::proc::current_has_sleep_interrupting_signal() {
            SOCKET_WAIT.finish_wait(pid);
            return Err(EINTR);
        }
        if deadline.is_some_and(|expires| crate::driver::time::monotonic_ns() >= expires) {
            SOCKET_WAIT.finish_wait(pid);
            return Err(ETIMEDOUT);
        }
        crate::sys::proc::await_io_until(deadline);
        SOCKET_WAIT.finish_wait(pid);
    }
}

/// Read Internet socket data with blocking/nonblocking descriptor semantics.
pub fn read(file: &Arc<Mutex<OpenFile>>, buf: &mut [u8], nonblock: bool) -> i64 {
    match retry(file, nonblock, None, |socket| match socket {
        SocketFile::Tcp(socket) => socket.try_read(buf),
        SocketFile::Udp(socket) => socket.try_recv_from(buf).map(|(count, _)| count),
        _ => Err(ENOTSOCK),
    }) {
        Ok(count) => count as i64,
        Err(errno) => -(errno as i64),
    }
}

/// Write Internet socket data and preserve partial counts and SIGPIPE semantics.
pub fn write(file: &Arc<Mutex<OpenFile>>, buf: &[u8], nonblock: bool) -> i64 {
    match retry(file, nonblock, None, |socket| match socket {
        SocketFile::Tcp(socket) => socket.try_write(buf),
        SocketFile::Udp(socket) => socket.try_write(buf),
        _ => Err(ENOTSOCK),
    }) {
        Ok(count) => count as i64,
        Err(errno) => {
            if errno == twilight_common::syscall::types::EPIPE {
                crate::sys::proc::queue_signal(crate::sys::proc::id(), crate::sys::proc::SIGPIPE);
            }
            -(errno as i64)
        }
    }
}
