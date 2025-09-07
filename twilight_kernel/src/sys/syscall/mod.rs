mod service;

use crate::arch::x86_64::idt::{Registers, PICS};
use crate::driver::timer::cmos::CMOS;
use crate::sys::syscall::service::read;
use crate::task::executor::sleep;
use crate::println;
use twilight_common::syscall::numbers::*;
use twilight_common::syscall::types::Timespec;
use x86_64::structures::idt::InterruptStackFrame;

#[allow(dead_code)]
pub extern "sysv64" fn syscall_handler(
    _stack_frame: &mut InterruptStackFrame,
    regs: &mut Registers
) {
    let syscall_number = regs.rax;
    let arg1 = regs.rdi;
    let arg2 = regs.rsi;
    let arg3 = regs.rdx;
    let _arg4 = regs.r10;
    let _arg5 = regs.r8;
    let _arg6 = regs.r9;
    
    let res = match syscall_number {
        SYS_READ => {
            let ptr = arg2 as *mut u8;
            let len = arg3;
            let buf = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
            read(arg1, buf, len)
        },
        SYS_WRITE => service::write(arg1 as i32, arg2, arg3),
        SYS_OPEN => {
            
            0
        }
        SYS_WRITEV => service::writev(arg1 as i32, arg2 as u64, arg3 as i32),
        SYS_ARCH_PRCTL => service::arch_prctl(arg1 as u64, arg2 as u64),
        SYS_EXIT => {
            service::exit()
        }
        SYS_TIME => {
            let out_ptr = arg1 as *mut i64; // time_t is i64
            let mut cmos = CMOS::new();
            let unix_time: u64 = cmos.unix_time();

            if !out_ptr.is_null() {
                unsafe { *out_ptr = unix_time as i64 };
            }
            unix_time as usize
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

            0
        }
        _ => {
            println!("Unknown syscall number: {}", syscall_number);
            0
        },
    };

    regs.rax = res;

    unsafe { PICS.lock().notify_end_of_interrupt(0x80) };
}
