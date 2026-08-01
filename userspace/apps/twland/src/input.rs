//! Real mouse input for twland.
//!
//! Reads PS/2 mouse packets from `/dev/input/mice` and decodes them into
//! relative motion and button events.  The kernel PS/2 driver
//! (`kernel/.../driver/mouse/ps2.rs`) enqueues 3-byte packets; the devfs
//! `MouseDev` node exposes them via `read` (blocking) and `poll`.
//!
//! twland's compositor loop is single-threaded, so the device is opened with
//! `O_NONBLOCK`: a `read` when no packet is ready returns `EAGAIN` instead of
//! blocking, which would freeze the compositor.  The kernel `read` syscall
//! path for char devices honors `O_NONBLOCK` by calling the driver's `poll()`
//! first and returning `-EAGAIN` when empty.

use std::fs::OpenOptions;
use std::io::{self, ErrorKind, Read};
use std::os::unix::fs::OpenOptionsExt;

pub const MICE_PATH: &str = "/dev/input/mice";
const PS2_PACKET_SIZE: usize = 3;
/// `O_NONBLOCK` from `fcntl.h` (Linux).  twland has no `libc` dependency, so
/// the raw constant is used, matching the rest of the crate's FFI style.
const O_NONBLOCK: i32 = 0o4000;

/// Linux input button codes (from `linux/input-event-codes.h`).
pub const BTN_LEFT: u32 = 0x110;

/// One decoded mouse event.
#[derive(Debug, Clone, Copy)]
pub enum MouseEvent {
    /// Relative motion in device pixels.  PS/2 dy is positive downward, which
    /// already matches screen coordinates — no inversion needed.
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
    ///         bit 4 = x sign
    ///         bit 5 = y sign
    ///         bit 6 = x overflow
    ///         bit 7 = y overflow
    /// byte 1: dx (9-bit signed via bit 4)
    /// byte 2: dy (9-bit signed via bit 5)
    /// ```
    fn decode(&mut self, packet: &[u8; PS2_PACKET_SIZE], out: &mut Vec<MouseEvent>) {
        let flags = packet[0];
        let dx = ps2_axis(packet[1], flags & 0x10 != 0);
        let dy = ps2_axis(packet[2], flags & 0x20 != 0);

        if dx != 0 || dy != 0 {
            out.push(MouseEvent::Motion { dx, dy });
        }

        let left_pressed = flags & 0x01 != 0;
        if left_pressed != self.left_pressed {
            self.left_pressed = left_pressed;
            out.push(MouseEvent::Button {
                button: BTN_LEFT,
                pressed: left_pressed,
            });
        }
    }
}

/// Reassemble a 9-bit signed PS/2 axis value from its byte and sign bit.
fn ps2_axis(byte: u8, negative: bool) -> i32 {
    let value = i32::from(byte);
    if negative { value - 256 } else { value }
}
