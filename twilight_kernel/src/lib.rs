#![no_std]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
#![feature(let_chains)]
//#![feature(core_float_math)]

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


unsafe extern "C" fn ap_main(_cpu: &limine::mp::Cpu) -> ! {
    use x86_64::instructions::{hlt, interrupts};

    interrupts::enable();

    loop {
        hlt();
    }
}


pub fn init_smp(mp_response: &'static MpResponse) {
    let smp = mp_response;
    let bsp_id = mp_response.bsp_lapic_id();

    let time = driver::timer::pit::uptime();

    for i in 0..smp.cpus().len() {
        let cpu = smp.cpus().get(i).unwrap();
        let apic_id = cpu.lapic_id;

        if apic_id == bsp_id {
            println!("\x1b[93m[{:.6}]\x1b[0m BSP Core {}: APIC ID {}", time, i, apic_id, );
        } else {
            println!("\x1b[93m[{:.6}]\x1b[0m AP Core {}: APIC ID {}", time, i, apic_id);
            
            cpu.goto_address.write(ap_main);
        }
    }
}

pub fn init(fb: &Framebuffer, hhdm_response: &HhdmResponse, memory_map_response: &'static MemoryMapResponse, mp_response: &'static MpResponse) {
    init_framebuffer(fb);
    arch::x86_64::gdt::init();
    arch::x86_64::idt::init();
    arch::x86_64::idt::init_pics();
    driver::uart::init();
    x86_64::instructions::interrupts::enable();
    driver::timer::init();

    let phys_mem_offset = VirtAddr::new(hhdm_response.offset());
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe {
        memory::BootInfoFrameAllocator::init(memory_map_response.entries())
    };

    memory::allocator::init_heap(&mut mapper, &mut frame_allocator, memory_map_response).expect("Failed to initialize heap");
    executor::init_executor();
    fs::init_fs();
    init_writer();

    driver::pci::init();
    driver::cpu::init();
    driver::disk::ata::init();
    init_smp(mp_response);
}




#[alloc_error_handler]
fn alloc_error(layout: alloc::alloc::Layout) -> ! {
    panic!("allocation error: {:?}", layout);
}
