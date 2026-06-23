use std::collections::BTreeMap;
use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;

const WAYLAND_SOCKET: &str = "/run/user/0/wayland-0";

const WL_DISPLAY_GET_REGISTRY: u16 = 1;
const WL_REGISTRY_GLOBAL: u16 = 0;
const WL_REGISTRY_BIND: u16 = 0;
const WL_SHM_FORMAT: u16 = 0;

const REGISTRY_ID: u32 = 2;
const SHM_ID: u32 = 3;

const WL_SHM_FORMAT_ARGB8888: u32 = 0;
const WL_SHM_FORMAT_XRGB8888: u32 = 1;

#[derive(Debug, Clone, Copy)]
struct WaylandHeader {
    object_id: u32,
    opcode: u16,
    size: u16,
}

#[derive(Debug, Clone)]
struct Global {
    name: u32,
    version: u32,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("twland_test_client: FAIL: {error}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let mut stream = UnixStream::connect(WAYLAND_SOCKET)?;
    println!("twland_test_client: connected");

    send_get_registry(&mut stream)?;
    let globals = read_registry_globals(&mut stream)?;

    for required in ["wl_compositor", "wl_shm", "wl_seat", "wl_output"] {
        if !globals.contains_key(required) {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("missing global {required}"),
            ));
        }
    }

    let shm = globals
        .get("wl_shm")
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "missing wl_shm global"))?;
    send_registry_bind(&mut stream, shm, "wl_shm", SHM_ID)?;

    let formats = read_shm_formats(&mut stream)?;
    if !formats.contains(&WL_SHM_FORMAT_ARGB8888) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "missing ARGB8888 shm format",
        ));
    }
    if !formats.contains(&WL_SHM_FORMAT_XRGB8888) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "missing XRGB8888 shm format",
        ));
    }

    println!("twland_test_client: PASS");
    Ok(())
}

fn send_get_registry(stream: &mut UnixStream) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, REGISTRY_ID);
    send_message(stream, 1, WL_DISPLAY_GET_REGISTRY, &payload)
}

fn read_registry_globals(stream: &mut UnixStream) -> io::Result<BTreeMap<String, Global>> {
    let mut globals = BTreeMap::new();

    for _ in 0..4 {
        let (header, payload) = read_message(stream)?;
        if header.object_id != REGISTRY_ID || header.opcode != WL_REGISTRY_GLOBAL {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!(
                    "expected registry.global, got object={} opcode={}",
                    header.object_id, header.opcode
                ),
            ));
        }

        let name = read_u32(&payload, 0)?;
        let (interface, offset) = read_wayland_string(&payload, 4)?;
        let version = read_u32(&payload, offset)?;
        println!("twland_test_client: global {interface} version={version}");
        globals.insert(interface, Global { name, version });
    }

    Ok(globals)
}

fn send_registry_bind(
    stream: &mut UnixStream,
    global: &Global,
    interface: &str,
    new_id: u32,
) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, global.name);
    push_wayland_string(&mut payload, interface);
    push_u32(&mut payload, global.version);
    push_u32(&mut payload, new_id);
    send_message(stream, REGISTRY_ID, WL_REGISTRY_BIND, &payload)
}

fn read_shm_formats(stream: &mut UnixStream) -> io::Result<Vec<u32>> {
    let mut formats = Vec::new();

    for _ in 0..2 {
        let (header, payload) = read_message(stream)?;
        if header.object_id != SHM_ID || header.opcode != WL_SHM_FORMAT {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!(
                    "expected wl_shm.format, got object={} opcode={}",
                    header.object_id, header.opcode
                ),
            ));
        }

        let format = read_u32(&payload, 0)?;
        match format {
            WL_SHM_FORMAT_ARGB8888 => println!("twland_test_client: shm format ARGB8888"),
            WL_SHM_FORMAT_XRGB8888 => println!("twland_test_client: shm format XRGB8888"),
            other => println!("twland_test_client: shm format {other}"),
        }
        formats.push(format);
    }

    Ok(formats)
}

fn read_message(stream: &mut UnixStream) -> io::Result<(WaylandHeader, Vec<u8>)> {
    let mut raw = [0_u8; 8];
    stream.read_exact(&mut raw)?;

    let object_id = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let packed = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
    let header = WaylandHeader {
        object_id,
        opcode: (packed & 0xffff) as u16,
        size: (packed >> 16) as u16,
    };
    if header.size < 8 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("invalid message size {}", header.size),
        ));
    }

    let mut payload = vec![0_u8; usize::from(header.size - 8)];
    stream.read_exact(&mut payload)?;
    Ok((header, payload))
}

fn send_message(
    stream: &mut UnixStream,
    object_id: u32,
    opcode: u16,
    payload: &[u8],
) -> io::Result<()> {
    let size = 8_usize
        .checked_add(payload.len())
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "message too large"))?;
    if size > u16::MAX as usize {
        return Err(io::Error::new(ErrorKind::InvalidInput, "message too large"));
    }

    let mut message = Vec::with_capacity(size);
    push_u32(&mut message, object_id);
    push_u32(&mut message, ((size as u32) << 16) | u32::from(opcode));
    message.extend_from_slice(payload);
    stream.write_all(&message)
}

fn read_u32(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| io::Error::new(ErrorKind::UnexpectedEof, "missing u32 argument"))?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn read_wayland_string(bytes: &[u8], offset: usize) -> io::Result<(String, usize)> {
    let length = read_u32(bytes, offset)? as usize;
    if length == 0 {
        return Ok((String::new(), align4(offset + 4)));
    }

    let start = offset + 4;
    let end = start
        .checked_add(length)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "string length overflow"))?;
    let raw = bytes
        .get(start..end)
        .ok_or_else(|| io::Error::new(ErrorKind::UnexpectedEof, "truncated string argument"))?;
    if raw.last() != Some(&0) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "string argument is not NUL-terminated",
        ));
    }

    let value = std::str::from_utf8(&raw[..raw.len() - 1])
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "string argument is not UTF-8"))?
        .to_string();
    Ok((value, align4(end)))
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_wayland_string(bytes: &mut Vec<u8>, value: &str) {
    let length = value.len() + 1;
    push_u32(bytes, length as u32);
    bytes.extend_from_slice(value.as_bytes());
    bytes.push(0);
    while bytes.len() % 4 != 0 {
        bytes.push(0);
    }
}

fn align4(value: usize) -> usize {
    (value + 3) & !3
}
