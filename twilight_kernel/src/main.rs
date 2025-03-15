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


#[unsafe(no_mangle)]
unsafe extern "C" fn kmain() -> ! {
    assert!(BASE_REVISION.is_supported());

    if let Some(framebuffer_response) = FRAMEBUFFER_REQUEST.get_response() {
        if let Some(framebuffer) = framebuffer_response.framebuffers().next() {
            twilight_kernel::init(&framebuffer);
        }
    }

    for i in 0..51 {
        println!("Hello from Twilight kernel! {}", i);
    }

    x86_64::instructions::interrupts::int3();

    println!("recovered from interrupt");
    hcf();
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