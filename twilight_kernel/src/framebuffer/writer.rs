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

    if let Some(font_bitmap) = PSF_FONTS.get(ascii as usize - 32) {
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
}

pub fn clear_char(framebuffer: &'static TwilightFrameBuffer, x: usize, y: usize, color: u32) {
    let fb_ptr = framebuffer.addr();
    let pitch = framebuffer.pitch() as usize;
    let char_width = 8;  // Assuming 8x16 font size
    let char_height = 16;

    for row in 0..char_height {
        let row_start = ((y + row) * pitch) + (x * 4);
        let row_ptr = unsafe { fb_ptr.offset(row_start as isize).cast::<u32>() };

        // Use a single unsafe block to reduce function call overhead
        unsafe {
            for col in 0..char_width {
                row_ptr.add(col).write(color);
            }
        }
    }
}
pub struct Writer {
    framebuffer: &'static TwilightFrameBuffer,
    column_position: usize,
    row_position: usize,
    color: u32,
    need_flush: bool,
}

impl Writer{
    pub fn new(color: u32) -> Self {
        Self {
            framebuffer: get_framebuffer(),
            column_position: 0,
            row_position: 0,
            color,
            need_flush: false,
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
            self.need_flush = true;
        }

        if self.need_flush {
            self.clear_line(self.row_position);
        }
    }

    pub fn clear_line(&mut self, row: usize) {
        let clear_color = 0x282C34u32;

        for i in 0..self.framebuffer.width()/8 {
            clear_char(self.framebuffer, i as usize * 8, row * 16, clear_color);
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
        // if self.need_flush {
        //     self.clear_line(self.row_position);
        //     self.need_flush = false;  // Reset the flag after clearing
        // }
        self.write_string(s);
        Ok(())
    }
}