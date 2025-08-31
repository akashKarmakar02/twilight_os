#![no_std]
#![no_main]
#![feature(asm_const)]

use core::panic::PanicInfo;


const STACK_SIZE: usize = 16 * 1024;

#[unsafe(link_section = ".bss.stack")]
static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::asm!(
        "move rsp, {stack_top}",
        stack_top = in(reg) STACK.as_ptr().add(STACK_SIZE),
        options(nostack, preserves_flags)
    );

    unsafe extern "C" {
        fn kmain();
    }
    kmain();

    loop {
        core::arch::asm!("hlt", options(nostack));
    }
}

#[panic_handler]
unsafe fn panic(info: &PanicInfo) -> ! {
    loop {
        core::arch::asm!("hlt", options(nostack));
    }
}
