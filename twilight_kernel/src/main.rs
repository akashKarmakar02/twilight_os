#![no_std]
#![no_main]
extern crate alloc;

use core::arch::asm;

use limine::BaseRevision;
use limine::framebuffer::Framebuffer;
use limine::request::{FramebufferRequest, HhdmRequest, MemoryMapRequest};
use limine::response::{HhdmResponse, MemoryMapResponse};
use twilight_kernel::{print, println};
use twilight_kernel::driver::keyboard::keyboard_interrupt;
use twilight_kernel::task::executor::Executor;
use twilight_kernel::task::Task;

#[used]
#[unsafe(link_section = ".requests")]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[used]
#[unsafe(link_section = ".requests")]
static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MEMMAP: MemoryMapRequest = MemoryMapRequest::new();

#[unsafe(no_mangle)]
unsafe extern "C" fn kmain() -> ! {
    assert!(BASE_REVISION.is_supported());

    let mut framebuffer: Option<Framebuffer> = None;
    let mut hhdm_response: Option<&HhdmResponse> = None;
    let mut memory_map_response: Option<&MemoryMapResponse> = None;

    if let Some(framebuffer_response) = FRAMEBUFFER_REQUEST.get_response() {
        if let Some(fb) = framebuffer_response.framebuffers().next() {
            framebuffer = Some(fb);
        }
    }

    if let Some(hr) = HHDM_REQUEST.get_response() {
        hhdm_response = Some(hr);
    }

    if let Some(mmr) = MEMMAP.get_response() {
        memory_map_response = Some(mmr);
    }

    twilight_kernel::init(&framebuffer.unwrap(), hhdm_response.unwrap(), memory_map_response.unwrap());

    twilight_kernel::console::start_kernel_console();

    let mut executor = Executor::new();
    executor.spawn(Task::new(keyboard_interrupt()));
    executor.run();
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
