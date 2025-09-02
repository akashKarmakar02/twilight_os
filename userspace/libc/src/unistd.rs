use crate::{syscall6, SizeT};
use core::ffi::{c_int, c_void};

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

/// void _exit(int status);
#[unsafe(no_mangle)]
pub extern "C" fn _exit(status: c_int) -> ! {
    unsafe {
        let _ = syscall6(60, status as usize, 0, 0, 0, 0, 0); // SYS_exit = 60
        core::hint::unreachable_unchecked()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn exit(status: c_int) -> ! { _exit(status) }
