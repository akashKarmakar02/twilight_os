#![allow(dead_code)]

use crate::arch::x86_64::halt;
use crate::driver::keyboard::KeyboardListener;
use crate::sys::console::framebuffer::FramebufferTerminal;
use crate::sys::console::TTY;
use crate::sys::fs::vfs::{BlockDev, VfsNodeOps};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::fmt::Write;
use core::{fmt, mem};
use spin::Mutex;
use twilight_common::syscall::types::EFAULT;

// x86_64/musl
const IOCTL_TIOCGWINSZ: u64 = 0x5413;
const IOCTL_TCGETS: u64 = 0x5401;
const IOCTL_TCSETS: u64 = 0x5402;
const IOCTL_TCSETSW: u64 = 0x5403;
const IOCTL_TCSETSF: u64 = 0x5404;

// Order matches Linux <termios.h> (x86_64/musl)
pub const VINTR: usize = 0;
pub const VQUIT: usize = 1;
pub const VERASE: usize = 2;
pub const VKILL: usize = 3;
pub const VEOF: usize = 4;
pub const VTIME: usize = 5;
pub const VMIN: usize = 6;
pub const VSWTC: usize = 7;
pub const VSTART: usize = 8;
pub const VSTOP: usize = 9;
pub const VSUSP: usize = 10;
pub const VEOL: usize = 11;
pub const VREPRINT: usize = 12;
pub const VDISCARD: usize = 13;
pub const VWERASE: usize = 14;
pub const VLNEXT: usize = 15;
pub const VEOL2: usize = 16;

const LFLAG_ECHO: u32 = 0x0008; // ECHO
const LFLAG_ICANON: u32 = 0x0002; // ICANON
const LFLAG_ISIG: u32 = 0x0001; // ISIG
const LFLAG_IEXTEN: u32 = 0x8000; // IEXTEN

const IFLAG_ICRNL: u32 = 0x0100; // ICRNL
const IFLAG_IXON: u32 = 0x0400; // IXON

const OFLAG_OPOST: u32 = 0x0001; // OPOST
const OFLAG_ONLCR: u32 = 0x0004; // ONLCR

#[repr(C)]
struct Winsize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

impl Winsize {
    fn new(rows: u16, cols: u16, xpixels: u16, ypixels: u16) -> Self {
        Self {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: xpixels,
            ws_ypixel: ypixels,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; 32], // NCCS = 32 on Linux x86_64
    pub c_ispeed: u32,  // speed_t
    pub c_ospeed: u32,  // speed_t
}

impl Termios {
    pub fn default() -> Self {
        let mut cc = [0u8; 32];
        cc[VINTR] = 0x03; // ^C
        cc[VQUIT] = 0x1C; // ^\
        cc[VERASE] = 0x7F; // DEL
        cc[VKILL] = 0x15; // ^U
        cc[VEOF] = 0x04; // ^D
        cc[VTIME] = 0; // deciseconds
        cc[VMIN] = 1; // canonical ignores this; raw often sets 1
        cc[VSTART] = 0x11; // ^Q
        cc[VSTOP] = 0x13; // ^S
        cc[VSUSP] = 0x1A; // ^Z
        cc[VEOL] = 0;
        cc[VREPRINT] = 0x12; // ^R
        cc[VDISCARD] = 0x0F; // ^O
        cc[VWERASE] = 0x17; // ^W
        cc[VLNEXT] = 0x16; // ^V
        cc[VEOL2] = 0;

        // Flags: these mimic what your C editor expects before it switches to raw mode
        const ICRNL: u32 = 0x00000100;
        const IXON: u32 = 0x00000400;
        const OPOST: u32 = 0x00000001;
        const ONLCR: u32 = 0x00000004;
        const CREAD: u32 = 0x00000800;
        const CS8: u32 = 0x00000030;
        const ISIG: u32 = 0x00000001;
        const ICANON: u32 = 0x00000002;
        const IEXTEN: u32 = 0x00008000;
        const ECHO: u32 = 0x00000008;

        Termios {
            c_iflag: ICRNL | IXON,
            c_oflag: OPOST | ONLCR,
            c_cflag: CREAD | CS8,
            c_lflag: ISIG | ICANON | IEXTEN | ECHO,
            c_line: 0,
            c_cc: cc,
            c_ispeed: 38400, // any value; many libcs ignore and use c_cflag Bxxxx
            c_ospeed: 38400,
        }
    }
}

pub struct Tty {
    term: FramebufferTerminal,

    // behavior flags derived from termios
    echo: bool,
    icanon: bool,
    isig: bool,
    ixon: bool,
    icrnl: bool,
    o_post: bool,
    onlcr: bool,
    vmin: u8,
    vtime: u8,

    ansi_state: AnsiState,
    read_to_count: Mutex<usize>,
    csi_buf: Vec<u8>,
    sgr_bold: bool,
    termios: Termios,
    output_buffer: Mutex<VecDeque<u8>>,
    input_buffer: Mutex<VecDeque<u8>>,
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum AnsiState {
    Ground,
    Esc,
    Csi,
}

impl Tty {
    const FLUSH_THRESHOLD: usize = 512;

    pub fn new() -> Self {
        let tios = Termios::default(); // your helper that fills cooked defaults
        Self {
            term: FramebufferTerminal::new(),
            output_buffer: Mutex::new(VecDeque::new()),
            input_buffer: Mutex::new(VecDeque::new()),

            echo: true,
            icanon: true,
            isig: true,
            ixon: true,
            icrnl: true,
            o_post: true,
            onlcr: true,
            vmin: tios.c_cc[VMIN],
            vtime: tios.c_cc[VTIME],

            ansi_state: AnsiState::Ground,
            read_to_count: Mutex::new(0),
            csi_buf: Vec::with_capacity(32),
            sgr_bold: false,
            termios: tios,
        }
    }

    pub fn put_input(&mut self, c: u8) {
        if self.is_raw_mode() && c == b'\x08' {
            self.input_buffer.lock().push_back(0x7Fu8);
            // serial_println!("{:?}", self.input_buffer);
        } else {
            self.input_buffer.lock().push_back(c);
        }
    }
    fn is_raw_mode(&self) -> bool {
        (self.termios.c_lflag & (LFLAG_ECHO | LFLAG_ICANON | LFLAG_ISIG | LFLAG_IEXTEN)) == 0
            && (self.termios.c_iflag & (IFLAG_ICRNL | IFLAG_IXON)) == 0
            && (self.termios.c_oflag & OFLAG_OPOST) == 0
    }

    #[inline]
    fn is_printable_ascii(b: u8) -> bool {
        (0x20..=0x7E).contains(&b)
    }

    #[inline]
    fn is_control_forced_flush(b: u8) -> bool {
        matches!(b, b'\n' | b'\r' | 0x08 | 0x7F)
    }

    fn write_bytes_ansi(&mut self, data: &[u8]) {
        for &b in data {
            self.ansi_feed(b);
        }
        if self.output_buffer.lock().len() >= Self::FLUSH_THRESHOLD {
            self.flush_output();
        }
    }

    fn ansi_feed(&mut self, b: u8) {
        match self.ansi_state {
            AnsiState::Ground => match b {
                0x1B => {
                    self.ansi_state = AnsiState::Esc;
                    self.csi_buf.clear();
                }
                _ if Self::is_control_forced_flush(b) => {
                    self.flush_output();
                    self.term.put_char(b);
                }
                _ if Self::is_printable_ascii(b) => {
                    self.output_buffer.lock().push_back(b);
                }
                _ => {
                    self.flush_output();
                    self.term.put_char(b);
                }
            },
            AnsiState::Esc => {
                if b == b'[' {
                    self.ansi_state = AnsiState::Csi;
                    self.csi_buf.clear();
                } else {
                    self.ansi_state = AnsiState::Ground;
                    self.output_buffer.lock().push_back(0x1B);
                    self.output_buffer.lock().push_back(b);
                }
            }
            AnsiState::Csi => {
                if (b'@'..=b'~').contains(&b) {
                    self.csi_buf.push(b);
                    self.handle_csi_final();
                    self.ansi_state = AnsiState::Ground;
                } else {
                    if self.csi_buf.len() < 64 {
                        self.csi_buf.push(b);
                    }
                }
            }
        }
    }

    pub fn move_cursor_left(&mut self) {
        self.term.cursor_x -= 1;
    }

    pub fn move_cursor_right(&mut self) {
        self.term.cursor_x += 1;
    }

    fn handle_csi_final(&mut self) {
        // take ownership of the bytes; self.csi_buf becomes an empty Vec
        let buf = mem::take(&mut self.csi_buf);

        if buf.is_empty() {
            self.ansi_state = AnsiState::Ground;
            return;
        }

        let final_byte = *buf.last().unwrap();
        let raw_params = &buf[..buf.len() - 1]; // params bytes
        let is_private = raw_params.first() == Some(&b'?');
        let params_bytes = if is_private {
            &raw_params[1..]
        } else {
            raw_params
        };

        // If you need a &str, decode from the local `buf` (not from self)
        let params_str = core::str::from_utf8(params_bytes).unwrap_or("");

        // Parse ints without touching self
        let nums: Vec<i32> = if params_str.is_empty() {
            Vec::new()
        } else {
            params_str
                .split(';')
                .map(|s| s.parse::<i32>().unwrap_or(-1))
                .collect()
        };

        // Now it's safe to mutate `self` again
        self.flush_output();

        match final_byte {
            b'm' => self.apply_sgr(params_str),
            b'J' => self.csi_ed(&nums),
            b'K' => self.csi_el(&nums),
            b'H' | b'f' => self.csi_cup(&nums),
            b'h' if is_private => self.csi_dec_private_set(&nums),
            b'l' if is_private => self.csi_dec_private_reset(&nums),
            _ => {}
        }

        self.ansi_state = AnsiState::Ground;
        // `self.csi_buf` remains empty; that's fine for your state machine
    }

    fn csi_el(&mut self, nums: &[i32]) {
        match *nums.get(0).unwrap_or(&2) {
            2 => self.term.erase_line(),                // clear whole line
            1 => self.term.erase_in_line_to_cursor(),   // BOL..cursor
            _ => self.term.erase_in_line_from_cursor(), // cursor..EOL (default 0)
        }
    }

    fn csi_cup(&mut self, nums: &[i32]) {
        // defaults 1;1 if missing/zero/neg
        let row = nums.get(0).copied().unwrap_or(1).max(1) as usize;
        let col = nums.get(1).copied().unwrap_or(1).max(1) as usize;
        let max_rows = (self.term.height / 16).max(1);
        let max_cols = (self.term.width / 8).max(1);
        self.term.cursor_y = row.saturating_sub(1).min(max_rows - 1);
        self.term.cursor_x = col.saturating_sub(1).min(max_cols - 1);
    }

    fn csi_ed(&mut self, nums: &[i32]) {
        match *nums.get(0).unwrap_or(&0) {
            2 => {
                self.term.clear();
                self.term.cursor_x = 0;
                self.term.cursor_y = 0;
            }
            1 => self.term.erase_display_to_cursor(),
            _ => self.term.erase_display_from_cursor(),
        }
    }

    fn csi_dec_private_set(&mut self, nums: &[i32]) {
        for n in nums {
            if *n == 25 {
                self.term.set_cursor_visible(true);
            } // ?25h
        }
    }
    fn csi_dec_private_reset(&mut self, nums: &[i32]) {
        for n in nums {
            if *n == 25 {
                self.term.set_cursor_visible(false);
            } // ?25l
        }
    }

    fn apply_termios(&mut self) {
        // Echo on/off
        self.echo = (self.termios.c_lflag & LFLAG_ECHO) != 0;

        // Cache VMIN / VTIME (used only when ICANON is OFF)
        self.vmin = self.termios.c_cc[VMIN];
        self.vtime = self.termios.c_cc[VTIME];

        // TODO (optional but recommended):
        // - Store booleans for ICANON/ISIG/IEXTEN and handle in read path
        // - Honor ICRNL (map CR->NL on input) and IXON (^S/^Q) in read path
        // - Honor OPOST/ONLCR in write path (you already mostly do raw out)
    }

    fn apply_sgr(&mut self, params: &str) {
        let mut it = if params.is_empty() {
            "0".split(';')
        } else {
            params.split(';')
        };

        while let Some(p) = it.next() {
            let code = if p.is_empty() {
                0
            } else {
                p.parse::<i32>().unwrap_or(-1)
            };

            match code {
                0 => {
                    self.sgr_bold = false;
                    self.term.color = DEFAULT_FG;
                    self.term.bg_color = DEFAULT_BG;
                    self.term.set_reverse(false);
                }
                1 => {
                    self.sgr_bold = true;
                }
                7 => {
                    self.term.set_reverse(true);
                }
                22 => {
                    self.sgr_bold = false;
                }
                27 => {
                    self.term.set_reverse(false);
                }
                30..=37 => {
                    let idx = (code - 30) as u8;
                    self.term.color = ansi16_color(idx, self.sgr_bold);
                }
                90..=97 => {
                    let idx = (code - 90 + 8) as u8;
                    self.term.color = ansi16_color(idx, false);
                }
                39 => {
                    self.term.color = DEFAULT_FG;
                }

                40..=47 => {
                    let idx = (code - 40) as u8;
                    self.term.bg_color = ansi16_color(idx, false);
                }
                100..=107 => {
                    let idx = (code - 100 + 8) as u8;
                    self.term.bg_color = ansi16_color(idx, false);
                }
                49 => {
                    self.term.bg_color = DEFAULT_BG;
                }

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
                                let (r, g, b) = (it.next(), it.next(), it.next());
                                if let (Some(r), Some(g), Some(b)) = (r, g, b) {
                                    if let (Ok(r), Ok(g), Ok(b)) =
                                        (r.parse::<u8>(), g.parse::<u8>(), b.parse::<u8>())
                                    {
                                        self.term.color = rgb(r, g, b);
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
                                let (r, g, b) = (it.next(), it.next(), it.next());
                                if let (Some(r), Some(g), Some(b)) = (r, g, b) {
                                    if let (Ok(r), Ok(g), Ok(b)) =
                                        (r.parse::<u8>(), g.parse::<u8>(), b.parse::<u8>())
                                    {
                                        self.term.bg_color = rgb(r, g, b);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }

                3 | 4 | 5 => {
                    // italic/underline/blink/invert: ignore visuals for now
                }

                _ => {}
            }
        }
    }

    fn flush_output(&mut self) {
        if self.output_buffer.lock().is_empty() {
            return;
        }

        let mut tmp: Vec<u8> = Vec::with_capacity(self.output_buffer.lock().len());
        while let Some(b) = self.output_buffer.lock().pop_front() {
            tmp.push(b);
        }

        let mut i = 0;
        while i < tmp.len() {
            let start = i;
            while i < tmp.len() && Self::is_printable_ascii(tmp[i]) {
                i += 1;
            }
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

#[inline]
fn rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}
fn ansi16_color(idx: u8, bold_for_basic: bool) -> u32 {
    const P: [u32; 16] = [
        0x000000, 0xAA0000, 0x00AA00, 0xAA5500, 0x0000AA, 0xAA00AA, 0x00AAAA, 0xAAAAAA, 0x555555,
        0xFF5555, 0x55FF55, 0xFFFF55, 0x5555FF, 0xFF55FF, 0x55FFFF, 0xFFFFFF,
    ];
    let mut i = idx.min(15);
    if bold_for_basic && i < 8 {
        i += 8;
    }
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

impl KeyboardListener for Tty {
    fn on_key(&self, key: u8, released: bool) {
        if !released {
            self.input_buffer.lock().push_back(key);
        }
    }
}

impl VfsNodeOps for Tty {
    fn read(&self, _device: &mut BlockDev, _lba: usize, buf: &mut [u8]) -> Result<Vec<u8>, ()> {
        let mut i = 0;

        loop {
            let c = loop {
                if self.input_buffer.lock().is_empty() {
                    halt();
                } else {
                    unsafe {
                        break self.input_buffer.lock().pop_front().unwrap_unchecked();
                    };
                }
            };
            buf[i] = c;
            i += 1;
            if !self.is_raw_mode() {
                *self.read_to_count.lock() += 1;
            }

            match c {
                b'\n' => {
                    if !self.is_raw_mode() {
                        *self.read_to_count.lock() = 0;
                    }
                    crate::print!("\n");
                    break;
                }
                0x08 | 0x7F => {
                    if i > 0 {
                        if buf.len() > 1 {
                            i -= 1;
                        }

                        if !self.is_raw_mode() {
                            if *self.read_to_count.lock() > 1 {
                                *self.read_to_count.lock() -= 2;
                                crate::print!("{}", c as char);
                                if buf.len() > 1 {
                                    i -= 1;
                                }
                            } else {
                                *self.read_to_count.lock() -= 1;
                            }
                        }
                    }

                    if i >= buf.len() {
                        break;
                    }
                }
                _ => {
                    if self.echo {
                        crate::print!("{}", c as char);
                    }
                    if i >= buf.len() {
                        break;
                    }
                }
            }
        }

        // *self.read_to_count.lock() = 0;
        let res = Vec::from(&buf[..i]);

        Ok(res)
    }

    fn write(&mut self, _device: &mut BlockDev, _lba: usize, data: &[u8]) -> Result<(), ()> {
        self.write_bytes_ansi(data);
        Ok(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(!self.input_buffer.lock().is_empty())
    }

    fn ioctl(&mut self, _device: &mut BlockDev, cmd: u64, arg: usize) -> Result<i64, ()> {
        match cmd {
            IOCTL_TIOCGWINSZ => {
                let winsize_ptr = arg as *mut Winsize;
                if winsize_ptr.is_null() {
                    return Ok(-(EFAULT as i64));
                }

                unsafe {
                    let winsize = &mut *winsize_ptr;
                    winsize.ws_row = (self.term.height / 16) as u16;
                    winsize.ws_col = (self.term.width / 8) as u16;
                    winsize.ws_xpixel = self.term.width as u16;
                    winsize.ws_ypixel = self.term.height as u16;
                }
            }
            IOCTL_TCGETS => {
                let termios_ptr = arg as *mut Termios;
                if termios_ptr.is_null() {
                    return Ok(-(EFAULT as i64));
                }

                unsafe {
                    *termios_ptr = self.termios;
                }
            }
            IOCTL_TCSETS | IOCTL_TCSETSW | IOCTL_TCSETSF => {
                if arg == 0 {
                    return Ok(-(EFAULT as i64));
                }
                // Copy termios FROM user
                let newt = unsafe { *(arg as *const Termios) };
                // Install and apply
                self.termios = newt;
                self.apply_termios();
            }
            _ => {
                return Ok(0);
            }
        }

        Ok(0)
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
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
    unsafe {
        TTY.get_mut().unwrap()
    }
}

/// Lightweight VFS wrapper that forwards /dev/tty ops to the single global TTY.
pub struct TtyDev;

impl VfsNodeOps for TtyDev {
    fn read(&self, device: &mut BlockDev, lba: usize, buf: &mut [u8]) -> Result<Vec<u8>, ()> {
        let tty = get_tty();
        tty.read(device, lba, buf)
    }

    fn write(&mut self, device: &mut BlockDev, lba: usize, data: &[u8]) -> Result<(), ()> {
        let tty = get_tty();
        tty.write(device, lba, data)
    }

    fn poll(&self, device: &mut BlockDev) -> Result<bool, ()> {
        let tty = get_tty();
        tty.poll(device)
    }

    fn ioctl(&mut self, device: &mut BlockDev, cmd: u64, arg: usize) -> Result<i64, ()> {
        let tty = get_tty();
        tty.ioctl(device, cmd, arg)
    }

    fn unlink(&mut self, device: &mut BlockDev) -> Result<i32, ()> {
        let tty = get_tty();
        tty.unlink(device)
    }
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
