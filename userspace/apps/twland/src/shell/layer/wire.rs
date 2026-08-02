use std::io;
use std::os::unix::net::UnixStream;

use crate::wire::{push_u32, read_i32, read_u32, read_wayland_string, send_message};

pub const INTERFACE: &str = "zwlr_layer_shell_v1";
pub const VERSION: u32 = 4;

pub const GET_LAYER_SURFACE: u16 = 0;
pub const DESTROY_LAYER_SHELL: u16 = 1;

pub const SET_SIZE: u16 = 0;
pub const SET_ANCHOR: u16 = 1;
pub const SET_EXCLUSIVE_ZONE: u16 = 2;
pub const SET_MARGIN: u16 = 3;
pub const SET_KEYBOARD_INTERACTIVITY: u16 = 4;
pub const GET_POPUP: u16 = 5;
pub const ACK_CONFIGURE: u16 = 6;
pub const DESTROY_LAYER_SURFACE: u16 = 7;
pub const SET_LAYER: u16 = 8;

const CONFIGURE: u16 = 0;

#[derive(Debug)]
pub enum ShellRequest {
    GetLayerSurface {
        id: u32,
        surface_id: u32,
        output_id: Option<u32>,
        layer: u32,
        namespace: String,
    },
    Destroy,
    Unknown,
}

#[derive(Debug)]
pub enum SurfaceRequest {
    SetSize {
        width: u32,
        height: u32,
    },
    SetAnchor(u32),
    SetExclusiveZone(i32),
    SetMargin {
        top: i32,
        right: i32,
        bottom: i32,
        left: i32,
    },
    SetKeyboardInteractivity(u32),
    GetPopup,
    AckConfigure(u32),
    Destroy,
    SetLayer(u32),
    Unknown,
}

pub fn parse_shell_request(opcode: u16, payload: &[u8]) -> io::Result<ShellRequest> {
    match opcode {
        GET_LAYER_SURFACE => {
            let id = read_u32(payload, 0)?;
            let surface_id = read_u32(payload, 4)?;
            let output_id = read_u32(payload, 8)?;
            let layer = read_u32(payload, 12)?;
            let (namespace, _) = read_wayland_string(payload, 16)?;
            Ok(ShellRequest::GetLayerSurface {
                id,
                surface_id,
                output_id: (output_id != 0).then_some(output_id),
                layer,
                namespace,
            })
        }
        DESTROY_LAYER_SHELL => Ok(ShellRequest::Destroy),
        _ => Ok(ShellRequest::Unknown),
    }
}

pub fn parse_surface_request(opcode: u16, payload: &[u8]) -> io::Result<SurfaceRequest> {
    match opcode {
        SET_SIZE => Ok(SurfaceRequest::SetSize {
            width: read_u32(payload, 0)?,
            height: read_u32(payload, 4)?,
        }),
        SET_ANCHOR => Ok(SurfaceRequest::SetAnchor(read_u32(payload, 0)?)),
        SET_EXCLUSIVE_ZONE => Ok(SurfaceRequest::SetExclusiveZone(read_i32(payload, 0)?)),
        SET_MARGIN => Ok(SurfaceRequest::SetMargin {
            top: read_i32(payload, 0)?,
            right: read_i32(payload, 4)?,
            bottom: read_i32(payload, 8)?,
            left: read_i32(payload, 12)?,
        }),
        SET_KEYBOARD_INTERACTIVITY => Ok(SurfaceRequest::SetKeyboardInteractivity(read_u32(
            payload, 0,
        )?)),
        GET_POPUP => Ok(SurfaceRequest::GetPopup),
        ACK_CONFIGURE => Ok(SurfaceRequest::AckConfigure(read_u32(payload, 0)?)),
        DESTROY_LAYER_SURFACE => Ok(SurfaceRequest::Destroy),
        SET_LAYER => Ok(SurfaceRequest::SetLayer(read_u32(payload, 0)?)),
        _ => Ok(SurfaceRequest::Unknown),
    }
}

pub fn send_configure(
    stream: &mut UnixStream,
    layer_surface_id: u32,
    serial: u32,
    width: u32,
    height: u32,
) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, serial);
    push_u32(&mut payload, width);
    push_u32(&mut payload, height);
    send_message(stream, layer_surface_id, CONFIGURE, &payload)
}
