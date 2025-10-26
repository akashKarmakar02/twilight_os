use crate::sys::{
    console::font::PSF_FONTS,
    framebuffer::{convert_color, get_framebuffer, FRAMEBUFFER},
    fs::VfsNode,
};

use crate::sys::framebuffer::get_framebuffer_mut;
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
        let fb = get_framebuffer_mut();
        let pitch = fb.width as usize;
        let mut buf = vec![0u8; w * h * 4];
        let color_bytes = convert_color(color);

        let start = y * pitch + x;

        for i in 0..buf.len() / 4 {
            buf[i * 4..i * 4 + 4].clone_from_slice(&color_bytes);
        }
        #[allow(static_mut_refs)]
        unsafe {
            let fb = FRAMEBUFFER.get_mut().unwrap();
            fb.write(start as u64, buf.as_slice()).unwrap();
        }
        fb.sync_partial(start as u64, w as u64 * h as u64);
    }

    fn backspace(&mut self) {
        if self.cursor_x > 0 {
            self.cursor_x -= 1;
            let x = self.cursor_x * Self::CHAR_W;
            let y = self.cursor_y * Self::CHAR_H;
            self.draw_char(x, y, ScreenChar { char: b' ', color: self.color });
        } else {
            // At column 0: keep it simple (no wrap). If you want wrap:
            // if self.cursor_y > 0 { self.cursor_y -= 1; self.cursor_x = self.cols().saturating_sub(1); }
        }
    }

    /// Clears the framebuffer with the background color
    pub fn clear(&mut self) {
        apply_console_bg(self.bg_color);
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    /// Write a single character at the current cursor position
    pub fn put_char(&mut self, c: u8) {
        if c == b'\n' {
            self.new_line();
            return;
        }

        if c == 0x08 || c == 0x7F {
            self.backspace();
            return;
        }

        if c == b'\r' {
            self.cursor_x = 0;
            return;
        }

        let x = self.cursor_x * 8;
        let y = self.cursor_y * 16;
        self.draw_char(
            x,
            y,
            ScreenChar {
                char: c,
                color: self.color,
            },
        );

        self.cursor_x += 1;
        if self.cursor_x * 8 >= self.width {
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
        let pitch = self.width;
        let color = screen_char.color;
        let ascii = screen_char.char;

        if let Some(font_bitmap) = PSF_FONTS.get(ascii as usize - 32) {
            let color_bytes = convert_color(color);
            let background_color_bytes = convert_color(self.bg_color);

            for (row, &bitmap) in font_bitmap.iter().enumerate() {
                let mut row_buf = vec![0u8; 8 * 4]; // 8 pixels * 4 bytes/pixel
                for col in 0..8 {
                    if (bitmap & (1 << (7 - col))) != 0 {
                        row_buf[col * 4..(col + 1) * 4].clone_from_slice(&color_bytes);
                    } else {
                        row_buf[col * 4..(col + 1) * 4].clone_from_slice(&background_color_bytes);
                    }
                }

                let pixel_offset = ((y + row) * pitch) + x; // pixel index
                #[allow(static_mut_refs)]
                unsafe {
                    let fb = FRAMEBUFFER.get_mut().unwrap();
                    fb.write(pixel_offset as u64, row_buf.as_slice()).unwrap();
                }
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
        fb.write(0, buf.as_slice()).unwrap();
    }
}
