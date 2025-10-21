use crate::sys::console::framebuffer::FramebufferTerminal;
use crate::sys::console::TTY;
use crate::sys::fs::vfs::{BlockDev, VfsNodeOps};
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::fmt::Write;
use crate::serial_prtinln;

pub struct Tty {
    term: FramebufferTerminal,
    input_buffer: VecDeque<u8>,
    output_buffer: VecDeque<u8>,
    echo: bool,

    ansi_state: AnsiState,
    csi_buf: Vec<u8>,
    sgr_bold: bool,
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum AnsiState { Ground, Esc, Csi }

impl Tty {
    const FLUSH_THRESHOLD: usize = 512;

    pub fn new() -> Self {
        Self {
            term: FramebufferTerminal::new(),
            input_buffer: VecDeque::new(),
            output_buffer: VecDeque::new(),
            echo: true,
            ansi_state: AnsiState::Ground,
            csi_buf: Vec::with_capacity(32),
            sgr_bold: false,
        }
    }

    #[inline]
    fn is_printable_ascii(b: u8) -> bool { (0x20..=0x7E).contains(&b) }

    #[inline]
    fn is_control_forced_flush(b: u8) -> bool { matches!(b, b'\n' | b'\r' | 0x08 | 0x7F) }

    fn write_bytes_ansi(&mut self, data: &[u8]) {
        for &b in data { self.ansi_feed(b); }
        if self.output_buffer.len() >= Self::FLUSH_THRESHOLD {
            self.flush_output();
        }
    }

    fn ansi_feed(&mut self, b: u8) {
        match self.ansi_state {
            AnsiState::Ground => {
                match b {
                    0x1B => {
                        self.ansi_state = AnsiState::Esc;
                        self.csi_buf.clear();
                    }
                    _ if Self::is_control_forced_flush(b) => {
                        self.flush_output();
                        self.term.put_char(b);
                    }
                    _ if Self::is_printable_ascii(b) => {
                        self.output_buffer.push_back(b);
                    }
                    _ => {
                        self.flush_output();
                        self.term.put_char(b);
                    }
                }
            }
            AnsiState::Esc => {
                if b == b'[' {
                    self.ansi_state = AnsiState::Csi;
                    self.csi_buf.clear();
                } else {
                    self.ansi_state = AnsiState::Ground;
                    self.output_buffer.push_back(0x1B);
                    self.output_buffer.push_back(b);
                }
            }
            AnsiState::Csi => {
                if (b'@'..=b'~').contains(&b) {
                    self.csi_buf.push(b);
                    self.handle_csi_final();
                    self.ansi_state = AnsiState::Ground;
                } else {
                    if self.csi_buf.len() < 64 { self.csi_buf.push(b); }
                }
            }
        }
    }

    fn handle_csi_final(&mut self) {
        let final_byte = *self.csi_buf.last().unwrap_or(&b'\0');
        let params_str = core::str::from_utf8(&self.csi_buf[..self.csi_buf.len().saturating_sub(1)])
            .unwrap_or("");

        let params = String::from(params_str);

        match final_byte {
            b'm' => {
                self.flush_output();
                self.apply_sgr(&params);
            }
            b'J' => {
                // Erase in Display (ED)
                // 0: from cursor to end, 1: from start to cursor, 2: entire screen
                let param = if params.is_empty() { "0" } else { params.as_str() };
                match param {
                    "2" => {
                        self.flush_output();
                        self.term.clear(); // use your FramebufferTerminal::clear()
                        self.term.cursor_x = 0;
                        self.term.cursor_y = 0;
                    }
                    _ => { /* TODO: partial clears later */ }
                }
            }
            b'H' | b'f' => {
                self.flush_output();
                self.term.cursor_x = 0;
                self.term.cursor_y = 0;
            }

            _ => {
                // For unimplemented CSI, ignore silently.
            }
        }
    }

    fn apply_sgr(&mut self, params: &str) {
        let mut it = if params.is_empty() { "0".split(';') } else { params.split(';') };

        while let Some(p) = it.next() {
            let code = if p.is_empty() { 0 } else { p.parse::<i32>().unwrap_or(-1) };

            match code {
                0 => {
                    self.sgr_bold = false;
                    self.term.color = DEFAULT_FG;
                    self.term.bg_color = DEFAULT_BG;
                }
                1 => { self.sgr_bold = true; }
                22 => { self.sgr_bold = false; }

                30..=37 => {
                    let idx = (code - 30) as u8;
                    self.term.color = ansi16_color(idx, self.sgr_bold);
                }
                90..=97 => {
                    let idx = (code - 90 + 8) as u8;
                    self.term.color = ansi16_color(idx, false);
                }
                39 => { self.term.color = DEFAULT_FG; }

                40..=47 => {
                    let idx = (code - 40) as u8;
                    self.term.bg_color = ansi16_color(idx, false);
                }
                100..=107 => {
                    let idx = (code - 100 + 8) as u8;
                    self.term.bg_color = ansi16_color(idx, false);
                }
                49 => { self.term.bg_color = DEFAULT_BG; }

                38 => {
                    if let Some(mode) = it.next() {
                        match mode {
                            "5" => {
                                if let Some(n) = it.next() {
                                    if let Ok(v) = n.parse::<u16>() {
                                        self.term.color = xterm_256_to_rgb(v as u8);
                                    }
                                }
                            }
                            "2" => {
                                let (r,g,b) = (it.next(), it.next(), it.next());
                                if let (Some(r), Some(g), Some(b)) = (r,g,b) {
                                    if let (Ok(r), Ok(g), Ok(b)) = (r.parse::<u8>(), g.parse::<u8>(), b.parse::<u8>()) {
                                        self.term.color = rgb(r,g,b);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }

                48 => {
                    if let Some(mode) = it.next() {
                        match mode {
                            "5" => {
                                if let Some(n) = it.next() {
                                    if let Ok(v) = n.parse::<u16>() {
                                        self.term.bg_color = xterm_256_to_rgb(v as u8);
                                    }
                                }
                            }
                            "2" => {
                                let (r,g,b) = (it.next(), it.next(), it.next());
                                if let (Some(r), Some(g), Some(b)) = (r,g,b) {
                                    if let (Ok(r), Ok(g), Ok(b)) = (r.parse::<u8>(), g.parse::<u8>(), b.parse::<u8>()) {
                                        self.term.bg_color = rgb(r,g,b);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }

                3 | 4 | 5 | 7 | 27 => {
                    // italic/underline/blink/invert: ignore visuals for now
                }

                _ => { /* ignore unknown SGR */ }
            }
        }
    }

    fn flush_output(&mut self) {
        if self.output_buffer.is_empty() { return; }

        let mut tmp: Vec<u8> = Vec::with_capacity(self.output_buffer.len());
        while let Some(b) = self.output_buffer.pop_front() { tmp.push(b); }

        let mut i = 0;
        while i < tmp.len() {
            let start = i;
            while i < tmp.len() && Self::is_printable_ascii(tmp[i]) { i += 1; }
            if i > start {
                let s = unsafe { core::str::from_utf8_unchecked(&tmp[start..i]) };
                self.term.write(s);
            }

            if i < tmp.len() {
                let b = tmp[i];
                match b {
                    b'\n' | b'\r' | 0x08 | 0x7F => self.term.put_char(b),
                    _ => self.term.put_char(b),
                }
                i += 1;
            }
        }
    }
}

const DEFAULT_FG: u32 = 0xFFFFFF;
const DEFAULT_BG: u32 = 0x101010;

#[inline] fn rgb(r:u8,g:u8,b:u8)->u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}
fn ansi16_color(idx: u8, bold_for_basic: bool) -> u32 {
    const P: [u32; 16] = [
        0x000000, 0xAA0000, 0x00AA00, 0xAA5500, 0x0000AA, 0xAA00AA, 0x00AAAA, 0xAAAAAA,
        0x555555, 0xFF5555, 0x55FF55, 0xFFFF55, 0x5555FF, 0xFF55FF, 0x55FFFF, 0xFFFFFF,
    ];
    let mut i = idx.min(15);
    if bold_for_basic && i < 8 { i += 8; }
    P[i as usize]
}

fn xterm_256_to_rgb(n: u8) -> u32 {
    match n {
        0..=15 => ansi16_color(n, false),
        16..=231 => {
            let c = n - 16;
            let r = c / 36;
            let g = (c % 36) / 6;
            let b = c % 6;
            let map = |v: u8| -> u8 { [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff][v as usize] };
            rgb(map(r), map(g), map(b))
        }
        232..=255 => {
            let v = 8 + (n - 232) * 10;
            rgb(v, v, v)
        }
    }
}

impl VfsNodeOps for Tty {
    fn read(&self, _device: &mut BlockDev, _lba: usize) -> Result<Vec<u8>, ()> {
        Err(())
    }

    fn write(&mut self, _device: &mut BlockDev, _lba: usize, data: &[u8]) -> Result<(), ()> {
        self.write_bytes_ansi(data);
        Ok(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(!self.input_buffer.is_empty())
    }
}

impl Write for Tty {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_bytes_ansi(s.as_bytes());
        Ok(())
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::sys::console::tty::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::sys::console::tty::_print(format_args!("\n")));
    ($($arg:tt)*) => ($crate::sys::console::tty::_print(format_args!("{}\n", format_args!($($arg)*))));
}

pub fn get_tty() -> &'static mut Tty {
    #[allow(static_mut_refs)]
    unsafe { TTY.get_mut().unwrap() }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        let tty = get_tty();
        tty.write_fmt(args).ok();
        tty.flush_output();
    });
}
