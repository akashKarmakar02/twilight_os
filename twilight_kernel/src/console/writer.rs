use alloc::{vec, vec::Vec};
use crate::console::font::PSF_FONTS;
use crate::framebuffer::convert_color;
use core::fmt;
use core::fmt::Write;
use crate::fs::Vfs;

static mut WRITER: Option<Writer> = None;

pub fn init_writer() {
    apply_console_bg();

    #[allow(static_mut_refs)]
    unsafe { WRITER = Some(Writer::new(0xBFBFBF)); }
}

pub fn get_writer() -> &'static mut Writer {
    #[allow(static_mut_refs)]
    unsafe { WRITER.as_mut().expect("Writer not initialized") }
}

fn apply_console_bg() {
    let mut fs = crate::fs::FS.get().unwrap().lock();

    if let Ok(inode) = fs.open("/dev/fb0") {
        let width = 1600;
        let height = 900;
        let total_pixels = width * height;

        let mut buf = vec![0u8; total_pixels / 2];
        let color = convert_color(0x101010u32);

        for g in 0..8usize {
            for i in 0..(total_pixels / 8) {
                let start_index = i * 4;
                let end_index = start_index + 4;
                buf[start_index..end_index].clone_from_slice(&color);
            }
            fs.write(inode, g as u64 * (total_pixels / 8) as u64, buf.as_slice()).unwrap();
        }

    }

}


pub fn clear_screen(clear_buffer: bool) {
    apply_console_bg();

    get_writer().row_position = 0;
    get_writer().column_position = 0;
    if clear_buffer {
        get_writer().buffer_content.clear();
    }
}

pub fn print(x: usize, y: usize, color: u32, ascii: u8) {
    let mut fs = crate::fs::FS.get().unwrap().lock();
    let pitch = 1600; // Width of the framebuffer in pixels

    if let Ok(inode) = fs.open("/dev/fb0") {
        if let Some(font_bitmap) = PSF_FONTS.get(ascii as usize - 32) {
            let color_bytes = convert_color(color);
            let background_color = convert_color(0x101010u32);

            for (row, &bitmap) in font_bitmap.iter().enumerate() {
                let mut row_buf = vec![0u8; 8 * 4]; // 8 pixels per row, 4 bytes per pixel
                for col in 0..8 {
                    if (bitmap & (1 << (7 - col))) != 0 {
                        row_buf[col * 4..(col + 1) * 4].clone_from_slice(&color_bytes);
                    } else {
                        row_buf[col * 4..(col + 1) * 4].clone_from_slice(&background_color);
                    }
                }

                let pixel_offset = ((y + row) * pitch) + x; // Offset in pixels
                fs.write(inode, pixel_offset as u64, row_buf.as_slice()).unwrap();
            }
        }
    }
}



pub fn clear_char(x: usize, y: usize, color: u32) {
    let mut fs = crate::fs::FS.get().unwrap().lock();
    let pitch = 1600;
    let char_width = 8;
    let char_height = 16;

    if let Ok(inode) = fs.open("/dev/fb0") {
        let color_bytes = convert_color(color);
        let row_buf = vec![color_bytes; char_width].concat();


        for row in 0..char_height {
            let pixel_offset = ((y + row) * pitch) + (x - 8); // Now in pixels
            fs.write(inode, pixel_offset as u64, row_buf.as_slice()).unwrap();
        }
    }
}

pub struct Writer {
    buffer: Vec<u64>,
    pub buffer_content: Vec<Vec<char>>,
    pub column_position: usize,
    pub row_position: usize,
    color: u32,
    screen_height: u64,
    screen_width: u64,
}

impl Writer {
    pub fn new(color: u32) -> Self {
        Self {
            buffer_content: Vec::new(),
            column_position: 0,
            row_position: 0,
            buffer: Vec::new(),
            color,
            screen_width: 1600,
            screen_height: 900,
        }
    }

    pub fn write_char(&mut self, c: char) {
        if self.buffer.is_empty() {
            self.buffer = vec![0x282C34, self.screen_width * self.screen_height];
        }
        match c {
            '\n' => self.new_line(),
            '\x08' => {
                clear_char( self.column_position * 8, self.row_position * 16, 0x101010u32);
                if self.column_position > 0 {
                    self.column_position -= 1;
                }
            },
            '\t' => {
                self.column_position += 4;
                if self.column_position >= (self.screen_width / 8) as usize{
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


                print(self.column_position * 8, self.row_position * 16, self.color, c as u8);
                self.column_position += 1;
                if self.column_position >= (self.screen_width / 8) as usize {
                    self.new_line();
                }
            }
        }
    }

    fn new_line(&mut self) {
        self.column_position = 0;
        self.row_position += 1;

        let max_rows = (self.screen_height / 16) as usize;

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
                print(col_idx * 8, row_idx * 16, self.color, *c as u8);
            }
        }
    }

    pub fn clear_line(&mut self) {
        let clear_color = 0x101010u32;

        for i in 0..self.screen_width / 8 {
            clear_char( (i as usize + 1) * 8, self.row_position *  16, clear_color);
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




#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::console::writer::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::framebuffer::_print("\n"));
    ($($arg:tt)*) => (print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        get_writer().write_fmt(args).unwrap();
    });
}
