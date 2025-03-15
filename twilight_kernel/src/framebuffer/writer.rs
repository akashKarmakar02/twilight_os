use core::fmt;
use core::fmt::{Write};
use crate::framebuffer::font::PSF_FONTS;
use crate::framebuffer::{get_framebuffer, TwilightFrameBuffer};

pub fn print(
    framebuffer: &'static TwilightFrameBuffer,
    x: usize,
    y: usize,
    color: u32,
    ascii: u8,
) {

    let fb_ptr = framebuffer.addr();
    let pitch = framebuffer.pitch();

    let font_bitmap = PSF_FONTS.get(ascii as usize - 32).unwrap();

    for (row, &bitmap) in font_bitmap.iter().enumerate() {
        let rev_bitmap = bitmap.reverse_bits();
        for col in 0..16 {
            if (rev_bitmap & (1 << col)) != 0 {
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

pub struct Writer {
    framebuffer: &'static TwilightFrameBuffer,
    column_position: usize,
    row_position: usize,
    color: u32,
}

impl Writer{
    pub fn new(color: u32) -> Self {
        Self {
            framebuffer: get_framebuffer(),
            column_position: 0,
            row_position: 0,
            color,
        }
    }

    pub fn write_char(&mut self,  c: char) {
        if c == '\n' {
            self.new_line();
        } else {
            print(self.framebuffer, self.column_position * 8, self.row_position * 16, self.color, c as u8);
            self.column_position += 1;
            if self.column_position >= (self.framebuffer.width() / 8) as usize {
                self.new_line();
            }
        }
    }

    fn new_line(&mut self) {
        self.column_position = 0;
        self.row_position += 1;
        if self.row_position >= (self.framebuffer.height() / 16) as usize {
            self.row_position = 0;
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