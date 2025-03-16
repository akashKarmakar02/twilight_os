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
    let fb_ptr = fb.addr();
    let width = fb.width() as usize;
    let height = fb.height() as usize;
    let total_pixels = width * height; // 4 bytes per pixel (ARGB or RGBA format)
    let bg_color = 0x282C34u32;

    unsafe {
        let fb_u32_ptr = fb_ptr.cast::<u32>(); // Cast to u32 pointer
        for i in 0..total_pixels {
            fb_u32_ptr.add(i).write(bg_color);
        }
    }
}

pub fn init_writer() {
    #[allow(static_mut_refs)]
    unsafe { WRITER = Some(Writer::new(0xE2E3E4)); }
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
    () => ($crate::framebuffer::_print("\n"));
    ($($arg:tt)*) => (print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    get_writer().write_fmt(args).unwrap();
}