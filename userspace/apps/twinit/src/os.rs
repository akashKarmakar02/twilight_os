//! Low-level OS primitives used by twinit.
//!
//! Every function in this module wraps a single libc symbol through
//! `unsafe extern "C"`. The wrappers return `io::Result` so that
//! callers never touch raw FFI directly.

use std::fs::File;
use std::io;
use std::os::fd::FromRawFd;
use std::os::raw::c_int;
use std::time::Duration;

pub const WNOHANG: c_int = 1;
const EINTR: i32 = 4;
const O_NONBLOCK: c_int = 0x800;
const O_CLOEXEC: c_int = 0x80000;
const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;

#[repr(C)]
#[derive(Clone, Copy)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

unsafe extern "C" {
    fn fork() -> c_int;
    fn setsid() -> c_int;
    fn dup2(oldfd: c_int, newfd: c_int) -> c_int;
    fn pipe2(pipefd: *mut c_int, flags: c_int) -> c_int;
    fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    fn nanosleep(request: *const Timespec, remainder: *mut Timespec) -> c_int;
    fn _exit(status: c_int) -> !;
}

/// Create a pipe whose read end is nonblocking and whose inherited helper
/// descriptors close automatically across exec. The duplicated stdout/stderr
/// descriptor in the child remains open because Linux `dup2` clears CLOEXEC.
pub fn create_log_pipe() -> io::Result<(File, File)> {
    let mut descriptors = [-1, -1];
    // SAFETY: `descriptors` is writable storage for exactly two C integers.
    // On success, pipe2 initializes both entries with owned descriptors.
    if unsafe { pipe2(descriptors.as_mut_ptr(), O_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: pipe2 succeeded, so both descriptors are uniquely owned here.
    let reader = unsafe { File::from_raw_fd(descriptors[0]) };
    // SAFETY: same as above; this is the distinct write end of the pipe.
    let writer = unsafe { File::from_raw_fd(descriptors[1]) };

    // SAFETY: reader is a valid open descriptor and F_GETFL has no third
    // argument requirements.
    let flags = unsafe { fcntl(descriptors[0], F_GETFL, 0) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: reader remains open and F_SETFL accepts the existing status
    // flags combined with O_NONBLOCK.
    if unsafe { fcntl(descriptors[0], F_SETFL, flags | O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok((reader, writer))
}

pub fn fork_process() -> io::Result<c_int> {
    // SAFETY: fork has no pointer arguments. twinit is single-threaded and
    // the child immediately performs descriptor setup followed by exec.
    let pid = unsafe { fork() };
    if pid < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(pid)
    }
}

pub fn create_session() -> io::Result<()> {
    // SAFETY: setsid has no arguments and only changes the calling process.
    if unsafe { setsid() } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn duplicate_fd(oldfd: c_int, newfd: c_int) -> io::Result<()> {
    // SAFETY: both descriptors are integer handles. Their validity is
    // checked by the kernel and no Rust references cross this operation.
    if unsafe { dup2(oldfd, newfd) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn reap_one() -> io::Result<Option<(c_int, c_int)>> {
    let mut status = 0;
    // SAFETY: status points to initialized writable storage for the full
    // duration of waitpid, and WNOHANG makes this call nonblocking.
    let pid = unsafe { waitpid(-1, &mut status, WNOHANG) };
    match pid {
        n if n > 0 => Ok(Some((n, status))),
        0 => Ok(None),
        _ => Err(io::Error::last_os_error()),
    }
}

pub fn sleep(duration: Duration) {
    let mut request = Timespec {
        tv_sec: duration.as_secs() as i64,
        tv_nsec: duration.subsec_nanos() as i64,
    };
    loop {
        let mut remainder = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: request and remainder are valid Timespec objects for the
        // duration of the call. nanosleep only reads/writes those objects.
        if unsafe { nanosleep(&request, &mut remainder) } == 0 {
            return;
        }
        if io::Error::last_os_error().raw_os_error() != Some(EINTR) {
            return;
        }
        request = remainder;
    }
}

pub fn exit_child(status: c_int) -> ! {
    // SAFETY: _exit terminates only the calling child process and does not
    // run duplicated parent-side Rust destructors after fork.
    unsafe { _exit(status) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn log_pipe_reader_is_nonblocking() {
        let (mut reader, mut writer) = create_log_pipe().unwrap();
        let mut byte = [0_u8; 1];
        assert_eq!(
            reader.read(&mut byte).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );

        writer.write_all(b"x").unwrap();
        assert_eq!(reader.read(&mut byte).unwrap(), 1);
        assert_eq!(byte[0], b'x');
    }
}
