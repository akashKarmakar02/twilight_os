#![no_std]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
#![feature(let_chains)]
pub mod arch;
pub mod driver;
pub mod kernel_utils;
pub mod task;
pub mod sys;

extern crate alloc;

use limine::framebuffer::Framebuffer;
use limine::response::{HhdmResponse, MemoryMapResponse, MpResponse};
use x86_64::VirtAddr;
use sys::{fs, memory};
use crate::sys::console::writer::init_writer;
use sys::framebuffer::init_framebuffer;
use crate::task::executor;

pub fn init(fb: &Framebuffer, hhdm_response: &HhdmResponse, memory_map_response: &'static MemoryMapResponse, mp_response: &'static MpResponse) {
    init_framebuffer(fb);
    arch::x86_64::gdt::init();
    arch::x86_64::idt::init();
    arch::x86_64::idt::init_pics();
    driver::uart::init();
    x86_64::instructions::interrupts::enable();
    driver::timer::init();

    let phys_mem_offset = VirtAddr::new(hhdm_response.offset());
    unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe {
        memory::BootInfoFrameAllocator::init(memory_map_response.entries())
    };

    memory::allocator::init_heap(&mut frame_allocator, memory_map_response).expect("Failed to initialize heap");
    executor::init_executor();
    fs::init_fs();
    init_writer();

    sys::pci::init();
    // depends on pci initialization
    driver::nic::init();
    driver::usb::init();
    
    driver::cpu::init(mp_response);
    driver::disk::ata::init();
    fs::init(true);
}


#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => ({
        let time = $crate::driver::timer::pit::uptime();
        $crate::println!("\x1b[93m[{:.6}]\x1b[0m {}", time, format_args!($($arg)*));
    });
}


#[alloc_error_handler]
fn alloc_error(layout: alloc::alloc::Layout) -> ! {
    panic!("allocation error: {:?}", layout);
}
