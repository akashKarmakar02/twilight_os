mod service;

use crate::arch::x86_64::idt::{Registers, PICS};
use crate::driver::timer::cmos::CMOS;
use crate::sys::syscall::service::read;
use crate::task::executor::sleep;
use crate::serial_prtinln;
use alloc::string::String;
use alloc::vec::Vec;
use twilight_common::syscall::numbers::*;
use twilight_common::syscall::types::{Rlimit64, Timespec};
use x86_64::structures::idt::InterruptStackFrame;

#[allow(dead_code)]
pub extern "sysv64" fn syscall_handler(
    _stack_frame: &mut InterruptStackFrame,
    regs: &mut Registers,
) {
    let syscall_number = regs.rax as usize;
    #[allow(static_mut_refs)]
    let cpu_id_ptr = unsafe { crate::arch::x86_64::cpu_local::CPUID.addr().as_ptr::<usize>() as *mut usize };
    let cpu_id = unsafe { &mut *cpu_id_ptr };
    serial_prtinln!("syscall: {} cpu: {}", syscall_number, cpu_id);
    let arg1 = regs.rdi;
    let arg2 = regs.rsi;
    let arg3 = regs.rdx;
    let arg4 = regs.r10;
    let _arg5 = regs.r8;
    let _arg6 = regs.r9;

    let res = match syscall_number {
        SYS_READ => {
            let ptr = arg2 as *mut u8;
            let len = arg3;
            let buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
            read(arg1, buf, len)
        }
        SYS_WRITE => service::write(arg1 as i32, arg2, arg3),
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
        SYS_WRITEV => service::writev(arg1 as i32, arg2 as u64, arg3 as i32),
        SYS_EXIT => service::exit(),
        SYS_UNAME => service::uname(arg1),
        SYS_ARCH_PRCTL => service::arch_prctl(arg1 as u64, arg2 as u64),
        SYS_GET_TID => {
            crate::sys::proc::id() as i64
        }
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
                    sleep(req.tv_sec as f64 + req.tv_nsec as f64 / 1000000000.0);
                }
            }

            0i64
        }
        SYS_GETDENTS64 => {
            let fd = arg1 as i32;
            let buf = arg2 as *mut u8;
            let buf_len = arg3;


            service::getdent64(fd, buf, buf_len)
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
            serial_prtinln!("Unknown syscall number: {}", syscall_number);
            0
        }
    };

    regs.rax = res;

    unsafe { PICS.lock().notify_end_of_interrupt(0x80) };
}

#[repr(transparent)]
pub struct UserPtr<T>(pub *const T);
unsafe impl<T> Send for UserPtr<T> {}
unsafe impl<T> Sync for UserPtr<T> {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserCopyError {
    Fault,   // invalid mapping / page fault
    TooLong, // exceeded max length without NUL
    Utf8,    // not valid UTF-8 (optional, if you enforce)
}

/// Implement this using your page-table walker or copyin routine.
/// It must validate that `addr` is a canonical, user-mapped address and handle faults.
fn read_user_byte(addr: *const u8) -> Result<u8, UserCopyError> {
    // ---- stub / hook point ----
    // Option A (if you have a safe copyin that returns Result):
    //     copyin(addr, &mut byte).map(|_| byte).map_err(|_| UserCopyError::Fault)
    //
    // Option B (if user pages are directly accessible but may fault):
    //     unsafe { core::ptr::read_volatile(addr) }  <-- wrap in a fault catcher
    //
    // Option C: translate_user_va(addr) -> *const u8 in kernel mapping, then read.
    //
    // For now, assume you have:
    unsafe {
        if !is_user_accessible(addr as usize) {
            return Err(UserCopyError::Fault);
        }
        Ok(core::ptr::read_volatile(addr))
    }
}

/// Copy a NUL-terminated string from user space with a max cap.
pub fn copy_cstr_from_user(uptr: UserPtr<u8>, max: usize) -> Result<String, UserCopyError> {
    let mut out: Vec<u8> = Vec::new();

    for i in 0..max {
        // SAFETY: arithmetic on raw pointer, bounds enforced by `max`
        let p = unsafe { uptr.0.add(i) };
        let b = read_user_byte(p)?;
        if b == 0 {
            // Finished; optionally validate UTF-8 for POSIX paths (not required)
            return String::from_utf8(out).map_err(|_| UserCopyError::Utf8);
        }
        out.push(b);
    }

    Err(UserCopyError::TooLong)
}

// You likely already have something like this; placeholder here:
unsafe fn is_user_accessible(_va: usize) -> bool {
    // check canonical addr, U/S bit in PTEs, present, etc.
    true
}
