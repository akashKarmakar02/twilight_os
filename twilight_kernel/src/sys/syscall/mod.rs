mod memory;
pub mod service;
mod utils;

use crate::arch::x86_64::idt::Registers;
use crate::driver::timer::cmos::CMOS;
use crate::serial_println;
use crate::sys::syscall::service::read;
use crate::sys::syscall::utils::{copy_cstr_from_user, UserPtr};
use crate::sys::syscall::SyscallError::ENOSYS;
use crate::task::executor::sleep;
use alloc::string::String;
use twilight_common::syscall::numbers::*;
use twilight_common::syscall::types::{Rlimit64, Timespec};
use x86_64::structures::idt::InterruptStackFrame;

#[allow(dead_code)]
pub extern "sysv64" fn syscall_handler(
    _stack_frame: &mut InterruptStackFrame,
    regs: &mut Registers,
) {
    let syscall_number = regs.rax as usize;
    let arg1 = regs.rdi;
    let arg2 = regs.rsi;
    let arg3 = regs.rdx;
    let arg4 = regs.r10;
    let arg5 = regs.r8;
    let arg6 = regs.r9;

    let res = match syscall_number {
        SYS_READ => {
            let ptr = arg2 as *mut u8;
            let len = arg3;
            let buf = unsafe { core::slice::from_raw_parts_mut(ptr, len as usize) };
            read(arg1 as usize, buf)
        }
        SYS_WRITE => service::write(arg1 as i32, arg2 as usize, arg3 as usize),
        SYS_OPEN => {
            let upath = UserPtr(arg1 as *const u8);

            let path = match copy_cstr_from_user(upath, 4096) {
                Ok(s) => s,
                _ => String::new(),
            };
            let flags = arg2 as i32;
            let mode = arg3 as i32;
            service::open(&path, flags, mode as u32)
        }
        SYS_CLOSE => service::close(arg1 as i32),
        SYS_STAT => service::stat(arg1 as usize, arg2 as usize),
        SYS_FSTAT => service::fstat(arg1 as usize, arg2 as usize),
        SYS_POLL => service::poll(arg1 as usize, arg2 as usize, arg3 as isize),
        SYS_LSEEK => service::lseek(arg1 as usize, arg2, arg3 as u8),
        SYS_MMAP => memory::mmap(
            arg1,
            arg2 as usize,
            arg3 as usize,
            arg4 as usize,
            arg5,
            arg6,
        ),
        SYS_MPROTECT => memory::mprotect(arg1, arg2 as usize, arg3 as usize),
        SYS_MUNMAP => memory::munmap(arg1, arg2 as usize),
        SYS_BRK => memory::brk(arg1 as usize),
        SYS_IOCTL => {
            service::ioctl(arg1 as usize, arg2 as usize, arg3 as usize)
            // 0
        }
        SYS_FCNTL => service::fcntl(arg1 as i32, arg2 as i32, arg3),
        SYS_READV => service::readv(arg1 as usize, arg2, arg3),
        SYS_WRITEV => service::writev(arg1 as i32, arg2, arg3 as i32),
        SYS_EXECVE => service::execev(arg1 as usize, arg2 as usize, arg3 as usize),
        SYS_EXIT => service::exit(),
        SYS_UNAME => service::uname(arg1 as usize),
        SYS_GETCWD => service::getcwd(arg1 as usize, arg2 as usize),
        SYS_CHDIR => service::chdir(arg1 as usize),
        SYS_MKDIR => service::mkdir(arg1 as usize, arg2 as usize),
        SYS_RMDIR => service::rmdir(arg1 as usize),
        SYS_UNLINK => service::unlink(arg1 as usize),
        SYS_SET_UID => service::setuid(arg1),
        SYS_GET_EUID => 0,
        SYS_ARCH_PRCTL => service::arch_prctl(arg1, arg2),
        SYS_GET_TID => crate::sys::proc::id() as i64,
        SYS_TIME => {
            let out_ptr = arg1 as *mut i64; // time_t is i64
            let mut cmos = CMOS::new();
            let unix_time: u64 = cmos.unix_time();

            if !out_ptr.is_null() {
                unsafe { *out_ptr = unix_time as i64 };
            }
            unix_time as i64
        }
        SYS_NANOSLEEP => {
            let req_timespec_ptr = arg1 as *const Timespec;
            let _rem_timespec_ptr = arg2 as *mut Timespec;

            unsafe {
                if !req_timespec_ptr.is_null() {
                    let req = &*req_timespec_ptr;
                    sleep((req.tv_nsec as f64 / 10000000000.0) + req.tv_sec as f64);
                }
            }

            0i64
        }
        SYS_GETDENTS64 => {
            let fd = arg1 as i32;
            let buf = arg2 as *mut u8;
            let buf_len = arg3;

            service::getdent64(fd, buf, buf_len as usize)
        }
        SYS_SETTID_ADDR => arg1 as i64,
        SYS_CLOCK_GETTIME => {
            let timespec_ptr = arg2 as *mut Timespec;
            crate::driver::timer::pit::sys_clock_gettime(arg1 as i32, timespec_ptr)
        }
        SYS_EXIT_GROUP => service::exit(),
        SYS_OPENAT => {
            let upath = UserPtr(arg2 as *const u8);

            let path = match copy_cstr_from_user(upath, 4096) {
                Ok(s) => s,
                _ => String::new(),
            };
            let flags = arg3 as i32;
            let mode = arg4 as i32;
            service::openat(arg1 as i32, path.as_str(), flags, mode as u32)
        }
        SYS_UTIMENAT => service::utimenat(arg1 as i32, arg2 as usize, arg3 as usize, arg4 as usize),
        SYS_PR_LIMIT64 => {
            let pid = arg1;
            let resource = arg2 as u32;

            let new_limit_ptr = arg3 as *const Rlimit64;
            let old_limit_ptr = arg4 as *mut Rlimit64;

            let new_limit = if new_limit_ptr.is_null() {
                None
            } else {
                Some(unsafe { &*new_limit_ptr })
            };

            let old_limit = if old_limit_ptr.is_null() {
                None
            } else {
                Some(unsafe { &mut *old_limit_ptr })
            };

            service::pr_limit64(pid as i32, resource, new_limit, old_limit)
        }
        _ => {
            serial_println!("Unknown syscall number: {}", syscall_number);
            -(ENOSYS as i64)
        }
    };

    regs.rax = res as u64;
}

#[derive(Copy, Clone, PartialEq, Debug)]
#[repr(isize)]
#[allow(clippy::enum_clike_unportable_variant)]
pub enum SyscallError {
    EDOM = 1,
    EILSEQ = 2,
    ERANGE = 3,

    E2BIG = 1001,
    EACCES = 1002,
    EADDRINUSE = 1003,
    EADDRNOTAVAIL = 1004,
    EAFNOSUPPORT = 1005,
    EAGAIN = 1006,
    EALREADY = 1007,
    EBADF = 1008,
    EBADMSG = 1009,
    EBUSY = 1010,
    ECANCELED = 1011,
    ECHILD = 1012,
    ECONNABORTED = 1013,
    ECONNREFUSED = 1014,
    ECONNRESET = 1015,
    EDEADLK = 1016,
    EDESTADDRREQ = 1017,
    EDQUOT = 1018,
    EEXIST = 1019,
    EFAULT = 1020,
    EFBIG = 1021,
    EHOSTUNREACH = 1022,
    EIDRM = 1023,
    EINPROGRESS = 1024,
    EINTR = 1025,
    EINVAL = 1026,
    EIO = 1027,
    EISCONN = 1028,
    EISDIR = 1029,
    ELOOP = 1030,
    EMFILE = 1031,
    EMLINK = 1032,
    EMSGSIZE = 1034,
    EMULTIHOP = 1035,
    ENAMETOOLONG = 1036,
    ENETDOWN = 1037,
    ENETRESET = 1038,
    ENETUNREACH = 1039,
    ENFILE = 1040,
    ENOBUFS = 1041,
    ENODEV = 1042,
    ENOENT = 1043,
    ENOEXEC = 1044,
    ENOLCK = 1045,
    ENOLINK = 1046,
    ENOMEM = 1047,
    ENOMSG = 1048,
    ENOPROTOOPT = 1049,
    ENOSPC = 1050,
    ENOSYS = 1051,
    ENOTCONN = 1052,
    ENOTDIR = 1053,
    ENOTEMPTY = 1054,
    ENOTRECOVERABLE = 1055,
    ENOTSOCK = 1056,
    ENOTSUP = 1057,
    ENOTTY = 1058,
    ENXIO = 1059,
    EOPNOTSUPP = 1060,
    EOVERFLOW = 1061,
    EOWNERDEAD = 1062,
    EPERM = 1063,
    EPIPE = 1064,
    EPROTO = 1065,
    EPROTONOSUPPORT = 1066,
    EPROTOTYPE = 1067,
    EROFS = 1068,
    ESPIPE = 1069,
    ESRCH = 1070,
    ESTALE = 1071,
    ETIMEDOUT = 1072,
    ETXTBSY = 1073,
    EXDEV = 1075,
    ENODATA = 1076,
    ETIME = 1077,
    ENOKEY = 1078,
    ESHUTDOWN = 1079,
    EHOSTDOWN = 1080,
    EBADFD = 1081,
    ENOMEDIUM = 1082,
    ENOTBLK = 1083,

    Unknown = isize::MAX,
}
