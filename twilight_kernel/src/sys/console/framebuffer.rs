use crate::sys::{
    console::font::PSF_FONTS,
    framebuffer::{FRAMEBUFFER, convert_color, get_framebuffer},
};

use crate::driver::disk::dummy_blockdev;
use crate::sys::fs::vfs::VfsNodeOps;
use alloc::vec;

/// A framebuffer-based terminal backend (no ANSI parsing, pure rendering)
#[derive(Clone, Copy, Debug)]
pub struct FramebufferTerminal {
    pub width: usize,
    pub height: usize,
    pub cursor_x: usize,
    pub cursor_y: usize,
    pub color: u32,
    pub bg_color: u32,
    pub reverse: bool,
}

#[derive(Clone, Copy)]
pub struct ScreenChar {
    pub char: u8,
    pub color: u32,
}

impl FramebufferTerminal {
    const CHAR_W: usize = 8;
    const CHAR_H: usize = 16;

    /// Initialize and clear the framebuffer console
    pub fn new() -> Self {
        let fb = get_framebuffer();
        let (width, height) = (fb.width as usize, fb.height as usize);
        let mut term = Self {
            width,
            height,
            cursor_x: 0,
            cursor_y: 0,
            color: 0xFFFFFF,
            bg_color: 0x101010,
            reverse: false,
        };
        term.clear();
        term
    }

    pub fn set_cursor_visible(&mut self, _v: bool) {
        // store a flag if you later want to draw a caret; for now this can be a no-op
    }

    pub fn erase_line(&mut self) {
        let y = self.cursor_y * 16;
        self.fill_rect(0, y, self.width, 16, self.bg_color);
        self.cursor_x = 0;
    }
    pub fn erase_in_line_from_cursor(&mut self) {
        let x = self.cursor_x * 8;
        let y = self.cursor_y * 16;
        self.fill_rect(x, y, self.width.saturating_sub(x), 16, self.bg_color);
    }
    pub fn erase_in_line_to_cursor(&mut self) {
        let x = self.cursor_x * 8;
        let y = self.cursor_y * 16;
        self.fill_rect(0, y, x, 16, self.bg_color);
    }
    pub fn erase_display_from_cursor(&mut self) {
        // clear from cursor to end of screen
        let x = self.cursor_x * 8;
        let y = self.cursor_y * 16;
        // clear part of current line
        self.fill_rect(x, y, self.width.saturating_sub(x), 16, self.bg_color);
        // clear all lines below
        if y + 16 < self.height {
            self.fill_rect(0, y + 16, self.width, self.height - (y + 16), self.bg_color);
        }
    }
    pub fn erase_display_to_cursor(&mut self) {
        let x = self.cursor_x * 8;
        let y = self.cursor_y * 16;
        // clear all lines above
        if y > 0 {
            self.fill_rect(0, 0, self.width, y, self.bg_color);
        }
        // clear part of current line up to cursor
        self.fill_rect(0, y, x, 16, self.bg_color);
    }

    fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        let pitch_pixels = get_framebuffer().width as usize; // pixels per row
        let mut row_buf = vec![0u8; w * 4];
        let color_bytes = convert_color(color);

        // prepare one scanline filled with bg color
        for px in 0..w {
            let off = px * 4;
            row_buf[off..off + 4].copy_from_slice(&color_bytes);
        }

        #[allow(static_mut_refs)]
        unsafe {
            let fb = FRAMEBUFFER.get_mut().unwrap();
            for row in 0..h {
                let pixel_offset = (y + row) * pitch_pixels + x; // pixel index, not bytes
                fb.write(&mut dummy_blockdev(), pixel_offset, &row_buf)
                    .unwrap();
            }
            // If you have a cheap partial sync, sync the whole rect; otherwise skip.
            fb.sync_partial(((y * pitch_pixels) + x) as u64, (w * h) as u64);
        }
    }

    fn backspace(&mut self) {
        if self.cursor_x > 0 {
            self.cursor_x -= 1;
            let x = self.cursor_x * Self::CHAR_W;
            let y = self.cursor_y * Self::CHAR_H;
            self.draw_char(
                x,
                y,
                ScreenChar {
                    char: b' ',
                    color: self.color,
                },
            );
        } else {
            // At column 0: keep it simple (no wrap). If you want wrap:
            // if self.cursor_y > 0 { self.cursor_y -= 1; self.cursor_x = self.cols().saturating_sub(1); }
        }
    }
    pub fn set_reverse(&mut self, v: bool) {
        // NEW
        self.reverse = v;
    }

    /// Clears the framebuffer with the background color
    pub fn clear(&mut self) {
        apply_console_bg(self.bg_color);
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    /// Write a single character at the current cursor position
    pub fn put_char(&mut self, c: u8) {
        match c {
            b'\n' => {
                self.new_line();
                return;
            }
            0x08 | 0x7F => {
                self.backspace();
                return;
            }
            b'\r' => {
                self.cursor_x = 0;
                return;
            }
            _ => {}
        }

        // Clamp to printable 32..=126; render others as space so font index is valid
        let ch = if (32..=126).contains(&c) { c } else { b' ' };

        let x = self.cursor_x * Self::CHAR_W;
        let y = self.cursor_y * Self::CHAR_H;
        self.draw_char(
            x,
            y,
            ScreenChar {
                char: ch,
                color: self.color,
            },
        );

        self.cursor_x += 1;
        if self.cursor_x * Self::CHAR_W > self.width {
            self.new_line();
        }
    }

    /// Writes a full string (no ANSI yet)
    pub fn write(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.put_char(b);
        }
    }

    /// Move cursor to new line, scroll if needed
    fn new_line(&mut self) {
        self.cursor_x = 0;
        self.cursor_y += 1;
        if (self.cursor_y) * 16 >= self.height {
            self.scroll();
            self.cursor_y -= 1;
        }
    }

    /// Scroll the framebuffer content up by one character row (16 pixels)
    fn scroll(&mut self) {
        let char_height = 16;

        #[allow(static_mut_refs)]
        unsafe {
            let fb = FRAMEBUFFER.get_mut().unwrap();
            fb.scroll_up(char_height as u64, self.bg_color);
            fb.sync_full();
        }
    }

    /// Draw a single ScreenChar at a pixel coordinate
    fn draw_char(&self, x: usize, y: usize, screen_char: ScreenChar) {
        let pitch_pixels = self.width;
        let ascii = screen_char.char;
        let (fg, bg) = if self.reverse {
            (
                convert_color(self.bg_color),
                convert_color(screen_char.color),
            )
        } else {
            (
                convert_color(screen_char.color),
                convert_color(self.bg_color),
            )
        };

        // Guard index into PSF_FONTS
        let glyph_opt = if (32..=126).contains(&ascii) {
            PSF_FONTS.get((ascii - 32) as usize)
        } else {
            PSF_FONTS.get(0) // space
        };

        if let Some(font_bitmap) = glyph_opt {
            #[allow(static_mut_refs)]
            unsafe {
                let fb = FRAMEBUFFER.get_mut().unwrap();

                for (row, &bits) in font_bitmap.iter().enumerate() {
                    // Build one scanline: bg everywhere, then overwrite fg where bit=1
                    let mut row_buf = vec![0u8; Self::CHAR_W * 4];
                    for col in 0..Self::CHAR_W {
                        // Start with bg
                        let off = col * 4;
                        row_buf[off..off + 4].copy_from_slice(&bg);
                        // Overlay fg if pixel bit is set
                        if (bits & (1 << (7 - col))) != 0 {
                            row_buf[off..off + 4].copy_from_slice(&fg);
                        }
                    }

                    let pixel_offset = (y + row) * pitch_pixels + x; // pixel index
                    fb.write(&mut dummy_blockdev(), pixel_offset, &row_buf)
                        .unwrap();
                }

                // Optional: sync only the glyph area
                // fb.sync_partial((y * pitch_pixels + x) as u64, (Self::CHAR_W * Self::CHAR_H) as u64);
            }
        }
    }

    /// Set text color (foreground)
    pub fn set_color(&mut self, color: u32) {
        self.color = color;
    }

    /// Set background color
    pub fn set_bg(&mut self, color: u32) {
        self.bg_color = color;
        apply_console_bg(color);
    }
}

/// Fill the entire framebuffer with a color
fn apply_console_bg(color: u32) {
    let fb = get_framebuffer();
    let width = fb.width as usize;
    let height = fb.height as usize;
    let total_pixels = width * height;

    let mut buf = vec![0u8; total_pixels * 4];
    let color_bytes = convert_color(color);

    for i in 0..total_pixels {
        let start = i * 4;
        buf[start..start + 4].clone_from_slice(&color_bytes);
    }

    #[allow(static_mut_refs)]
    unsafe {
        let fb = FRAMEBUFFER.get_mut().unwrap();
        fb.write(&mut dummy_blockdev(), 0, buf.as_slice()).unwrap();
    }
}
