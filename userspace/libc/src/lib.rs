#![no_std]

mod unistd;

use crate::unistd::_exit;

#[allow(dead_code)]
type SsizeT = isize;
type SizeT = usize;
type SysRet = isize;

#[inline(always)]
pub fn syscall6(n: usize, a: usize, b: usize, c: usize, d: usize, e: usize, f: usize) -> SysRet {
    let mut ret: isize;
    unsafe {
        core::arch::asm!(
        "syscall",
        inlateout("rax") n as isize => ret,   // signed result
        in("rdi") a, in("rsi") b, in("rdx") c,
        in("r10") d, in("r8") e, in("r9") f,
        lateout("rcx") _, lateout("r11") _,
        options(nostack) // (drop preserves_flags; syscall clobbers rflags into r11 anyway)
        );
    }
    ret
}


#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { _exit(127) }
