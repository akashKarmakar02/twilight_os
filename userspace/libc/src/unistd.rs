use crate::{syscall6, SizeT};
use core::ffi::{c_char, c_int, c_void};
use twilight_common::syscall::numbers::SYS_OPENAT;
use twilight_common::syscall::types::O_CREAT;

/// SsizeT write(int fd, const void *buf, SizeT len);
#[unsafe(no_mangle)]
pub extern "C" fn write(fd: c_int, buf: *const c_void, len: SizeT) -> SizeT {
    syscall6(1, fd as usize, buf as usize, len, 0, 0, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn read(fd: c_int, buf: *mut c_void, len: usize) -> SizeT {
    // Linux x86_64 SYS_read = 0  (change if your ABI differs)
    syscall6(0, fd as usize, buf as usize, len, 0, 0, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn open(pathname: *const c_void, flags: c_int, mode: c_int) -> c_int {
    syscall6(2, pathname as usize, flags as usize, mode as usize, 0, 0, 0) as c_int // SYS_open = 2
}

/// int close(int fd);
#[unsafe(no_mangle)]
pub extern "C" fn close(fd: c_int) -> c_int {
    syscall6(3, fd as usize, 0, 0, 0, 0, 0) as c_int // SYS_close = 3
}

/// void _exit(int status);
#[unsafe(no_mangle)]
pub extern "C" fn _exit(status: c_int) -> ! {
    unsafe {
        let _ = syscall6(60, status as usize, 0, 0, 0, 0, 0); // SYS_exit = 60
        core::hint::unreachable_unchecked()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn openat(dirfd: c_int, pathname: *const c_char, flags: c_int, mode: c_int) -> c_int {
    // sign-extend the 32-bit dirfd to 64-bit
    let dirfd_se = (dirfd as i64) as usize;

    // default mode only if creating; otherwise kernel ignores it anyway
    let creating = (flags & O_CREAT) != 0; // || (flags & O_TMPFILE) == O_TMPFILE
    let sys_mode = if creating && mode == 0 { 0o666 } else { mode } as usize;

    let r = unsafe {
        syscall6(
            SYS_OPENAT,
            dirfd_se,
            pathname as usize,
            flags as usize,
            sys_mode,
            0,
            0,
        )
    };

    if r < 0 {
        // if you have errno, set it here to -r as i32
        // errno::set((-r) as i32);
        -1
    } else {
        r as c_int
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn exit(status: c_int) -> ! { _exit(status) }
