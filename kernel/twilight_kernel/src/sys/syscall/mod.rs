pub mod crypto;
pub mod fs_attr;
pub(crate) mod memory;
pub mod service;
mod utils;
use crate::arch::x86_64::idt::Registers;
use crate::driver::timer::cmos::CMOS;
use crate::serial_println;
use crate::sys::syscall::service::read;
use crate::sys::syscall::utils::{UserPtr, copy_cstr_from_user};
use alloc::string::String;
use twilight_common::syscall::numbers::*;
use twilight_common::syscall::types::{EFAULT, EINVAL, ENOSYS, Rlimit64, Timespec};
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

    let mut restored_from_signal = false;
    let res = match syscall_number {
        SYS_READ => {
            let ptr = arg2 as *mut u8;
            let len = arg3;
            let buf = unsafe { core::slice::from_raw_parts_mut(ptr, len as usize) };
            read(arg1 as usize, buf)
        }
        SYS_WRITE => service::write(arg1 as i32, arg2 as usize, arg3 as usize),
        SYS_PREAD64 => service::pread64(arg1 as i32, arg2 as usize, arg3 as usize, arg4 as u64),
        SYS_RT_SIGACTION => {
            service::rt_sigaction(arg1 as i32, arg2 as usize, arg3 as usize, arg4 as usize)
        }
        SYS_RT_SIGPROCMASK => {
            service::rt_sigprocmask(arg1 as i32, arg2 as usize, arg3 as usize, arg4 as usize)
        }
        SYS_RT_SIGRETURN => {
            restored_from_signal = service::rt_sigreturn(_stack_frame, regs);
            if restored_from_signal {
                0
            } else {
                -(EFAULT as i64)
            }
        }
        SYS_RT_SIGSUSPEND => service::rt_sigsuspend(arg1 as usize, arg2 as usize),
        SYS_SIGALTSTACK => service::sigaltstack(arg1 as usize, arg2 as usize),
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
        SYS_LSTAT => service::lstat(arg1 as usize, arg2 as usize),
        SYS_POLL => service::poll(arg1 as usize, arg2 as usize, arg3 as isize),
        SYS_SELECT => service::select(arg1 as i32, arg2 as usize, arg3 as usize, arg4 as usize, arg5 as usize),
        SYS_LSEEK => service::lseek(arg1 as usize, arg2, arg3 as u8),
        SYS_MMAP => memory::mmap(
            arg1,
            arg2 as usize,
            arg3 as usize,
            arg4 as usize,
            arg5,
            arg6,
        ),
        SYS_MSYNC => memory::msync(arg1, arg2 as usize, arg3 as i32),
        SYS_MPROTECT => memory::mprotect(arg1, arg2 as usize, arg3 as usize),
        SYS_MUNMAP => memory::munmap(arg1, arg2 as usize),
        SYS_BRK => memory::brk(arg1 as usize),
        SYS_DUP => service::dup(arg1 as i32),
        SYS_DUP2 => service::dup2(arg1 as i32, arg2 as i32),
        SYS_DUP3 => service::dup3(arg1 as i32, arg2 as i32, arg3 as i32),
        SYS_IOCTL => {
            service::ioctl(arg1 as usize, arg2 as usize, arg3 as usize)
            // 0
        }
        SYS_FCNTL => service::fcntl(arg1 as i32, arg2 as i32, arg3),
        SYS_FTRUNCATE => service::ftruncate(arg1 as i32, arg2 as i64),
        SYS_READV => service::readv(arg1 as usize, arg2, arg3),
        SYS_WRITEV => service::writev(arg1 as i32, arg2, arg3 as i32),
        SYS_PREADV => service::preadv(arg1 as i32, arg2 as usize, arg3 as usize, arg4 as u64),
        SYS_PWRITEV => service::pwritev(arg1 as i32, arg2 as usize, arg3 as usize, arg4 as u64),
        SYS_ACCESS => service::access(arg1 as usize, arg2 as i32),
        SYS_PIPE => service::pipe(arg1 as usize),
        SYS_PAUSE => service::pause(),
        SYS_SCHED_YIELD => service::sched_yield(),
        SYS_MADVISE => memory::madvise(arg1, arg2 as usize, arg3 as i32),
        SYS_GETPID => service::getpid(),
        SYS_SETPGID => service::setpgid(arg1 as i32, arg2 as i32),
        SYS_GETPPID => service::getppid(),
        SYS_GETPGRP => service::getpgrp(),
        SYS_SETSID => service::setsid(),
        SYS_GETPGID => service::getpgid(arg1 as i32),
        SYS_GETSID => service::getsid(arg1 as i32),
        SYS_STATFS => service::statfs(arg1 as usize, arg2 as usize),
        SYS_FSTATFS => service::fstatfs(arg1 as i32, arg2 as usize),
        SYS_SOCKET => service::socket(arg1 as i32, arg2 as i32, arg3 as i32),
        SYS_CONNECT => service::connect(arg1 as i32, arg2 as usize, arg3 as usize),
        SYS_BIND => service::bind(arg1 as i32, arg2 as usize, arg3 as usize),
        SYS_LISTEN => service::listen(arg1 as i32, arg2 as i32),
        SYS_ACCEPT => service::accept(arg1 as i32, arg2 as usize, arg3 as usize),
        SYS_ACCEPT4 => service::accept4(arg1 as i32, arg2 as usize, arg3 as usize, arg4 as i32),
        SYS_SENDTO => service::sendto(
            arg1 as i32,
            arg2 as usize,
            arg3 as usize,
            arg4 as i32,
            arg5 as usize,
            arg6 as usize,
        ),
        SYS_RECVFROM => service::recvfrom(
            arg1 as i32,
            arg2 as usize,
            arg3 as usize,
            arg4 as i32,
            arg5 as usize,
            arg6 as usize,
        ),
        SYS_SENDMSG => service::sendmsg(arg1 as i32, arg2 as usize, arg3 as i32),
        SYS_RECVMSG => service::recvmsg(arg1 as i32, arg2 as usize, arg3 as i32),
        SYS_SOCKETPAIR => {
            service::socketpair(arg1 as i32, arg2 as i32, arg3 as i32, arg4 as usize)
        }
        SYS_SHUTDOWN => service::shutdown(arg1 as i32, arg2 as i32),
        SYS_SETSOCKOPT => service::setsockopt(
            arg1 as i32,
            arg2 as i32,
            arg3 as i32,
            arg4 as usize,
            arg5 as usize,
        ),
        SYS_GETSOCKOPT => service::getsockopt(
            arg1 as i32,
            arg2 as i32,
            arg3 as i32,
            arg4 as usize,
            arg5 as usize,
        ),
        SYS_GETSOCKNAME => service::getsockname(arg1 as i32, arg2 as usize, arg3 as usize),
        SYS_GETPEERNAME => service::getpeername(arg1 as i32, arg2 as usize, arg3 as usize),
        SYS_CLONE => service::clone(
            arg1,
            arg2,
            arg3 as usize,
            arg4 as usize,
            arg5,
            _stack_frame,
            regs,
        ),
        SYS_FORK => service::fork(_stack_frame, regs),
        SYS_EXECVE => service::execve(
            arg1 as usize,
            arg2 as usize,
            arg3 as usize,
            _stack_frame,
            regs,
        ),
        SYS_EXIT => service::exit(arg1 as i32),
        SYS_KILL => service::kill(arg1 as i32, arg2 as i32),
        SYS_UNAME => service::uname(arg1 as usize),
        SYS_GETCWD => service::getcwd(arg1 as usize, arg2 as usize),
        SYS_CHDIR => service::chdir(arg1 as usize),
        SYS_FCHDIR => service::fchdir(arg1 as i32),
        SYS_RENAME => service::rename(arg1 as usize, arg2 as usize),
        SYS_MKDIR => service::mkdir(arg1 as usize, arg2 as usize),
        SYS_RMDIR => service::rmdir(arg1 as usize),
        SYS_UNLINK => service::unlink(arg1 as usize),
        SYS_READLINK => service::readlink(arg1 as usize, arg2 as usize, arg3 as usize),
        SYS_CHMOD => service::chmod(arg1 as usize, arg2 as u32),
        SYS_UMASK => service::umask(arg1 as u32),
        SYS_GETRUSAGE => service::getrusage(arg1 as i32, arg2 as usize),
        SYS_SYSINFO => service::sysinfo(arg1 as usize),
        SYS_GETUID => service::geteuid(),
        SYS_GETGID => service::getegid(),
        SYS_SET_UID => service::setuid(arg1),
        SYS_SET_GID => service::setgid(arg1),
        SYS_GET_EUID => service::geteuid(),
        SYS_GET_EGID => service::getegid(),
        SYS_PRCTL => service::prctl(
            arg1 as i32,
            arg2 as usize,
            arg3 as usize,
            arg4 as usize,
            arg5 as usize,
        ),
        SYS_ARCH_PRCTL => service::arch_prctl(arg1, arg2),
        SYS_LISTXATTR => service::listxattr(arg1 as usize, arg2 as usize, arg3 as usize),
        SYS_LLISTXATTR => service::llistxattr(arg1 as usize, arg2 as usize, arg3 as usize),
        SYS_FLISTXATTR => service::flistxattr(arg1 as i32, arg2 as usize, arg3 as usize),
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
            let rem_timespec_ptr = arg2 as *mut Timespec;

            if req_timespec_ptr.is_null() {
                -(EFAULT as i64)
            } else {
                let req = unsafe { &*req_timespec_ptr };

                // We don't implement interruption yet; if rem != NULL, return 0 remaining.
                if !rem_timespec_ptr.is_null() {
                    unsafe {
                        *rem_timespec_ptr = Timespec {
                            tv_sec: 0,
                            tv_nsec: 0,
                        }
                    };
                }

                match crate::driver::timer::pit::sleep_timespec(req) {
                    Ok(()) => 0i64,
                    Err(e) => e, // negative errno
                }
            }
        }
        SYS_SETITIMER => service::setitimer(arg1 as i32, arg2 as usize, arg3 as usize),
        SYS_CAPGET => service::capget(arg1 as usize, arg2 as usize),
        SYS_GETDENTS64 => {
            let fd = arg1 as i32;
            let buf = arg2 as *mut u8;
            let buf_len = arg3;

            service::getdent64(fd, buf, buf_len as usize)
        }
        // Linux returns the thread id (tid) and records the location for clear_tid on exit.
        // We don't implement clear_tid yet, but returning a real tid is critical for glibc.
        SYS_SETTID_ADDR => crate::sys::proc::id() as i64,
        SYS_CLOCK_GETTIME => {
            let timespec_ptr = arg2 as *mut Timespec;
            crate::driver::timer::pit::sys_clock_gettime(arg1 as i32, timespec_ptr)
        }
        SYS_CLOCK_NANOSLEEP => {
            const TIMER_ABSTIME: i32 = 1;
            const NSEC_PER_SEC: i64 = 1_000_000_000;

            let clockid = arg1 as i32;
            let flags = arg2 as i32;
            let req_ptr = arg3 as *const Timespec;
            let rem_ptr = arg4 as *mut Timespec;

            if req_ptr.is_null() {
                -(EFAULT as i64)
            } else if flags & !TIMER_ABSTIME != 0 {
                -(EINVAL as i64)
            } else {
                // SAFETY: req_ptr was checked non-null above. read_unaligned
                // avoids creating a reference to the packed userspace timespec.
                let req = unsafe { core::ptr::read_unaligned(req_ptr) };
                if req.tv_sec < 0 || req.tv_nsec < 0 || req.tv_nsec >= NSEC_PER_SEC {
                    -(EINVAL as i64)
                } else {
                    if !rem_ptr.is_null() {
                        // SAFETY: rem_ptr is caller-provided writable memory by
                        // Linux ABI contract when non-null. Twilight does not
                        // interrupt sleeps yet, so remaining time is zero.
                        unsafe {
                            core::ptr::write_unaligned(
                                rem_ptr,
                                Timespec {
                                    tv_sec: 0,
                                    tv_nsec: 0,
                                },
                            );
                        }
                    }

                    if flags & TIMER_ABSTIME == 0 {
                        match crate::driver::timer::pit::sleep_timespec(&req) {
                            Ok(()) => 0,
                            Err(errno) => errno,
                        }
                    } else {
                        let mut now = Timespec::default();
                        let now_res = crate::driver::timer::pit::sys_clock_gettime(
                            clockid,
                            &mut now as *mut Timespec,
                        );
                        if now_res < 0 {
                            now_res
                        } else {
                            let req_ns = (req.tv_sec as i128)
                                .saturating_mul(NSEC_PER_SEC as i128)
                                .saturating_add(req.tv_nsec as i128);
                            let now_ns = (now.tv_sec as i128)
                                .saturating_mul(NSEC_PER_SEC as i128)
                                .saturating_add(now.tv_nsec as i128);
                            if req_ns <= now_ns {
                                0
                            } else {
                                crate::driver::timer::pit::sleep_ns(
                                    core::cmp::min(
                                        (req_ns - now_ns) as u128,
                                        u64::MAX as u128,
                                    ) as u64,
                                );
                                0
                            }
                        }
                    }
                }
            }
        }
        SYS_EXIT_GROUP => service::exit_group(arg1 as i32),
        SYS_WAIT4 => service::wait4(arg1 as i32, arg2 as usize, arg3 as i32, arg4 as usize),
        SYS_FUTEX => service::futex(
            arg1 as usize,
            arg2 as i32,
            arg3 as u32,
            arg4 as usize,
            arg5 as usize,
            arg6 as u32,
        ),
        SYS_SCHED_GETAFFINITY => {
            service::sched_getaffinity(arg1 as i32, arg2 as usize, arg3 as usize)
        }
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
        SYS_MKDIRAT => service::mkdirat(arg1 as i32, arg2 as usize, arg3 as usize),
        SYS_NEWFSTATAT => {
            service::newfstatat(arg1 as i32, arg2 as usize, arg3 as usize, arg4 as i32)
        }
        SYS_READLINKAT => {
            service::readlinkat(arg1 as i32, arg2 as usize, arg3 as usize, arg4 as usize)
        }
        SYS_TGKILL => service::tgkill(arg1 as i32, arg2 as i32, arg3 as i32),
        SYS_UTIMENAT => service::utimenat(arg1 as i32, arg2 as usize, arg3 as usize, arg4 as usize),
        SYS_PIPE2 => service::pipe2(arg1 as usize, arg2 as i32),
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
        SYS_SET_ROBUST_LIST => service::set_robust_list(arg1 as usize, arg2 as usize),
        SYS_GETRANDOM => service::getrandom(arg1 as usize, arg2 as usize, arg3 as u32),
        SYS_MEMFD_CREATE => service::memfd_create(arg1 as usize, arg2 as u32),
        SYS_RSEQ => service::rseq(arg1 as usize, arg2 as u32, arg3 as u32, arg4 as u32),
        SYS_REBOOT => {
            // Linux reboot magic numbers
            let magic1 = arg1 as u32;
            let magic2 = arg2 as u32;
            let cmd = arg3 as u32;

            if magic1 == 0xfee1dead && magic2 == 672274793 {
                match cmd {
                    0x01234567 => {
                        // LINUX_REBOOT_CMD_RESTART
                        crate::arch::x86_64::power::restart();
                        0
                    }
                    0x4321fedc => {
                        // LINUX_REBOOT_CMD_POWER_OFF
                        crate::arch::x86_64::power::poweroff();
                        0
                    }
                    _ => -(EINVAL as i64),
                }
            } else {
                -(EINVAL as i64)
            }
        }
        // custom syscall currently used for installing (TODO: do this in normal way by writing to /dev/disk0)
        700 => {
            crate::kernel_utils::install::main();
            0
        }
        // SYS_PPOLL
        271 => service::ppoll(
            arg1 as usize,
            arg2 as usize,
            arg3 as usize,
            arg4 as usize,
            arg5 as usize,
        ),
        // SYS_ADD_USER_KEY
        448 => crypto::sys_add_user_key(arg1 as u32, arg2 as *const u8, arg3 as usize) as i64,
        // SYS_SET_FILE_ATTR
        449 => fs_attr::sys_set_file_attr(arg1 as *const u8, arg2 as u32, arg3 as u32) as i64,
        // clone3 – musl probes this; returning ENOSYS makes it fall back to clone.
        SYS_CLONE3 => -(ENOSYS as i64),
        _ => {
            serial_println!("Unknown syscall number: {}", syscall_number);
            -(ENOSYS as i64)
            // 0
        }
    };

    if syscall_number != 271 {
        // serial_println!(
        //     "[syscall] pid={} nr={} res={} rip={:#x} rsp={:#x}",
        //     crate::sys::proc::id(),
        //     syscall_number,
        //     res,
        //     _stack_frame.instruction_pointer.as_u64(),
        //     _stack_frame.stack_pointer.as_u64(),
        // );
    }

    if !restored_from_signal {
        regs.rax = res as u64;
        service::deliver_pending_signal(_stack_frame, regs);
    }
    // Syscall dispatch and signal delivery have completed and no syscall-local
    // lock is intentionally held here, making this a safe deferred-reschedule
    // point before returning to userspace.
    crate::sys::preempt::cond_resched();

    if syscall_number == SYS_FORK || syscall_number == SYS_WAIT4 || syscall_number == SYS_EXECVE {
        // serial_println!(
        //     "[syscall] pid={} nr={} return rax={:#x}",
        //     crate::sys::proc::id(),
        //     syscall_number,
        //     regs.rax,
        // );
    }
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
