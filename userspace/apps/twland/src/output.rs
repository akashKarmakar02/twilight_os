//! `wl_output` event emission for twland.
//!
//! Owns the `wl_output` protocol details: opcodes, mode flags, and the
//! initial event sequence sent when a client binds the output global.  A
//! bound output must receive `geometry`, one or more `mode` events, and a
//! final `done` (at version 2+) before the client considers the output
//! initialized; without `done` conforming clients block waiting for it.
//!
//! This module knows only the output geometry (pixel width/height), not the
//! framebuffer internals, so it stays decoupled from the rendering module.

use std::io;
use std::os::unix::net::UnixStream;

use crate::wire::{push_i32, push_u32, push_wayland_string, send_message};

// wl_output events.
const WL_OUTPUT_GEOMETRY: u16 = 0;
const WL_OUTPUT_MODE: u16 = 1;
const WL_OUTPUT_DONE: u16 = 2;

// wl_output.mode flags.
const MODE_CURRENT: u32 = 1;
const MODE_PREFERRED: u32 = 2;

// wl_output.subpixel: 0 = unknown.
const SUBPIXEL_UNKNOWN: u32 = 0;
// wl_output.transform: 0 = normal (no transform).
const TRANSFORM_NORMAL: u32 = 0;

/// A 60 Hz refresh, in millihertz, the unit `wl_output.mode` expects.  The
/// framebuffer has no real refresh rate; this is a sane default so clients
/// pick a reasonable frame timing.
const DEFAULT_REFRESH_MHZ: u32 = 60_000;

/// Send the initial `wl_output` event sequence for a freshly bound output:
/// `geometry`, the current `mode`, then `done` (only at version 2+, since
/// `wl_output.done` was introduced in version 2 — emitting it to a v1 client
/// is a protocol error).
///
/// `width`/`height` are the output's pixel dimensions.  Physical dimensions
/// and manufacturer/model are unknown for the framebuffer, so they are sent
/// as zero/empty — clients treat that as "unspecified".
pub fn send_initial_events(
    stream: &mut UnixStream,
    output_id: u32,
    version: u32,
    width: i32,
    height: i32,
) -> io::Result<()> {
    send_geometry(stream, output_id)?;
    send_mode(stream, output_id, MODE_CURRENT | MODE_PREFERRED, width, height)?;
    if version >= 2 {
        send_done(stream, output_id)?;
    }
    Ok(())
}

fn send_geometry(stream: &mut UnixStream, output_id: u32) -> io::Result<()> {
    // geometry(x, y, physical_width, physical_height, subpixel, make, model, transform).
    // Physical dimensions and make/model are unknown for the framebuffer, so
    // they are sent as zero/empty; clients treat that as "unspecified".  The
    // pixel width/height belong in the mode event, not here.
    let mut payload = Vec::new();
    push_i32(&mut payload, 0); // logical x origin
    push_i32(&mut payload, 0); // logical y origin
    push_i32(&mut payload, 0); // physical width (mm) — unknown
    push_i32(&mut payload, 0); // physical height (mm) — unknown
    push_u32(&mut payload, SUBPIXEL_UNKNOWN);
    push_wayland_string(&mut payload, ""); // make — unknown
    push_wayland_string(&mut payload, ""); // model — unknown
    push_u32(&mut payload, TRANSFORM_NORMAL);
    send_message(stream, output_id, WL_OUTPUT_GEOMETRY, &payload)
}

fn send_mode(
    stream: &mut UnixStream,
    output_id: u32,
    flags: u32,
    width: i32,
    height: i32,
) -> io::Result<()> {
    // mode(flags, width, height, refresh) — refresh is in mHz.
    let mut payload = Vec::new();
    push_u32(&mut payload, flags);
    push_i32(&mut payload, width);
    push_i32(&mut payload, height);
    push_u32(&mut payload, DEFAULT_REFRESH_MHZ);
    send_message(stream, output_id, WL_OUTPUT_MODE, &payload)
}

fn send_done(stream: &mut UnixStream, output_id: u32) -> io::Result<()> {
    send_message(stream, output_id, WL_OUTPUT_DONE, &[])
}
