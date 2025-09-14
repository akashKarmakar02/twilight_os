use crate::sys::fs::{VfsError, VfsNode};
use limine::framebuffer::Framebuffer;
use spin::Once;

#[allow(static_mut_refs)]
pub static mut FRAMEBUFFER: Once<TwilightFrameBuffer> = Once::new();
pub struct TwilightFrameBuffer {
    addr: *mut u8,
    pub(crate) height: u64,
    pub(crate) width: u64,
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

    fn extract_color(&self, pixels: &[u8]) -> u32 {
        if pixels.len() < 4 {
            panic!("Input data is too short to extract a color!");
        }

        let r = pixels[0] as u32;
        let g = pixels[1] as u32;
        let b = pixels[2] as u32;

        (r << 16) | (g << 8) | b // Keep the RGB format same as input
    }

    // pub fn write(&mut self, offset: u32, buffer: &[u32], unless: bool) {
    //     let fb_ptr = self.addr();
    //     unsafe {
    //         let fb_u32_ptr = fb_ptr.cast::<u32>();
    //         for i in 1..buffer.len() {
    //             let color = buffer[i];
    //             fb_u32_ptr.add(i + offset as usize).write(color);
    //         }
    //     }
    // }
}

impl Clone for TwilightFrameBuffer {
    fn clone(&self) -> Self {
        Self {
            addr: self.addr,
            height: self.height,
            width: self.width,
            pitch: self.pitch,
        }
    }
}

impl VfsNode for TwilightFrameBuffer {
    fn read(&self, _offset: u64, _buffer: &mut [u8]) -> Result<usize, VfsError> {
        Err(VfsError::PermissionDenied)
    }

    fn write(&mut self, offset: u64, buffer: &[u8]) -> Result<usize, VfsError> {
        if buffer.len() % 4 != 0 {
            return Err(VfsError::InvalidOperation);
        }

        let fb_ptr = self.addr();

        unsafe {
            let fb_u32_ptr = fb_ptr.cast::<u32>();
            for i in 0..(buffer.len() / 4) {
                let color = self.extract_color(&buffer[i * 4..(i + 1) * 4]);
                fb_u32_ptr.add(i + offset as usize).write_volatile(color);
            }
        }

        Ok(buffer.len())
    }

    fn size(&self) -> u64 {
        self.width * self.height
    }

    fn is_directory(&self) -> bool {
        false
    }
}

unsafe impl Send for TwilightFrameBuffer {}

unsafe impl Sync for TwilightFrameBuffer {}

pub fn init_framebuffer(fb: &Framebuffer) {
    #[allow(static_mut_refs)]
    unsafe {
        FRAMEBUFFER.call_once(|| TwilightFrameBuffer::new(fb));
    }
}

pub fn get_pitch() -> u64 {
    #[allow(static_mut_refs)]
    unsafe {
        FRAMEBUFFER.get().unwrap().pitch()
    }
}

pub fn convert_color(color: u32) -> [u8; 4] {
    let rgba: [u8; 4] = [
        ((color >> 16) & 0xFF) as u8, // Red
        ((color >> 8) & 0xFF) as u8,  // Green
        (color & 0xFF) as u8,         // Blue
        255,                          // Alpha (fully opaque)
    ];

    rgba
}

pub fn get_framebuffer() -> &'static TwilightFrameBuffer {
    #[allow(static_mut_refs)]
    unsafe {
        FRAMEBUFFER.get().unwrap()
    }
}

pub fn get_framebuffer_mut() -> &'static mut TwilightFrameBuffer {
    #[allow(static_mut_refs)]
    unsafe {
        FRAMEBUFFER.get_mut().unwrap()
    }
}
