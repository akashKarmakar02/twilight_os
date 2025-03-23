use alloc::vec::Vec;
use crate::framebuffer::font::PSF_FONTS;
use crate::framebuffer::{TwilightFrameBuffer, get_framebuffer, clear_screen};
use core::fmt;
use core::fmt::Write;

pub fn print(framebuffer: &'static TwilightFrameBuffer, x: usize, y: usize, color: u32, ascii: u8) {
    let fb_ptr = framebuffer.addr();
    let pitch = framebuffer.pitch();
    if let Some(font_bitmap) = PSF_FONTS.get(ascii as usize - 32) {
        for (row, &bitmap) in font_bitmap.iter().enumerate() {
            for col in 0..8 {
                if (bitmap & (1 << (7 - col))) != 0 {
                    let pixel_offset = ((y + row) * pitch as usize) + ((x + col) * 4);
                    unsafe {
                        fb_ptr
                            .offset(pixel_offset as isize)
                            .cast::<u32>()
                            .write(color);
                    }
                }
            }
        }
    }
}


pub fn clear_char(framebuffer: &'static TwilightFrameBuffer, x: usize, y: usize, color: u32) {
    let fb_ptr = framebuffer.addr();
    let pitch = framebuffer.pitch() as usize;
    let char_width = 8;
    let char_height = 16;

    for row in 0..char_height {
        for col in 0..char_width {
            let pixel_offset = ((y + row) * pitch) + ((x + col - 8) * 4);
            unsafe {
                fb_ptr
                    .offset(pixel_offset as isize)
                    .cast::<u32>()
                    .write(color);
            }
        }
    }
}

pub struct Writer {
    framebuffer: &'static TwilightFrameBuffer,
    pub buffer_content: Vec<Vec<char>>,
    pub column_position: usize,
    pub row_position: usize,
    color: u32,
}

impl Writer {
    pub fn new(color: u32) -> Self {
        Self {
            framebuffer: get_framebuffer(),
            buffer_content: Vec::new(),
            column_position: 0,
            row_position: 0,
            color,
        }
    }

    pub fn write_char(&mut self, c: char) {
        match c {
            '\n' => self.new_line(),
            '\x08' => {
                clear_char(self.framebuffer, self.column_position * 8, self.row_position * 16, 0x282C34u32);
                if self.column_position > 0 {
                    self.column_position -= 1;
                }
            },
            '\t' => {
                self.column_position += 4;
                if self.column_position >= (self.framebuffer.width / 8) as usize{
                    self.new_line();
                }
            },
            _ => {
                if let Some(current_buffer) = self.buffer_content.get_mut(self.row_position) {
                    current_buffer.push(c);
                } else {
                    let mut current_buffer = Vec::new();
                    current_buffer.push(c);
                    self.buffer_content.push(current_buffer);
                }


                print(self.framebuffer, self.column_position * 8, self.row_position * 16, self.color, c as u8);
                self.column_position += 1;
                if self.column_position >= (self.framebuffer.width() / 8) as usize {
                    self.new_line();
                }
            }
        }
    }

    fn new_line(&mut self) {
        self.column_position = 0;
        self.row_position += 1;

        let max_rows = (self.framebuffer.height() / 16) as usize;

        if self.row_position >= max_rows {
            self.buffer_content.remove(0);

            self.buffer_content.push(Vec::new());

            self.redraw_screen();

            self.row_position = max_rows - 1;
        }
    }

    fn redraw_screen(&mut self) {
        clear_screen(false);

        for (row_idx, line) in self.buffer_content.iter().enumerate() {
            for (col_idx, c) in line.iter().enumerate() {
                print(self.framebuffer, col_idx * 8, row_idx * 16, self.color, *c as u8);
            }
        }
    }

    pub fn clear_line(&mut self) {
        let clear_color = 0x282C34u32;

        for i in 0..self.framebuffer.width() / 8 {
            clear_char(self.framebuffer, (i as usize + 1) * 8, self.row_position *  16, clear_color);
        }
    }

    pub fn write_string(&mut self, s: &str) {
        for c in s.chars() {
            self.write_char(c);
        }
    }
}

impl Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}
