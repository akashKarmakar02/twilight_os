use alloc::vec::Vec;

use crate::sys::framebuffer::{get_framebuffer_mut, TwilightFrameBuffer};

const _HEIGHT: u16 = 720;
const _WIDTH: u16 = 1280;

const BUF_SIZE: u32 = 1280 * 720;

#[allow(dead_code)]
enum ViMode {
    Command,
    Insert,
    View,
}

#[allow(dead_code)]
struct Vi {
    content: Vec<u8>,
    mode: ViMode,
    cursor_position: u64,
    framebuffer: &'static mut TwilightFrameBuffer,
}

impl Vi {
    fn new(content: Vec<u8>) -> Self {
        let fb = get_framebuffer_mut();
        Self {
            content,
            cursor_position: 0,
            mode: ViMode::Command,
            framebuffer: fb,
        }
    }

    fn init(&mut self) {
        let buf = [0x101010u32; BUF_SIZE as usize];
        let fb_ptr = self.framebuffer.addr();

        unsafe {
            let fb_u32_ptr = fb_ptr.cast::<u32>();
            for i in 1..buf.len() {
                let color = buf[i];
                fb_u32_ptr.add(i).write(color);
            }
        }
    }
}

pub fn main(_args: &[&str]) {
    let mut demo_content = Vec::new();
    demo_content.push(b'h');
    demo_content.push(b'e');
    demo_content.push(b'l');
    demo_content.push(b'l');
    demo_content.push(b'o');
    let mut vi = Vi::new(demo_content);

    vi.init();
}
