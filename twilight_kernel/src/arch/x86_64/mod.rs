use x86_64::instructions::interrupts;

pub mod idt;
pub mod gdt;
pub mod power;

pub fn halt() {
    let disabled = !interrupts::are_enabled();
    interrupts::enable_and_hlt();
    if disabled {
        interrupts::disable();
    }
}