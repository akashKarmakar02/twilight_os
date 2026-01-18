use crate::sys::{
    console::font::PSF_FONTS,
    framebuffer::{FRAMEBUFFER, convert_color, get_framebuffer},
};

use crate::driver::disk::dummy_blockdev;
use crate::sys::fs::vfs::VfsNodeOps;
use alloc::vec;

const CURSOR_W: usize = 8;
const CURSOR_H: usize = 16;
const CURSOR_BACKUP_LEN: usize = CURSOR_W * CURSOR_H;

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
    cursor_visible: bool,
    cursor_drawn: bool,
    cursor_saved_x: usize,
    cursor_saved_y: usize,
    cursor_backup: [u32; CURSOR_BACKUP_LEN],
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
            cursor_visible: true,
            cursor_drawn: false,
            cursor_saved_x: 0,
            cursor_saved_y: 0,
            cursor_backup: [0; CURSOR_BACKUP_LEN],
        };
        term.clear();
        term.draw_cursor();
        term
    }

    pub fn set_cursor_visible(&mut self, v: bool) {
        if v {
            self.cursor_visible = true;
            self.draw_cursor();
        } else {
            self.erase_cursor();
            self.cursor_visible = false;
        }
    }

    fn cursor_rect(&self, cell_x: usize, cell_y: usize) -> Option<(usize, usize, usize, usize)> {
        let x = cell_x.saturating_mul(Self::CHAR_W);
        let y = cell_y.saturating_mul(Self::CHAR_H);
        if x + CURSOR_W > self.width || y + CURSOR_H > self.height {
            return None;
        }
        Some((x, y, CURSOR_W, CURSOR_H))
    }

    fn erase_cursor(&mut self) {
        if !self.cursor_drawn {
            return;
        }
        let Some((x, y, w, h)) = self.cursor_rect(self.cursor_saved_x, self.cursor_saved_y) else {
            self.cursor_drawn = false;
            return;
        };

        #[allow(static_mut_refs)]
        unsafe {
            let fb = FRAMEBUFFER.get_mut().unwrap();
            let pitch_pixels = fb.width as usize;
            for row in 0..h {
                let start = row * w;
                let bytes = core::slice::from_raw_parts(
                    self.cursor_backup[start..start + w].as_ptr() as *const u8,
                    w * 4,
                );
                let pixel_offset = (y + row) * pitch_pixels + x;
                let _ = fb.write(&mut dummy_blockdev(), pixel_offset, bytes);
            }
            fb.sync_partial(((y * pitch_pixels) + x) as u64, (w * h) as u64);
        }

        self.cursor_drawn = false;
    }

    fn draw_cursor(&mut self) {
        if !self.cursor_visible {
            return;
        }
        // If cursor is currently drawn at some position, erase it first.
        self.erase_cursor();

        let Some((x, y, w, h)) = self.cursor_rect(self.cursor_x, self.cursor_y) else {
            return;
        };

        // Save the pixels under the cursor so we can restore later.
        #[allow(static_mut_refs)]
        unsafe {
            let fb = FRAMEBUFFER.get_mut().unwrap();
            let pitch_pixels = fb.width as usize;
            for row in 0..h {
                let mut row_buf = [0u32; CURSOR_W];
                let bytes = core::slice::from_raw_parts_mut(row_buf.as_mut_ptr() as *mut u8, w * 4);
                let pixel_offset = (y + row) * pitch_pixels + x;
                let _ = fb.read(&mut dummy_blockdev(), pixel_offset, bytes);
                let start = row * w;
                self.cursor_backup[start..start + w].copy_from_slice(&row_buf[..w]);
            }

            // Draw block cursor (full cell) in the current foreground color.
            let color_bytes = convert_color(self.color);
            let mut out_row = vec![0u8; w * 4];
            for px in 0..w {
                let off = px * 4;
                out_row[off..off + 4].copy_from_slice(&color_bytes);
            }
            for row in 0..h {
                let pixel_offset = (y + row) * pitch_pixels + x;
                let _ = fb.write(&mut dummy_blockdev(), pixel_offset, &out_row);
            }
            fb.sync_partial(((y * pitch_pixels) + x) as u64, (w * h) as u64);
        }

        self.cursor_saved_x = self.cursor_x;
        self.cursor_saved_y = self.cursor_y;
        self.cursor_drawn = true;
    }

    pub fn erase_line(&mut self) {
        self.erase_cursor();
        let y = self.cursor_y * 16;
        self.fill_rect(0, y, self.width, 16, self.bg_color);
        self.cursor_x = 0;
        self.draw_cursor();
    }
    pub fn erase_in_line_from_cursor(&mut self) {
        self.erase_cursor();
        let x = self.cursor_x * 8;
        let y = self.cursor_y * 16;
        self.fill_rect(x, y, self.width.saturating_sub(x), 16, self.bg_color);
        self.draw_cursor();
    }
    pub fn erase_in_line_to_cursor(&mut self) {
        self.erase_cursor();
        let x = self.cursor_x * 8;
        let y = self.cursor_y * 16;
        self.fill_rect(0, y, x, 16, self.bg_color);
        self.draw_cursor();
    }
    pub fn erase_display_from_cursor(&mut self) {
        self.erase_cursor();
        // clear from cursor to end of screen
        let x = self.cursor_x * 8;
        let y = self.cursor_y * 16;
        // clear part of current line
        self.fill_rect(x, y, self.width.saturating_sub(x), 16, self.bg_color);
        // clear all lines below
        if y + 16 < self.height {
            self.fill_rect(0, y + 16, self.width, self.height - (y + 16), self.bg_color);
        }
        self.draw_cursor();
    }
    pub fn erase_display_to_cursor(&mut self) {
        self.erase_cursor();
        let x = self.cursor_x * 8;
        let y = self.cursor_y * 16;
        // clear all lines above
        if y > 0 {
            self.fill_rect(0, 0, self.width, y, self.bg_color);
        }
        // clear part of current line up to cursor
        self.fill_rect(0, y, x, 16, self.bg_color);
        self.draw_cursor();
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
        self.erase_cursor();
        apply_console_bg(self.bg_color);
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.draw_cursor();
    }

    /// Write a single character at the current cursor position
    pub fn put_char(&mut self, c: u8) {
        self.erase_cursor();
        match c {
            b'\n' => {
                self.new_line();
                self.draw_cursor();
                return;
            }
            0x08 | 0x7F => {
                self.backspace();
                self.draw_cursor();
                return;
            }
            b'\r' => {
                self.cursor_x = 0;
                self.draw_cursor();
                return;
            }
            _ => {}
        }

        // Keep controls invisible; everything else can use the expanded font table
        let ch = if c.is_ascii_control() { b'?' } else { c };

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
        self.draw_cursor();
    }

    /// Writes a full string (no ANSI yet)
    pub fn write(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.put_char(b);
        }
    }

    pub fn refresh_cursor(&mut self) {
        self.draw_cursor();
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

        // Grab glyph from expanded font table; fall back to '?' if somehow missing
        let glyph_opt = PSF_FONTS
            .get(ascii as usize)
            .or_else(|| PSF_FONTS.get(b'?' as usize));

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
                    fb.write(&mut dummy_blockdev(), pixel_offset, &row_buf).unwrap();
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
