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
    interrupts::enable();
    crate::driver::usb::poll_all_drivers();
    unsafe {
        core::arch::asm!("hlt");
    }
    if disabled {
        interrupts::disable();
    }
}
