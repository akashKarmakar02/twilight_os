use x86_64::instructions::interrupts;

pub mod idt;
pub mod gdt;
pub mod power;
pub mod syscall;
pub mod asm_utils;
pub mod io;
pub mod cpu_local;

pub fn halt() {
    let disabled = !interrupts::are_enabled();
    interrupts::enable_and_hlt();
    if disabled {
        interrupts::disable();
    }
}