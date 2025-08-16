#![no_std]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
#![feature(let_chains)]
#![feature(decl_macro)]

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
    driver::uart::init();
    arch::x86_64::gdt::init();
    arch::x86_64::idt::init();
    arch::x86_64::idt::init_pics();
    init_framebuffer(fb);

    let phys_mem_offset = VirtAddr::new(hhdm_response.offset());

    memory::init(phys_mem_offset, memory_map_response.entries());

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


    arch::x86_64::gdt::init_after_boot();

    arch::x86_64::syscall::init();

    sys::proc::init();

    x86_64::instructions::interrupts::enable();
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => ({
        let time = $crate::driver::timer::pit::uptime();
        $crate::println!("\x1b[93m[{:.6}]\x1b[0m {}", time, format_args!($($arg)*));
    });
}

#[macro_export]
macro_rules! extern_sym {
    ($sym:ident) => {{
        unsafe extern "C" {
            static $sym: ::core::ffi::c_void;
        }

        // The value is not accessed, we only take its address. The `addr_of!()` ensures
        // that no intermediate references is created.
        ::core::ptr::addr_of!($sym)
    }};
}

#[alloc_error_handler]
fn alloc_error(layout: alloc::alloc::Layout) -> ! {
    panic!("allocation error: {:?}", layout);
}
