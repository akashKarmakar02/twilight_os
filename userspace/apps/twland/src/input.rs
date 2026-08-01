//! Real input devices for twland.
//!
//! Two non-blocking readers live here, one per kernel input device:
//!
//! * [`Mouse`] — PS/2 mouse packets from `/dev/input/mice` (3-byte packets).
//! * [`Keyboard`] — evdev `struct input_event` records from `/dev/input/event0`.
//!
//! Both are opened with `O_NONBLOCK`: a `read` when no data is ready returns
//! `EAGAIN` instead of blocking, which would freeze the single-threaded
//! compositor loop.  The kernel `read` syscall path for char devices honors
//! `O_NONBLOCK` by calling the driver's `poll()` first and returning `-EAGAIN`
//! when the queue is empty.
//!
//! The kernel side does the heavy lifting: the PS/2 mouse driver
//! (`kernel/.../driver/mouse/ps2.rs`) enqueues 3-byte packets, and the keyboard
//! driver (`kernel/.../driver/keyboard/`) already decodes scancodes to Linux
//! evdev keycodes and packs them into `struct input_event` records.  twland
//! therefore only has to parse the wire formats — no scancode mapping here.

use std::fs::OpenOptions;
use std::io::{self, ErrorKind, Read};
use std::os::unix::fs::OpenOptionsExt;

pub const MICE_PATH: &str = "/dev/input/mice";
pub const EVENT0_PATH: &str = "/dev/input/event0";
const PS2_PACKET_SIZE: usize = 3;
/// `O_NONBLOCK` from `fcntl.h` (Linux).  twland has no `libc` dependency, so
/// the raw constant is used, matching the rest of the crate's FFI style.
const O_NONBLOCK: i32 = 0o4000;
/// Log raw input packets for debugging.
const TWLAND_DEBUG_INPUT: bool = true;

/// Linux input button codes (from `linux/input-event-codes.h`).
pub const BTN_LEFT: u32 = 0x110;

/// `EV_KEY` event type from `linux/input-event-codes.h`.
const EV_KEY: u16 = 0x01;

/// Size of the kernel's `struct input_event` as emitted by `KeyboardDev`:
/// `i64 tv_sec, i64 tv_usec, u16 type, u16 code, i32 value` = 24 bytes.
const INPUT_EVENT_SIZE: usize = 24;

/// One decoded mouse event.
#[derive(Debug, Clone, Copy)]
pub enum MouseEvent {
    /// Relative motion in device pixels. PS/2 Y is positive upward, so it is
    /// inverted during decoding to match screen coordinates (positive down).
    Motion { dx: i32, dy: i32 },
    /// A button state transition.  Only emitted on an edge (press↔release).
    Button { button: u32, pressed: bool },
}

/// A non-blocking reader for `/dev/input/mice`.
///
/// Owned by the `Client` and polled once per compositor iteration.  The fd is
/// kept open for the lifetime of the client.
pub struct Mouse {
    file: std::fs::File,
    /// Previous button state, for edge detection.  Only the left button is
    /// tracked for now; the PS/2 packet also carries right/middle bits.
    left_pressed: bool,
}

impl Mouse {
    /// Open `/dev/input/mice` non-blocking.  Returns `Ok(None)` if the device
    /// is not present (e.g. no mouse), so the compositor can run without one.
    pub fn open() -> io::Result<Option<Self>> {
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(O_NONBLOCK)
            .open(MICE_PATH)
        {
            Ok(f) => f,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };

        Ok(Some(Self {
            file,
            left_pressed: false,
        }))
    }

    /// Drain all available PS/2 packets and return their decoded events.
    ///
    /// Returns an empty vec when no packets are ready (or no mouse).  Each
    /// `read` returns `EAGAIN` when the queue is empty, so this naturally
    /// terminates.
    pub fn poll(&mut self) -> io::Result<Vec<MouseEvent>> {
        let mut events = Vec::new();
        let mut packet = [0u8; PS2_PACKET_SIZE];

        loop {
            match self.file.read(&mut packet) {
                Ok(0) => break, // EOF — shouldn't happen for a char device
                Ok(PS2_PACKET_SIZE) => self.decode(&packet, &mut events),
                Ok(_) => break, // partial packet — wait for the rest
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }

        Ok(events)
    }

    /// Decode one 3-byte PS/2 packet into events, appended to `out`.
    ///
    /// ```text
    /// byte 0: flags
    ///         bit 0 = left button
    ///         bit 1 = right button
    ///         bit 2 = middle button
    ///         bit 3 = always 1 (sync bit)
    ///         bit 4 = x sign (redundant — byte 1 is already two's complement)
    ///         bit 5 = y sign (redundant — byte 2 is already two's complement)
    /// byte 1: dx (signed 8-bit, two's complement)
    /// byte 2: dy (signed 8-bit, two's complement)
    /// ```
    fn decode(&mut self, packet: &[u8; PS2_PACKET_SIZE], out: &mut Vec<MouseEvent>) {
        let flags = packet[0];
        // The data bytes are already in two's complement — cast directly to i8,
        // matching the proven decode in doomgeneric_twilight.c.
        let dx = packet[1] as i8 as i32;
        let dy = -(packet[2] as i8 as i32);

        if dx != 0 || dy != 0 {
            if TWLAND_DEBUG_INPUT {
                eprintln!("twland: mouse dx={dx} dy={dy} flags=0x{flags:02x}");
            }
            out.push(MouseEvent::Motion { dx, dy });
        }

        let left_pressed = flags & 0x01 != 0;
        if left_pressed != self.left_pressed {
            self.left_pressed = left_pressed;
            if TWLAND_DEBUG_INPUT {
                eprintln!("twland: mouse left={left_pressed}");
            }
            out.push(MouseEvent::Button {
                button: BTN_LEFT,
                pressed: left_pressed,
            });
        }
    }
}

/// One decoded keyboard event.
///
/// `keycode` is a Linux evdev keycode (the `KEY_*` constants from
/// `linux/input-event-codes.h`), as produced by the kernel keyboard driver.
#[derive(Debug, Clone, Copy)]
pub struct KeyboardEvent {
    pub keycode: u32,
    pub pressed: bool,
}

/// A non-blocking reader for `/dev/input/event0`.
///
/// The kernel `KeyboardDev` node emits `struct input_event` records (24 bytes
/// each) with `type == EV_KEY`, `code` already an evdev keycode, and
/// `value` of 1 (press), 0 (release) or 2 (repeat).  Repeat is collapsed into
/// press here; clients that want real repeat can be added later.
///
/// Owned by the `Client` and polled once per compositor iteration, alongside
/// [`Mouse`].  The fd is kept open for the lifetime of the client.
pub struct Keyboard {
    file: std::fs::File,
    /// Bytes from a read that did not contain a complete 24-byte record; the
    /// next read appends to this and record framing resumes.  A `read` may
    /// return a non-multiple of `INPUT_EVENT_SIZE` bytes.
    residual: Vec<u8>,
}

impl Keyboard {
    /// Open `/dev/input/event0` non-blocking.  Returns `Ok(None)` if the
    /// device is not present (e.g. no keyboard), so the compositor can run
    /// without one.
    pub fn open() -> io::Result<Option<Self>> {
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(O_NONBLOCK)
            .open(EVENT0_PATH)
        {
            Ok(f) => f,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };

        Ok(Some(Self {
            file,
            residual: Vec::new(),
        }))
    }

    /// Drain all available `input_event` records and return their decoded
    /// events.
    ///
    /// Returns an empty vec when no records are ready (or no keyboard).  Each
    /// `read` returns `EAGAIN` when the queue is empty, so this naturally
    /// terminates.  Non-`EV_KEY` records (sync/misc events the kernel may
    /// emit) are silently dropped — only key transitions are forwarded.
    pub fn poll(&mut self) -> io::Result<Vec<KeyboardEvent>> {
        let mut events = Vec::new();
        let mut buf = [0u8; INPUT_EVENT_SIZE * 8];

        loop {
            match self.file.read(&mut buf) {
                Ok(0) => break, // EOF — shouldn't happen for a char device
                Ok(n) => self.parse_records(&buf[..n], &mut events),
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }

        Ok(events)
    }

    /// Append `n` bytes to the residual buffer, then decode as many complete
    /// 24-byte records as are present.  Leftover bytes stay in `residual`.
    fn parse_records(&mut self, bytes: &[u8], out: &mut Vec<KeyboardEvent>) {
        self.residual.extend_from_slice(bytes);

        let mut start = 0;
        while start + INPUT_EVENT_SIZE <= self.residual.len() {
            let record = &self.residual[start..start + INPUT_EVENT_SIZE];
            start += INPUT_EVENT_SIZE;
            self.decode(record, out);
        }
        self.residual.drain(..start);
    }

    /// Decode one 24-byte `struct input_event` into a key event, if it is a
    /// key transition.
    ///
    /// ```text
    /// bytes  0..8  : tv_sec  (i64, ignored)
    /// bytes  8..16 : tv_usec (i64, ignored)
    /// bytes 16..18 : type    (u16, little-endian — only EV_KEY is forwarded)
    /// bytes 18..20 : code    (u16, little-endian — evdev keycode)
    /// bytes 20..24 : value   (i32, little-endian — 1 press, 0 release, 2 repeat)
    /// ```
    fn decode(&self, record: &[u8], out: &mut Vec<KeyboardEvent>) {
        let type_ = u16::from_le_bytes([record[16], record[17]]);
        if type_ != EV_KEY {
            return;
        }

        let keycode = u16::from_le_bytes([record[18], record[19]]) as u32;
        let value = i32::from_le_bytes([
            record[20],
            record[21],
            record[22],
            record[23],
        ]);
        // value: 1 = press, 0 = release, 2 = autorepeat.  Collapse repeat into
        // press so a held key keeps firing; clients that distinguish can be
        // added later.
        let pressed = match value {
            0 => false,
            1 | 2 => true,
            _ => return,
        };

        if TWLAND_DEBUG_INPUT {
            eprintln!("twland: key code={keycode} pressed={pressed}");
        }
        out.push(KeyboardEvent { keycode, pressed });
    }
}
