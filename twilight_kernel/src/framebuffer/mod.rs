use core::fmt;
use limine::framebuffer::Framebuffer;
use spin::Once;
use crate::framebuffer::writer::Writer;

pub mod font;
pub mod writer;

#[allow(static_mut_refs)]
pub static mut FRAMEBUFFER: Once<TwilightFrameBuffer> = Once::new();
pub struct TwilightFrameBuffer {
    addr: *mut u8,
    height: u64,
    width: u64,
    pitch: u64,
}

impl TwilightFrameBuffer {
    pub fn new(fb: &Framebuffer) -> Self {
        Self {
            addr: fb.addr(),
            width: fb.width(),
            height: fb.height(),
            pitch: fb.pitch(),
        }
    }

    pub fn addr(&self) -> *mut u8 {
        self.addr
    }
    pub fn width(&self) -> u64 {
        self.width
    }
    pub fn height(&self) -> u64 {
        self.height
    }
    pub fn pitch(&self) -> u64 {
        self.pitch
    }
}

static mut WRITER: Option<Writer> = None;

pub fn init_framebuffer(fb: &Framebuffer) {
    #[allow(static_mut_refs)]
    unsafe {
        FRAMEBUFFER.call_once(|| TwilightFrameBuffer::new(fb));
    }
}

pub fn init_writer() {
    #[allow(static_mut_refs)]
    unsafe { WRITER = Some(Writer::new(0xFFFFFF)); }
}

pub fn get_writer() -> &'static mut Writer {
    #[allow(static_mut_refs)]
    unsafe { WRITER.as_mut().expect("Writer not initialized") }
}


pub fn get_framebuffer() -> &'static TwilightFrameBuffer {
    #[allow(static_mut_refs)]
    unsafe { FRAMEBUFFER.get().unwrap() }
}


#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::framebuffer::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => (print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    get_writer().write_fmt(args).unwrap();
}