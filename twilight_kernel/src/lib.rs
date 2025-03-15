#![feature(abi_x86_interrupt)]
#![no_std]

pub mod framebuffer;
pub mod arch;

use limine::framebuffer::Framebuffer;
use crate::framebuffer::{init_framebuffer, init_writer};

pub fn init(fb: &Framebuffer) {
    init_framebuffer(fb);
    init_writer();
    arch::x86_64::gdt::init();
    arch::x86_64::idt::init();
    // unsafe { arch::x86_64::idt::PICS.lock().initialize() };
    // x86_64::instructions::interrupts::enable();
}
