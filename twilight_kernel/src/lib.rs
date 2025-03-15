#![no_std]

pub mod framebuffer;

use limine::framebuffer::Framebuffer;
use crate::framebuffer::{init_framebuffer, init_writer};

pub fn init(fb: &Framebuffer) {
    init_framebuffer(fb);
    init_writer();
}
