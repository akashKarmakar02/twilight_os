use crate::sys::console::font::PSF_FONTS;
use crate::sys::framebuffer::{FRAMEBUFFER, convert_color, get_framebuffer};
use crate::sys::fs::VfsNode;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::{vec, vec::Vec};
use core::fmt;
use core::fmt::Write;

#[derive(Copy, Clone)]
enum AnsiState {
    Normal,
    Esc,
    Csi,
}

static mut WRITER: Option<Writer> = None;

#[derive(Clone)]
pub struct ScreenChar {
    pub char: u8,
    pub color: u32,
}

// Initialize the kernel shell
pub fn init_writer() {
    apply_console_bg();

    #[allow(static_mut_refs)]
    unsafe {
        WRITER = Some(Writer::new(0x141A21));
    }
}

// global function to get the writer in a safe mode
pub fn get_writer() -> &'static mut Writer {
    #[allow(static_mut_refs)]
    unsafe {
        WRITER.as_mut().expect("Writer not initialized")
    }
}

// apply background color to the kernel shell using framebuffer
fn apply_console_bg() {
    // using fixed size (because we don't have ioctl)
    let width = get_framebuffer().width as usize;
    let height = get_framebuffer().height as usize;
    let total_pixels = width * height;

    let mut buf = vec![0u8; total_pixels * 4];
    let color = convert_color(0x101010u32);

    for i in 0..total_pixels {
        let start_index = i * 4;
        let end_index = start_index + 4;
        buf[start_index..end_index].clone_from_slice(&color);
    }
    #[allow(static_mut_refs)]
    unsafe {
        let fb = FRAMEBUFFER.get_mut().unwrap();
        fb.write(0, buf.as_slice()).unwrap();
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

pub fn print(x: usize, y: usize, screen_char: ScreenChar) {
    let pitch = get_framebuffer().width as usize; // Width of the framebuffer in pixels

    let color = screen_char.color;
    let ascii = screen_char.char;

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

            #[allow(static_mut_refs)]
            unsafe {
                let fb = FRAMEBUFFER.get_mut().unwrap();
                fb.write(pixel_offset as u64, row_buf.as_slice()).unwrap();
            }
        }
    }
}

pub fn clear_char(x: usize, y: usize, color: u32) {
    let pitch = get_framebuffer().width as usize;
    let char_width = 8;
    let char_height = 16;

    let color_bytes = convert_color(color);
    let row_buf = vec![color_bytes; char_width].concat();

    for row in 0..char_height {
        let pixel_offset = ((y + row) * pitch) + (x - 8); // Now in pixels

        #[allow(static_mut_refs)]
        unsafe {
            let fb = FRAMEBUFFER.get_mut().unwrap();
            fb.write(pixel_offset as u64, row_buf.as_slice()).unwrap();
        }
    }
}

pub struct Writer {
    buffer: Vec<u64>,
    pub buffer_content: VecDeque<Vec<ScreenChar>>,
    pub column_position: usize,
    pub row_position: usize,
    color: u32,
    screen_height: u64,
    screen_width: u64,
    cursor_visible: bool,
    cursor_timer: usize,
    ansi_state: AnsiState,
    ansi_buf: String,
}

impl Writer {
    pub fn new(color: u32) -> Self {
        Self {
            buffer_content: VecDeque::new(),
            column_position: 0,
            row_position: 0,
            buffer: Vec::new(),
            color,
            screen_width: get_framebuffer().width,
            screen_height: get_framebuffer().height,
            cursor_visible: true,
            cursor_timer: 0,
            ansi_state: AnsiState::Normal,
            ansi_buf: String::new(),
        }
    }

    pub fn write_char(&mut self, c: char) {
        if self.buffer.is_empty() {
            self.buffer = vec![0x282C34, self.screen_width * self.screen_height];
        }
        match c {
            '\n' => self.new_line(),
            '\x08' => {
                clear_char(
                    self.column_position * 8,
                    self.row_position * 16,
                    0x101010u32,
                );
                self.clear_cursor();
                if let Some(current_buffer) = self.buffer_content.get_mut(self.row_position) {
                    current_buffer.pop();
                }
                if self.column_position > 0 {
                    self.column_position -= 1;
                }
            }
            '\r' => {
                self.clear_line();
                self.column_position = 0;
                if let Some(current_buffer) = self.buffer_content.get_mut(self.row_position) {
                    current_buffer.pop();
                }
            }
            '\t' => {
                self.column_position += 4;
                if self.column_position >= (self.screen_width / 8) as usize {
                    self.new_line();
                }
            }
            _ => {
                if let Some(current_buffer) = self.buffer_content.get_mut(self.row_position) {
                    current_buffer.push(ScreenChar {
                        char: c as u8,
                        color: self.color,
                    });
                } else {
                    let mut current_buffer = Vec::new();
                    current_buffer.push(ScreenChar {
                        char: c as u8,
                        color: self.color,
                    });
                    self.buffer_content.push_back(current_buffer);
                }

                let screen_char = ScreenChar {
                    char: c as u8,
                    color: self.color,
                };

                print(
                    self.column_position * 8,
                    self.row_position * 16,
                    screen_char,
                );
                self.column_position += 1;
                if self.column_position >= (self.screen_width / 8) as usize {
                    self.new_line();
                }
            }
        }

        self.cursor_visible = true;
        self.cursor_timer = 0;
        self.draw_cursor();
    }

    fn new_line(&mut self) {
        if self.column_position < (self.screen_width / 8) as usize {
            self.clear_cursor();
        }
        self.column_position = 0;
        self.row_position += 1;

        let max_rows = (self.screen_height / 16) as usize;

        if self.row_position >= max_rows {
            self.buffer_content.pop_front();

            self.buffer_content.push_back(Vec::new());

            self.redraw_screen();

            self.row_position = max_rows - 1;
        }
    }

    fn redraw_screen(&mut self) {
        clear_screen(false);

        for (row_idx, line) in self.buffer_content.iter().enumerate() {
            for (col_idx, screen_char) in line.iter().enumerate() {
                print(col_idx * 8, row_idx * 16, screen_char.clone());
            }
        }
    }
    fn draw_cursor(&mut self) {
        if self.cursor_visible {
            let x = self.column_position * 8;
            let y = self.row_position * 16;
            let color = 0xFFFFFFu32; // Cursor color (white)
            let color_bytes = convert_color(color);

            let pitch = get_framebuffer().width as usize; // Framebuffer width in pixels
            let char_width = 8;
            let char_height = 16;

            // Create a buffer for one row (8 pixels wide)
            let row_buf = vec![color_bytes; char_width].concat();

            for row in 0..char_height {
                let pixel_offset = ((y + row) * pitch) + x;

                #[allow(static_mut_refs)]
                unsafe {
                    let fb = FRAMEBUFFER.get_mut().unwrap();
                    fb.write(pixel_offset as u64, row_buf.as_slice()).unwrap();
                }
            }
        }
    }

    pub fn tick(&mut self) {
        self.cursor_timer += 1;
        if self.cursor_timer >= 30 {
            // tune blink speed
            self.cursor_timer = 0;

            // Toggle visibility
            if self.cursor_visible {
                self.clear_cursor();
            } else {
                self.draw_cursor();
            }

            self.cursor_visible = !self.cursor_visible;
        }
    }

    fn clear_cursor(&mut self) {
        let x = self.column_position * 8;
        let y = self.row_position * 16;
        let color = 0x101010u32; // Cursor color (white)
        let color_bytes = convert_color(color);

        let pitch = get_framebuffer().width as usize; // Framebuffer width in pixels
        let char_width = 8;
        let char_height = 16;

        // Create a buffer for one row (8 pixels wide)
        let row_buf = vec![color_bytes; char_width].concat();

        for row in 0..char_height {
            let pixel_offset = ((y + row) * pitch) + x;

            #[allow(static_mut_refs)]
            unsafe {
                let fb = FRAMEBUFFER.get_mut().unwrap();
                fb.write(pixel_offset as u64, row_buf.as_slice()).unwrap();
            }
        }
    }

    pub fn clear_line(&mut self) {
        let clear_color = 0x101010u32;

        for i in 0..self.screen_width / 8 {
            clear_char((i as usize + 1) * 8, self.row_position * 16, clear_color);
        }
    }

    fn feed_char(&mut self, c: char) {
        match self.ansi_state {
            AnsiState::Normal => {
                if c == '\x1b' {
                    self.ansi_state = AnsiState::Esc;
                } else {
                    self.write_char(c);
                }
            }
            AnsiState::Esc => {
                if c == '[' {
                    self.ansi_buf.clear();
                    self.ansi_state = AnsiState::Csi;
                } else {
                    // Not a CSI sequence; print ESC literally and this char
                    self.write_char('\x1b');
                    self.write_char(c);
                    self.ansi_state = AnsiState::Normal;
                }
            }
            AnsiState::Csi => {
                match c {
                    // collect params for SGR
                    '0'..='9' | ';' => self.ansi_buf.push(c),
                    'm' => {
                        // SGR end
                        if self.ansi_buf.is_empty() {
                            self.parse_ansi_code("0");
                        } else {
                            self.parse_ansi_code(self.ansi_buf.clone().as_str());
                        }
                        self.ansi_buf.clear();
                        self.ansi_state = AnsiState::Normal;
                    }
                    _ => {
                        // unsupported CSI; ignore it and return to normal
                        self.ansi_buf.clear();
                        self.ansi_state = AnsiState::Normal;
                    }
                }
            }
        }
    }

    // Replace write_string with a streaming version
    pub fn write_string(&mut self, s: &str) {
        for ch in s.chars() {
            self.feed_char(ch);
        }
    }

    // (optional) broaden reset handling:
    fn parse_ansi_code(&mut self, code: &str) {
        let parts: Vec<u8> = code.split(';').filter_map(|s| s.parse().ok()).collect();
        if parts.is_empty() {
            self.color = 0xBFBFBF;
            return;
        } // treat empty as reset

        for code in parts {
            match code {
                0 | 39 => self.color = 0xBFBFBF, // reset/default fg
                30 => self.color = 0x000000,
                31 => self.color = 0xAA0000,
                32 => self.color = 0x00AA00,
                33 => self.color = 0xAA5500,
                34 => self.color = 0x0000AA,
                35 => self.color = 0xAA00AA,
                36 => self.color = 0x00AAAA,
                37 => self.color = 0xAAAAAA,
                90 => self.color = 0x555555,
                91 => self.color = 0xFF5555,
                92 => self.color = 0x68CB5F,
                93 => self.color = 0xFFFF55,
                94 => self.color = 0x5555FF,
                95 => self.color = 0xFF55FF,
                96 => self.color = 0x55FFFF,
                97 => self.color = 0xFFFFFF,
                // ignore attributes like 1 (bold), 4 (underline) for now
                _ => {}
            }
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
    ($($arg:tt)*) => ($crate::sys::console::writer::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::sys::framebuffer::_print("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        get_writer().write_fmt(args).unwrap();
    });
}
