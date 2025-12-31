use x86_64::instructions::interrupts;

pub mod asm_utils;
pub mod cpu_local;
pub mod gdt;
pub mod idt;
pub mod io;
pub mod power;
pub mod syscall;

pub fn halt() {
    let disabled = !interrupts::are_enabled();
    interrupts::enable_and_hlt();
    if disabled {
        interrupts::disable();
    }
}
