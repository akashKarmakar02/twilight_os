mod service;

use x86_64::structures::idt::InterruptStackFrame;
use crate::arch::x86_64::idt::{Registers, PICS};
use crate::{print, println};
use twilight_common::syscall::numbers::*;
use crate::sys::syscall::service::read;

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
        SYS_WRITE => {
            let file_descriptor = arg1;
            let buf = arg2 as *const u8;
            let len = arg3;
            let res = unsafe { core::slice::from_raw_parts(buf, len) };

            if file_descriptor == 1 {
                print!("{}", core::str::from_utf8(res).unwrap());
            }

            len
        }
        SYS_EXIT => {
            println!("Exiting with code {}", arg1);

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
