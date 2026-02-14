use crate::sys::{
    console::font::get_glyph,
    framebuffer::{FRAMEBUFFER, convert_color, get_framebuffer},
};

use crate::driver::disk::dummy_blockdev;
use crate::sys::fs::vfs::VfsNodeOps;

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
    // UTF-8 decoding state
    utf8_buf: [u8; 4],
    utf8_len: usize,
}

#[derive(Clone, Copy)]
pub struct ScreenChar {
    pub char: char,
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
            utf8_buf: [0; 4],
            utf8_len: 0,
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
            let mut out_row = [0u8; CURSOR_W * 4];
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

        // Chunk configuration
        const CHUNK_PIXELS: usize = 256; // 1024 bytes
        let mut row_buf = [0u8; CHUNK_PIXELS * 4];
        let color_bytes = convert_color(color);

        // Fill buffer with color pattern once
        for i in 0..CHUNK_PIXELS {
            let off = i * 4;
            row_buf[off..off + 4].copy_from_slice(&color_bytes);
        }

        #[allow(static_mut_refs)]
        unsafe {
            let fb = FRAMEBUFFER.get_mut().unwrap();

            for row in 0..h {
                let row_start_pixel = (y + row) * pitch_pixels + x;

                // Write row in chunks
                let mut pixels_written = 0;
                while pixels_written < w {
                    let remaining = w - pixels_written;
                    let chunk = remaining.min(CHUNK_PIXELS);

                    let pixel_offset = row_start_pixel + pixels_written;
                    let byte_count = chunk * 4;

                    fb.write(&mut dummy_blockdev(), pixel_offset, &row_buf[..byte_count])
                        .unwrap();

                    pixels_written += chunk;
                }
            }
            // Sync the filled rect
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
                    char: ' ',
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

    /// Write a single byte to the terminal, decoding UTF-8 on the fly
    pub fn put_byte(&mut self, byte: u8) {
        // If we have no pending bytes, and this is a single-byte char (0xxxxxxx), print it immediately.
        if self.utf8_len == 0 && (byte & 0x80) == 0 {
            self.put_char_utf32(byte as char);
            return;
        }

        // Otherwise, buffer it.
        if self.utf8_len < self.utf8_buf.len() {
            self.utf8_buf[self.utf8_len] = byte;
            self.utf8_len += 1;
        }

        // Check if we have a valid UTF-8 sequence
        if let Ok(s) = core::str::from_utf8(&self.utf8_buf[0..self.utf8_len]) {
            // It's valid! (and complete, because form_utf8 checks for completeness if key is correct?
            // Wait, from_utf8 might fail if incomplete.
            // Actually, from_utf8 succeeds for complete chars.
            // But if we have partial, it fails.
            // We need to know if it's *valid so far* or *complete*.

            // Simple approach: if from_utf8 succeeds, we emit just that char.
            // Since we only buffer up to 4 bytes, and we process one by one.
            if let Some(c) = s.chars().next() {
                self.put_char_utf32(c);
                self.utf8_len = 0;
            }
        } else if self.utf8_len >= 4 {
            // Buffer full and invalid -> garbage. Drop buffer, print replacement?
            // For now, just reset to avoid stuck state.
            // Maybe print replacement char ''?
            self.put_char_utf32('?');
            self.utf8_len = 0;
        } else {
            // Incomplete or invalid, wait for more bytes (unless we determined it's definitely invalid,
            // but std utf8 validation is complex to perform manually without std helper).
            // However, `core::str::from_utf8` errors if incomplete.
            // Example: [0xE2] -> error (part of 3-byte seq).
            // We continue buffering.
        }
    }

    /// Internal: Write a decoded Unicode character
    fn put_char_utf32(&mut self, c: char) {
        self.erase_cursor();
        match c {
            '\n' => {
                self.new_line();
                return;
            }
            '\u{08}' | '\u{7F}' => {
                self.backspace();
                return;
            }
            '\r' => {
                self.cursor_x = 0;
                return;
            }
            _ => {}
        }
        if self.cursor_x >= self.width / Self::CHAR_W {
            self.new_line();
        }

        // Keep controls invisible; everything else is rendered
        let ch = if c.is_control() { '?' } else { c };

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
            self.put_byte(b);
        }
    }

    pub fn refresh_cursor(&mut self) {
        self.draw_cursor();
    }

    /// Move cursor to new line, scroll if needed
    fn new_line(&mut self) {
        self.cursor_x = 0;
        self.cursor_y += 1;
        if (self.cursor_y + 1) * 16 > self.height {
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
        let fg_u32 = if self.reverse {
            self.bg_color
        } else {
            screen_char.color
        };
        let bg_u32 = if self.reverse {
            screen_char.color
        } else {
            self.bg_color
        };

        let glyph_opt = get_glyph(screen_char.char);

        if let Some(font_bitmap) = glyph_opt {
            #[allow(static_mut_refs)]
            unsafe {
                let fb = FRAMEBUFFER.get_mut().unwrap();
                let width = fb.width as usize;

                // Use raw pointers to avoid double mutable borrow of `fb`
                // (one for pixels_mut and one for video_buf)
                let pixels_ptr = fb.pixels_mut().as_mut_ptr();
                let vram_ptr = fb.video_buf.as_mut_ptr();
                let total_len = fb.video_buf.len(); // Assume both buffers same size

                for (row, &bits) in font_bitmap.iter().enumerate() {
                    let row_start = (y + row) * width + x;

                    for col in 0..Self::CHAR_W {
                        let is_fg = (bits & (1 << (7 - col))) != 0;
                        let color = if is_fg { fg_u32 } else { bg_u32 };

                        let idx = row_start + col;
                        if idx < total_len {
                            // Write to RAM backbuffer
                            pixels_ptr.add(idx).write(color);
                            // Write to VRAM (Write Combining)
                            vram_ptr.add(idx).write(color);
                        }
                    }
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

    // Chunk size: 256 pixels = 1024 bytes (same as fill_rect)
    const CHUNK_PIXELS: usize = 256;
    let mut buf = [0u8; CHUNK_PIXELS * 4];
    let color_bytes = convert_color(color);

    // Fill buffer pattern
    for i in 0..CHUNK_PIXELS {
        let off = i * 4;
        buf[off..off + 4].copy_from_slice(&color_bytes);
    }

    #[allow(static_mut_refs)]
    unsafe {
        let fb = FRAMEBUFFER.get_mut().unwrap();

        let mut pixels_written = 0;
        while pixels_written < total_pixels {
            let remaining = total_pixels - pixels_written;
            let chunk = remaining.min(CHUNK_PIXELS);

            fb.write(&mut dummy_blockdev(), pixels_written, &buf[..chunk * 4])
                .unwrap();

            pixels_written += chunk;
        }
    }
}
