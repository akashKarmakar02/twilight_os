#![no_std]
#![no_main]

use core::arch::asm;

use limine::BaseRevision;
use limine::request::{FramebufferRequest};
use twilight_kernel::{print, println};

#[used]
#[unsafe(link_section = ".requests")]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[used]
#[unsafe(link_section = ".requests")]
static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

use x86_64::instructions::port::Port;

pub fn init_pit() {
    let mut command = Port::<u8>::new(0x43);
    let mut channel0 = Port::<u8>::new(0x40);

    let frequency = 1000; // Set timer frequency to 1000Hz (1ms interval)
    let divisor: u16 = (1193182 / frequency) as u16; // Calculate divisor

    unsafe {
        command.write(0x36); // PIT mode 3 (Square Wave Generator)
        channel0.write((divisor & 0xFF) as u8); // Low byte
        channel0.write((divisor >> 8) as u8);   // High byte
    }
}


#[unsafe(no_mangle)]
unsafe extern "C" fn kmain() -> ! {
    assert!(BASE_REVISION.is_supported());
    if let Some(framebuffer_response) = FRAMEBUFFER_REQUEST.get_response() {
        if let Some(framebuffer) = framebuffer_response.framebuffers().next() {
            twilight_kernel::init(&framebuffer);
        }
    }

    println!("{}", check_interrupts_enabled());
    println!("Hello from Twilight kernel!");

    hcf();
}

pub fn check_interrupts_enabled() -> bool {
    let flags: u64;
    unsafe {
        asm!("pushfq; pop {}", out(reg) flags, options(nomem, nostack));
    }
    flags & (1 << 9) != 0 // Check if IF (bit 9) is set
}

#[panic_handler]
fn rust_panic(info: &core::panic::PanicInfo) -> ! {
    println!("{}", info);
    hcf();
}

fn hcf() -> ! {
    loop {
        unsafe {
            #[cfg(target_arch = "x86_64")]
            asm!("hlt");
            #[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
            asm!("wfi");
            #[cfg(target_arch = "loongarch64")]
            asm!("idle 0");
        }
    }
}