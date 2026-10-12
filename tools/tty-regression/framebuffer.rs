//! Host regression tests for the kernel framebuffer terminal.
//! Only framebuffer storage and VFS I/O are substituted; decoding, glyphs,
//! cursor drawing, and editing use the production modules. Tests serialize
//! access to the global framebuffer just as the single-CPU console does.
#![allow(dead_code, static_mut_refs)]

use std::sync::{Mutex, MutexGuard};

#[path = "../../kernel/twilight_kernel/src/sys/console/font.rs"]
pub mod font;
#[path = "../../kernel/twilight_kernel/src/sys/console/framebuffer.rs"]
mod terminal;

mod driver {
    pub mod disk {
        pub fn dummy_blockdev() {}
    }
}

mod sys {
    pub mod console {
        pub use crate::font;
    }

    pub mod fs {
        pub mod vfs {
            pub trait VfsNodeOps {
                fn read(
                    &mut self,
                    dev: &mut (),
                    offset: usize,
                    buf: &mut [u8],
                ) -> Result<usize, ()>;
                fn write(&mut self, dev: &mut (), offset: usize, buf: &[u8]) -> Result<usize, ()>;
            }
        }
    }

    pub mod framebuffer {
        use super::fs::vfs::VfsNodeOps;

        pub struct Framebuffer {
            pub width: u64,
            pub height: u64,
            pixels: Vec<u32>,
            pub video_buf: Vec<u32>,
        }

        impl Framebuffer {
            pub fn new(width: usize, height: usize) -> Self {
                Self {
                    width: width as u64,
                    height: height as u64,
                    pixels: vec![0; width * height],
                    video_buf: vec![0; width * height],
                }
            }

            pub fn pixels_mut(&mut self) -> &mut [u32] {
                &mut self.pixels
            }

            pub fn sync_partial(&self, _offset: u64, _len: u64) {}
            pub fn sync_full(&self) {}

            pub fn scroll_up(&mut self, rows: u64, color: u32) {
                let offset = (rows * self.width) as usize;
                let end = self.pixels.len() - offset;
                self.pixels.copy_within(offset.., 0);
                self.pixels[end..].fill(color);
                self.video_buf.copy_from_slice(&self.pixels);
            }

            pub fn snapshot(&self) -> (Vec<u32>, Vec<u32>) {
                (self.pixels.clone(), self.video_buf.clone())
            }
        }

        impl VfsNodeOps for Framebuffer {
            fn read(&mut self, _dev: &mut (), offset: usize, buf: &mut [u8]) -> Result<usize, ()> {
                for (pixel, bytes) in self.pixels[offset..].iter().zip(buf.chunks_exact_mut(4)) {
                    bytes.copy_from_slice(&pixel.to_ne_bytes());
                }
                Ok(buf.len())
            }

            fn write(&mut self, _dev: &mut (), offset: usize, buf: &[u8]) -> Result<usize, ()> {
                for (index, bytes) in buf.chunks_exact(4).enumerate() {
                    let pixel = u32::from_ne_bytes(bytes.try_into().unwrap());
                    self.pixels[offset + index] = pixel;
                    self.video_buf[offset + index] = pixel;
                }
                Ok(buf.len())
            }
        }

        pub struct FramebufferSlot(pub Option<Framebuffer>);

        impl FramebufferSlot {
            pub fn get_mut(&mut self) -> Option<&mut Framebuffer> {
                self.0.as_mut()
            }
        }

        pub static mut FRAMEBUFFER: FramebufferSlot = FramebufferSlot(None);

        pub fn get_framebuffer() -> &'static mut Framebuffer {
            // SAFETY: Each test holds TEST_LOCK while using the terminal.
            unsafe { FRAMEBUFFER.get_mut().unwrap() }
        }

        pub fn convert_color(color: u32) -> [u8; 4] {
            color.to_ne_bytes()
        }
    }
}

use sys::framebuffer::{FRAMEBUFFER, Framebuffer, get_framebuffer};
use terminal::FramebufferTerminal;

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn setup() -> MutexGuard<'static, ()> {
    let guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    // SAFETY: The guard serializes all accesses, including renderer calls.
    unsafe { FRAMEBUFFER.0 = Some(Framebuffer::new(80, 64)) };
    guard
}

#[test]
fn bell_preserves_pixels_and_cursor() {
    let _guard = setup();
    let mut term = FramebufferTerminal::new();
    term.write("ok");
    term.refresh_cursor();
    let before = get_framebuffer().snapshot();
    let position = (term.cursor_x, term.cursor_y);

    term.write("\x07\x07\x07\x07");

    assert_eq!((term.cursor_x, term.cursor_y), position);
    assert_eq!(get_framebuffer().snapshot(), before);
}

#[test]
fn completion_bells_then_backspace_match_normal_editing() {
    let _guard = setup();
    let mut term = FramebufferTerminal::new();
    term.write("clear\x07\x07\x07\x07\x08 \x08n");
    term.refresh_cursor();
    let edited = get_framebuffer().snapshot();
    let position = (term.cursor_x, term.cursor_y);

    let mut expected = FramebufferTerminal::new();
    expected.write("clean");
    expected.refresh_cursor();
    assert_eq!(position, (expected.cursor_x, expected.cursor_y));
    assert_eq!(edited, get_framebuffer().snapshot());
}

#[test]
fn bell_at_right_margin_does_not_wrap_or_scroll() {
    let _guard = setup();
    let mut term = FramebufferTerminal::new();
    term.write("1234567890\n1234567890\n1234567890\n1234567890");
    let before = get_framebuffer().snapshot();
    let position = (term.cursor_x, term.cursor_y);

    term.write("\x07\x07");

    assert_eq!((term.cursor_x, term.cursor_y), position);
    assert_eq!(get_framebuffer().snapshot(), before);
}
