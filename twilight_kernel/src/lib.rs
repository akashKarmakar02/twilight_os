#![no_std]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]

pub mod framebuffer;
pub mod arch;
pub mod memory;
pub mod console;
pub mod driver;
pub mod kernel_utils;
pub mod task;
pub mod buffer;
pub mod fs;

extern crate alloc;

use limine::framebuffer::Framebuffer;
use limine::response::{HhdmResponse, MemoryMapResponse};
use x86_64::VirtAddr;
use crate::console::writer::init_writer;
use crate::framebuffer::{init_framebuffer};
use crate::task::executor;

pub fn init(fb: &Framebuffer, hhdm_response: &HhdmResponse, memory_map_response: &'static MemoryMapResponse) {
    init_framebuffer(fb);
    arch::x86_64::gdt::init();
    arch::x86_64::idt::init();
    arch::x86_64::idt::init_pics();
    x86_64::instructions::interrupts::enable();

    let phys_mem_offset = VirtAddr::new(hhdm_response.offset());
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe {
        memory::BootInfoFrameAllocator::init(memory_map_response.entries())
    };

    memory::allocator::init_heap(&mut mapper, &mut frame_allocator, memory_map_response).expect("Failed to initialize heap");
    executor::init_executor();
    fs::init_fs();
    init_writer();
}




#[alloc_error_handler]
fn alloc_error(layout: alloc::alloc::Layout) -> ! {
    panic!("allocation error: {:?}", layout);
}
