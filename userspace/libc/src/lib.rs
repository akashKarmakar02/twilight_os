#![no_std]

mod unistd;

use crate::unistd::_exit;

#[allow(dead_code)]
type SsizeT = isize;
type SizeT = usize;

#[inline(always)]
pub fn syscall6(n: usize, a: usize, b: usize, c: usize, d: usize, e: usize, f: usize) -> SizeT {
    let mut ret: usize;
    unsafe {
        core::arch::asm!(
        "syscall",
        inlateout("rax") n as isize => ret,
        in("rdi") a, in("rsi") b, in("rdx") c,
        in("r10") d, in("r8") e, in("r9") f,
        lateout("rcx") _, lateout("r11") _,
        options(nostack, preserves_flags)
        );
    }

    ret
}

#[unsafe(no_mangle)]
pub extern "C" fn syscall(n: usize, a: usize, b: usize, c: usize) -> SizeT {
    syscall6(n, a, b, c, 0, 0, 0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { _exit(127) }
