use core::ffi::c_void;
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::ptr;

const RUNTIME_DIR: &str = "/run/user/0";
const WAYLAND_SOCKET: &str = "/run/user/0/wayland-0";
const FB_PATH: &str = "/dev/fb0";

const WL_DISPLAY_SYNC: u16 = 0;
const WL_DISPLAY_GET_REGISTRY: u16 = 1;
const WL_CALLBACK_DONE: u16 = 0;
const WL_REGISTRY_GLOBAL: u16 = 0;
const WL_REGISTRY_BIND: u16 = 0;
const WL_COMPOSITOR_CREATE_SURFACE: u16 = 0;
const WL_SHM_CREATE_POOL: u16 = 0;
const WL_SHM_FORMAT: u16 = 0;
const WL_SHM_POOL_CREATE_BUFFER: u16 = 0;
const WL_SHM_POOL_DESTROY: u16 = 1;
const WL_BUFFER_RELEASE: u16 = 0;
const WL_BUFFER_DESTROY: u16 = 0;
const WL_SURFACE_DESTROY: u16 = 0;
const WL_SURFACE_ATTACH: u16 = 1;
const WL_SURFACE_DAMAGE: u16 = 2;
const WL_SURFACE_COMMIT: u16 = 6;

const WL_SHM_FORMAT_ARGB8888: u32 = 0;
const WL_SHM_FORMAT_XRGB8888: u32 = 1;

const FBIOGET_VSCREENINFO: u64 = 0x4600;
const FBIOGET_FSCREENINFO: u64 = 0x4602;
const FBIOPAN_DISPLAY: u64 = 0x4606;

const PROT_READ: i32 = 0x1;
const PROT_WRITE: i32 = 0x2;
const MAP_SHARED: i32 = 0x01;
const MAP_FAILED: *mut c_void = usize::MAX as *mut c_void;

const SOL_SOCKET: i32 = 1;
const SCM_RIGHTS: i32 = 1;
const MAX_WIRE_CHUNK: usize = 64 * 1024;
const MAX_CONTROL_BYTES: usize = 128;

const GLOBALS: &[Global] = &[
    Global {
        name: 1,
        interface: "wl_compositor",
        version: 4,
        kind: WaylandObjectKind::Compositor,
    },
    Global {
        name: 2,
        interface: "wl_shm",
        version: 1,
        kind: WaylandObjectKind::Shm,
    },
    Global {
        name: 3,
        interface: "wl_seat",
        version: 5,
        kind: WaylandObjectKind::Seat,
    },
    Global {
        name: 4,
        interface: "wl_output",
        version: 3,
        kind: WaylandObjectKind::Output,
    },
];

#[repr(C)]
struct Iovec {
    iov_base: *mut c_void,
    iov_len: usize,
}

#[repr(C)]
struct Msghdr {
    msg_name: *mut c_void,
    msg_namelen: u32,
    msg_iov: *mut Iovec,
    msg_iovlen: i32,
    __pad_iovlen: i32,
    msg_control: *mut c_void,
    msg_controllen: u32,
    __pad_controllen: u32,
    msg_flags: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Cmsghdr {
    cmsg_len: u32,
    __pad_len: i32,
    cmsg_level: i32,
    cmsg_type: i32,
}

#[repr(C)]
#[derive(Default)]
struct FbVarScreenInfo {
    xres: u32,
    yres: u32,
    bits_per_pixel: u32,
    red_offset: u32,
    green_offset: u32,
    blue_offset: u32,
}

#[repr(C)]
#[derive(Default)]
struct FbFixScreenInfo {
    line_length: u32,
    smem_len: u32,
}

unsafe extern "C" {
    fn recvmsg(fd: i32, msg: *mut Msghdr, flags: i32) -> isize;
    fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: isize,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> i32;
    fn close(fd: i32) -> i32;
    fn ioctl(fd: i32, request: u64, ...) -> i32;
}

#[derive(Debug, Clone, Copy)]
struct WaylandHeader {
    object_id: u32,
    opcode: u16,
    size: u16,
}

struct ReceivedMessage {
    header: WaylandHeader,
    payload: Vec<u8>,
    fds: Vec<OwnedFdRaw>,
}

struct Client {
    objects: HashMap<u32, WaylandObject>,
    pools: HashMap<u32, ShmPoolState>,
    buffers: HashMap<u32, BufferState>,
    surfaces: HashMap<u32, SurfaceState>,
    queued_messages: VecDeque<ReceivedMessage>,
    next_serial: u32,
}

#[derive(Debug, Clone)]
struct WaylandObject {
    kind: WaylandObjectKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaylandObjectKind {
    Display,
    Registry,
    Callback,
    Compositor,
    Shm,
    ShmPool,
    Buffer,
    Surface,
    Seat,
    Output,
}

struct ShmPoolState {
    fd: i32,
    size: usize,
    mapped_addr: *mut u8,
    destroyed: bool,
}

#[derive(Debug, Clone)]
struct BufferState {
    pool_id: u32,
    offset: usize,
    width: i32,
    height: i32,
    stride: i32,
    format: u32,
}

#[derive(Debug, Clone)]
struct SurfaceState {
    attached_buffer: Option<u32>,
    pending_buffer: Option<u32>,
    damage: Option<Rect>,
    x: i32,
    y: i32,
    attach_x: i32,
    attach_y: i32,
}

#[derive(Debug, Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Debug, Clone, Copy)]
struct Global {
    name: u32,
    interface: &'static str,
    version: u32,
    kind: WaylandObjectKind,
}

struct SoftwareOutput {
    _file: File,
    width: usize,
    height: usize,
    stride: usize,
    map_bytes: usize,
    pixels: *mut u8,
}

#[derive(Debug)]
struct OwnedFdRaw {
    fd: i32,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("twland: fatal: {error}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    println!("twland: starting");
    println!("twland: runtime dir {RUNTIME_DIR}");
    ensure_dir("/run")?;
    ensure_dir("/run/user")?;
    ensure_dir(RUNTIME_DIR)?;

    let mut output = SoftwareOutput::open()?;
    output.clear(0xff101018)?;
    println!("twland: framebuffer cleared");

    match fs::remove_file(WAYLAND_SOCKET) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let listener = UnixListener::bind(WAYLAND_SOCKET)?;
    println!("twland: listening on {WAYLAND_SOCKET}");

    for accepted in listener.incoming() {
        match accepted {
            Ok(stream) => {
                println!("twland: client connected");
                if let Err(error) = handle_client(stream, &mut output) {
                    eprintln!("twland: client disconnected: {error}");
                } else {
                    println!("twland: client disconnected");
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => eprintln!("twland: accept failed: {error}"),
        }
    }

    Ok(())
}

fn ensure_dir(path: &str) -> io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            if Path::new(path).is_dir() {
                Ok(())
            } else {
                Err(error)
            }
        }
        Err(error) => Err(error),
    }
}

fn handle_client(mut stream: UnixStream, output: &mut SoftwareOutput) -> io::Result<()> {
    let mut client = Client::new();

    loop {
        let message = match recv_wayland_message(&mut client, &mut stream) {
            Ok(Some(message)) => message,
            Ok(None) => return Ok(()),
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset | ErrorKind::BrokenPipe
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
        };

        println!(
            "twland: request object={} opcode={} size={}",
            message.header.object_id, message.header.opcode, message.header.size
        );

        dispatch_request(&mut client, output, &mut stream, message)?;
    }
}

impl Client {
    fn new() -> Self {
        let mut objects = HashMap::new();
        objects.insert(
            1,
            WaylandObject {
                kind: WaylandObjectKind::Display,
            },
        );

        Self {
            objects,
            pools: HashMap::new(),
            buffers: HashMap::new(),
            surfaces: HashMap::new(),
            queued_messages: VecDeque::new(),
            next_serial: 1,
        }
    }

    fn insert_object(&mut self, id: u32, kind: WaylandObjectKind) -> io::Result<()> {
        if id == 0 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "object id 0 cannot be created",
            ));
        }
        self.objects.insert(id, WaylandObject { kind });
        Ok(())
    }

    fn next_serial(&mut self) -> u32 {
        let serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1).max(1);
        serial
    }

    fn cleanup_destroyed_pools(&mut self) {
        let live_pool_ids = self
            .buffers
            .values()
            .map(|buffer| buffer.pool_id)
            .collect::<Vec<_>>();
        self.pools
            .retain(|id, pool| !pool.destroyed || live_pool_ids.contains(id));
    }
}

fn dispatch_request(
    client: &mut Client,
    output: &mut SoftwareOutput,
    stream: &mut UnixStream,
    message: ReceivedMessage,
) -> io::Result<()> {
    let Some(object) = client.objects.get(&message.header.object_id) else {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("unknown object {}", message.header.object_id),
        ));
    };

    match (object.kind, message.header.opcode) {
        (WaylandObjectKind::Display, WL_DISPLAY_SYNC) => {
            handle_display_sync(client, stream, &message.payload)
        }
        (WaylandObjectKind::Display, WL_DISPLAY_GET_REGISTRY) => {
            handle_get_registry(client, stream, &message.payload)
        }
        (WaylandObjectKind::Registry, WL_REGISTRY_BIND) => {
            handle_registry_bind(client, stream, message.header.object_id, &message.payload)
        }
        (WaylandObjectKind::Compositor, WL_COMPOSITOR_CREATE_SURFACE) => {
            handle_compositor_create_surface(client, &message.payload)
        }
        (WaylandObjectKind::Shm, WL_SHM_CREATE_POOL) => {
            handle_shm_create_pool(client, message.payload, message.fds)
        }
        (WaylandObjectKind::ShmPool, WL_SHM_POOL_CREATE_BUFFER) => {
            handle_shm_pool_create_buffer(client, message.header.object_id, &message.payload)
        }
        (WaylandObjectKind::ShmPool, WL_SHM_POOL_DESTROY) => {
            handle_shm_pool_destroy(client, message.header.object_id);
            Ok(())
        }
        (WaylandObjectKind::Buffer, WL_BUFFER_DESTROY) => {
            handle_buffer_destroy(client, message.header.object_id);
            Ok(())
        }
        (WaylandObjectKind::Surface, WL_SURFACE_DESTROY) => {
            handle_surface_destroy(client, message.header.object_id);
            Ok(())
        }
        (WaylandObjectKind::Surface, WL_SURFACE_ATTACH) => {
            handle_surface_attach(client, message.header.object_id, &message.payload)
        }
        (WaylandObjectKind::Surface, WL_SURFACE_DAMAGE) => {
            handle_surface_damage(client, message.header.object_id, &message.payload)
        }
        (WaylandObjectKind::Surface, WL_SURFACE_COMMIT) => {
            handle_surface_commit(client, output, stream, message.header.object_id)
        }
        (kind, opcode) => Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("unsupported request kind={kind:?} opcode={opcode}"),
        )),
    }
}

fn handle_display_sync(
    client: &mut Client,
    stream: &mut UnixStream,
    payload: &[u8],
) -> io::Result<()> {
    let callback_id = read_u32(payload, 0)?;
    println!("twland: wl_display.sync callback_id={callback_id}");

    client.insert_object(callback_id, WaylandObjectKind::Callback)?;

    let mut response = Vec::new();
    push_u32(&mut response, client.next_serial());
    send_message(stream, callback_id, WL_CALLBACK_DONE, &response)?;

    client.objects.remove(&callback_id);
    Ok(())
}

fn handle_get_registry(
    client: &mut Client,
    stream: &mut UnixStream,
    payload: &[u8],
) -> io::Result<()> {
    let registry_id = read_u32(payload, 0)?;
    println!("twland: wl_display.get_registry new_id={registry_id}");

    client.insert_object(registry_id, WaylandObjectKind::Registry)?;
    for global in GLOBALS {
        send_registry_global(stream, registry_id, global)?;
    }

    Ok(())
}

fn handle_registry_bind(
    client: &mut Client,
    stream: &mut UnixStream,
    registry_id: u32,
    payload: &[u8],
) -> io::Result<()> {
    let global_name = read_u32(payload, 0)?;
    let (interface, next_offset) = read_wayland_string(payload, 4)?;
    let version = read_u32(payload, next_offset)?;
    let new_id = read_u32(payload, next_offset + 4)?;

    println!(
        "twland: bind global={global_name} interface={interface} version={version} new_id={new_id}"
    );

    let Some(global) = GLOBALS
        .iter()
        .find(|global| global.name == global_name && global.interface == interface)
    else {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!(
                "registry {registry_id} cannot bind unknown global={global_name} interface={interface}"
            ),
        ));
    };

    client.insert_object(new_id, global.kind)?;

    if global.kind == WaylandObjectKind::Shm {
        send_shm_format(stream, new_id, WL_SHM_FORMAT_ARGB8888)?;
        send_shm_format(stream, new_id, WL_SHM_FORMAT_XRGB8888)?;
        println!("twland: wl_shm bound, sent supported formats");
    }

    Ok(())
}

fn handle_compositor_create_surface(client: &mut Client, payload: &[u8]) -> io::Result<()> {
    let surface_id = read_u32(payload, 0)?;
    client.insert_object(surface_id, WaylandObjectKind::Surface)?;
    client.surfaces.insert(
        surface_id,
        SurfaceState {
            attached_buffer: None,
            pending_buffer: None,
            damage: None,
            x: 40,
            y: 40,
            attach_x: 0,
            attach_y: 0,
        },
    );
    println!("twland: wl_compositor.create_surface id={surface_id}");
    Ok(())
}

fn handle_shm_create_pool(
    client: &mut Client,
    payload: Vec<u8>,
    mut fds: Vec<OwnedFdRaw>,
) -> io::Result<()> {
    let pool_id = read_u32(&payload, 0)?;
    let size = read_i32(&payload, 4)?;
    if size <= 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("invalid shm pool size {size}"),
        ));
    }

    if fds.is_empty() {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "wl_shm.create_pool requires one SCM_RIGHTS fd",
        ));
    }

    let fd = fds.remove(0).into_raw();
    let size = size as usize;
    let mapped = unsafe_mmap_shm(fd, size)?;

    client.insert_object(pool_id, WaylandObjectKind::ShmPool)?;
    client.pools.insert(
        pool_id,
        ShmPoolState {
            fd,
            size,
            mapped_addr: mapped,
            destroyed: false,
        },
    );

    println!("twland: wl_shm.create_pool id={pool_id} fd={fd} size={size}");
    Ok(())
}

fn handle_shm_pool_create_buffer(
    client: &mut Client,
    pool_id: u32,
    payload: &[u8],
) -> io::Result<()> {
    let buffer_id = read_u32(payload, 0)?;
    let offset = read_i32(payload, 4)?;
    let width = read_i32(payload, 8)?;
    let height = read_i32(payload, 12)?;
    let stride = read_i32(payload, 16)?;
    let format = read_u32(payload, 20)?;

    if offset < 0 || width <= 0 || height <= 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "invalid wl_shm_pool.create_buffer geometry",
        ));
    }
    if format != WL_SHM_FORMAT_ARGB8888 && format != WL_SHM_FORMAT_XRGB8888 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("unsupported shm format {format}"),
        ));
    }

    let min_stride = width
        .checked_mul(4)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "buffer stride overflow"))?;
    if stride < min_stride {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("invalid stride {stride} for width {width}"),
        ));
    }

    let pool = client
        .pools
        .get(&pool_id)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown shm pool"))?;
    let end = (offset as usize)
        .checked_add(
            (stride as usize)
                .checked_mul(height as usize)
                .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "buffer size overflow"))?,
        )
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "buffer end overflow"))?;
    if end > pool.size {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("buffer exceeds shm pool: end={end} pool={}", pool.size),
        ));
    }

    client.insert_object(buffer_id, WaylandObjectKind::Buffer)?;
    client.buffers.insert(
        buffer_id,
        BufferState {
            pool_id,
            offset: offset as usize,
            width,
            height,
            stride,
            format,
        },
    );

    println!(
        "twland: wl_shm_pool.create_buffer id={buffer_id} size={width}x{height} stride={stride} format={format}"
    );
    Ok(())
}

fn handle_shm_pool_destroy(client: &mut Client, pool_id: u32) {
    client.objects.remove(&pool_id);
    if let Some(pool) = client.pools.get_mut(&pool_id) {
        pool.destroyed = true;
    }
    client.cleanup_destroyed_pools();
    println!("twland: wl_shm_pool.destroy id={pool_id}");
}

fn handle_buffer_destroy(client: &mut Client, buffer_id: u32) {
    client.objects.remove(&buffer_id);
    client.buffers.remove(&buffer_id);
    client.cleanup_destroyed_pools();
    println!("twland: wl_buffer.destroy id={buffer_id}");
}

fn handle_surface_destroy(client: &mut Client, surface_id: u32) {
    client.objects.remove(&surface_id);
    client.surfaces.remove(&surface_id);
    println!("twland: wl_surface.destroy id={surface_id}");
}

fn handle_surface_attach(client: &mut Client, surface_id: u32, payload: &[u8]) -> io::Result<()> {
    let buffer_id = read_u32(payload, 0)?;
    let x = read_i32(payload, 4)?;
    let y = read_i32(payload, 8)?;

    if buffer_id != 0 && !client.buffers.contains_key(&buffer_id) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("surface attached unknown buffer {buffer_id}"),
        ));
    }

    let surface = client
        .surfaces
        .get_mut(&surface_id)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown surface"))?;
    surface.pending_buffer = (buffer_id != 0).then_some(buffer_id);
    surface.attach_x = x;
    surface.attach_y = y;

    println!("twland: wl_surface.attach surface={surface_id} buffer={buffer_id} x={x} y={y}");
    Ok(())
}

fn handle_surface_damage(client: &mut Client, surface_id: u32, payload: &[u8]) -> io::Result<()> {
    let rect = Rect {
        x: read_i32(payload, 0)?,
        y: read_i32(payload, 4)?,
        width: read_i32(payload, 8)?,
        height: read_i32(payload, 12)?,
    };
    if rect.width <= 0 || rect.height <= 0 {
        return Ok(());
    }

    let surface = client
        .surfaces
        .get_mut(&surface_id)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown surface"))?;
    surface.damage = Some(surface.damage.map_or(rect, |old| old.union(rect)));
    Ok(())
}

fn handle_surface_commit(
    client: &mut Client,
    output: &mut SoftwareOutput,
    stream: &mut UnixStream,
    surface_id: u32,
) -> io::Result<()> {
    let mut surface = client
        .surfaces
        .get(&surface_id)
        .cloned()
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown surface"))?;

    if surface.pending_buffer.is_some() {
        surface.attached_buffer = surface.pending_buffer;
        surface.pending_buffer = None;
    }

    let Some(buffer_id) = surface.attached_buffer else {
        client.surfaces.insert(surface_id, surface);
        println!("twland: wl_surface.commit surface={surface_id} no-buffer");
        return Ok(());
    };
    let buffer = client.buffers.get(&buffer_id).cloned().ok_or_else(|| {
        io::Error::new(ErrorKind::InvalidData, "commit references unknown buffer")
    })?;
    let pool = client
        .pools
        .get(&buffer.pool_id)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "commit references unknown pool"))?;
    let damage = surface.damage.take().unwrap_or(Rect {
        x: 0,
        y: 0,
        width: buffer.width,
        height: buffer.height,
    });

    // TODO: Real Wayland mapping must require an xdg-shell role. This temporary
    // Twilight debug path blits roleless surfaces directly to the framebuffer.
    let blit = blit_shm_buffer_to_output(output, pool, &buffer, &surface, damage)?;
    output.sync()?;
    send_message(stream, buffer_id, WL_BUFFER_RELEASE, &[])?;

    client.surfaces.insert(surface_id, surface);
    println!(
        "twland: wl_surface.commit surface={surface_id} buffer={buffer_id} blit={}x{}",
        blit.width, blit.height
    );
    Ok(())
}

fn recv_wayland_message(
    client: &mut Client,
    stream: &mut UnixStream,
) -> io::Result<Option<ReceivedMessage>> {
    if let Some(message) = client.queued_messages.pop_front() {
        return Ok(Some(message));
    }

    let mut data = vec![0_u8; MAX_WIRE_CHUNK];
    let mut control = [0_u8; MAX_CONTROL_BYTES];
    let mut iov = Iovec {
        iov_base: data.as_mut_ptr().cast(),
        iov_len: data.len(),
    };
    let mut msg = Msghdr {
        msg_name: ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        __pad_iovlen: 0,
        msg_control: control.as_mut_ptr().cast(),
        msg_controllen: control.len() as u32,
        __pad_controllen: 0,
        msg_flags: 0,
    };

    // SAFETY: `msg` points to valid writable iovec/control buffers for the
    // duration of the call, and the fd comes from a live UnixStream.
    let received = unsafe { recvmsg(stream.as_raw_fd(), &mut msg, 0) };
    if received < 0 {
        return Err(io::Error::last_os_error());
    }
    if received == 0 {
        return Ok(None);
    }

    let fds = parse_received_fds(&control, msg.msg_controllen as usize);
    parse_wire_chunk(&data[..received as usize], fds, &mut client.queued_messages)?;
    Ok(client.queued_messages.pop_front())
}

fn parse_wire_chunk(
    bytes: &[u8],
    mut fds: Vec<OwnedFdRaw>,
    queue: &mut VecDeque<ReceivedMessage>,
) -> io::Result<()> {
    let mut offset = 0usize;
    let mut first = true;

    while offset < bytes.len() {
        if offset + 8 > bytes.len() {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "truncated Wayland header",
            ));
        }

        let header = parse_header(&bytes[offset..offset + 8]);
        if header.size < 8 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("invalid message size {}", header.size),
            ));
        }

        let end = offset + usize::from(header.size);
        if end > bytes.len() {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "truncated Wayland payload",
            ));
        }

        let message_fds = if first {
            first = false;
            std::mem::take(&mut fds)
        } else {
            Vec::new()
        };
        queue.push_back(ReceivedMessage {
            header,
            payload: bytes[offset + 8..end].to_vec(),
            fds: message_fds,
        });
        offset = end;
    }

    Ok(())
}

fn parse_received_fds(control: &[u8], controllen: usize) -> Vec<OwnedFdRaw> {
    let mut fds = Vec::new();
    if controllen < size_of::<Cmsghdr>() || controllen > control.len() {
        return fds;
    }

    let mut offset = 0usize;
    while offset + size_of::<Cmsghdr>() <= controllen {
        // SAFETY: Bounds above guarantee enough bytes for an unaligned cmsghdr.
        let cmsg = unsafe { ptr::read_unaligned(control.as_ptr().add(offset).cast::<Cmsghdr>()) };
        let cmsg_len = cmsg.cmsg_len as usize;
        if cmsg_len < size_of::<Cmsghdr>() || offset + cmsg_len > controllen {
            break;
        }

        if cmsg.cmsg_level == SOL_SOCKET && cmsg.cmsg_type == SCM_RIGHTS {
            let data_start = offset + size_of::<Cmsghdr>();
            let data_len = cmsg_len - size_of::<Cmsghdr>();
            for chunk in control[data_start..data_start + data_len].chunks_exact(size_of::<i32>()) {
                let fd = i32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                if fd >= 0 {
                    fds.push(OwnedFdRaw { fd });
                }
            }
        }

        let next = cmsg_align(cmsg_len);
        if next <= offset {
            break;
        }
        offset += next;
    }

    fds
}

fn parse_header(raw: &[u8]) -> WaylandHeader {
    let object_id = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let packed = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
    WaylandHeader {
        object_id,
        opcode: (packed & 0xffff) as u16,
        size: (packed >> 16) as u16,
    }
}

fn send_registry_global(
    stream: &mut UnixStream,
    registry_id: u32,
    global: &Global,
) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, global.name);
    push_wayland_string(&mut payload, global.interface);
    push_u32(&mut payload, global.version);
    send_message(stream, registry_id, WL_REGISTRY_GLOBAL, &payload)
}

fn send_shm_format(stream: &mut UnixStream, shm_id: u32, format: u32) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, format);
    send_message(stream, shm_id, WL_SHM_FORMAT, &payload)
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

fn blit_shm_buffer_to_output(
    output: &mut SoftwareOutput,
    pool: &ShmPoolState,
    buffer: &BufferState,
    surface: &SurfaceState,
    damage: Rect,
) -> io::Result<Rect> {
    if buffer.format != WL_SHM_FORMAT_ARGB8888 && buffer.format != WL_SHM_FORMAT_XRGB8888 {
        return Err(io::Error::new(ErrorKind::InvalidData, "unsupported format"));
    }

    let mut src_x0 = damage.x.max(0);
    let mut src_y0 = damage.y.max(0);
    let mut src_x1 = damage.x.saturating_add(damage.width).min(buffer.width);
    let mut src_y1 = damage.y.saturating_add(damage.height).min(buffer.height);
    if src_x1 <= src_x0 || src_y1 <= src_y0 {
        return Ok(Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        });
    }

    let mut dst_x0 = surface.x + surface.attach_x + src_x0;
    let mut dst_y0 = surface.y + surface.attach_y + src_y0;
    if dst_x0 < 0 {
        src_x0 -= dst_x0;
        dst_x0 = 0;
    }
    if dst_y0 < 0 {
        src_y0 -= dst_y0;
        dst_y0 = 0;
    }
    if dst_x0 >= output.width as i32 || dst_y0 >= output.height as i32 {
        return Ok(Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        });
    }

    let max_width = (output.width as i32 - dst_x0).max(0);
    let max_height = (output.height as i32 - dst_y0).max(0);
    src_x1 = src_x1.min(src_x0 + max_width);
    src_y1 = src_y1.min(src_y0 + max_height);
    let width = src_x1 - src_x0;
    let height = src_y1 - src_y0;
    if width <= 0 || height <= 0 {
        return Ok(Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        });
    }

    let src_x0 = src_x0 as usize;
    let src_y0 = src_y0 as usize;
    let dst_x0 = dst_x0 as usize;
    let dst_y0 = dst_y0 as usize;
    let width = width as usize;
    let height = height as usize;
    let stride = buffer.stride as usize;

    for row in 0..height {
        let src_offset = buffer
            .offset
            .checked_add((src_y0 + row) * stride)
            .and_then(|offset| offset.checked_add(src_x0 * 4))
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "source offset overflow"))?;
        let dst_offset = (dst_y0 + row)
            .checked_mul(output.stride)
            .and_then(|offset| offset.checked_add(dst_x0 * 4))
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "destination offset overflow"))?;
        let row_bytes = width * 4;
        if src_offset + row_bytes > pool.size || dst_offset + row_bytes > output.map_bytes {
            return Err(io::Error::new(ErrorKind::InvalidData, "blit out of bounds"));
        }

        // SAFETY: Bounds checks above prove both source and destination row
        // ranges are within their mmap regions. The framebuffer and memfd
        // mappings are distinct, non-overlapping mappings.
        unsafe {
            ptr::copy_nonoverlapping(
                pool.mapped_addr.add(src_offset),
                output.pixels.add(dst_offset),
                row_bytes,
            );
        }
    }

    println!("twland: blit {}x{} at {},{}", width, height, dst_x0, dst_y0);
    Ok(Rect {
        x: dst_x0 as i32,
        y: dst_y0 as i32,
        width: width as i32,
        height: height as i32,
    })
}

fn unsafe_mmap_shm(fd: i32, size: usize) -> io::Result<*mut u8> {
    // SAFETY: `fd` is an owned memfd received via SCM_RIGHTS. The kernel maps
    // `size` bytes shared/read-only for this process. The returned pointer is
    // checked against MAP_FAILED and then owned by ShmPoolState until munmap.
    let mapped = unsafe { mmap(ptr::null_mut(), size, PROT_READ, MAP_SHARED, fd, 0) };
    if mapped == MAP_FAILED {
        Err(io::Error::last_os_error())
    } else {
        Ok(mapped.cast())
    }
}

impl SoftwareOutput {
    fn open() -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(FB_PATH)?;
        let fd = file.as_raw_fd();
        let mut var = FbVarScreenInfo::default();
        let mut fix = FbFixScreenInfo::default();

        // SAFETY: ioctl writes into valid framebuffer info structs for a live
        // /dev/fb0 fd. Constants match Twilight's framebuffer device ABI.
        let var_result = unsafe { ioctl(fd, FBIOGET_VSCREENINFO, &mut var) };
        if var_result < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: Same as above for fixed screen information.
        let fix_result = unsafe { ioctl(fd, FBIOGET_FSCREENINFO, &mut fix) };
        if fix_result < 0 {
            return Err(io::Error::last_os_error());
        }
        if var.xres == 0 || var.yres == 0 || var.bits_per_pixel != 32 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!(
                    "unsupported framebuffer mode {}x{} {}bpp",
                    var.xres, var.yres, var.bits_per_pixel
                ),
            ));
        }

        let map_bytes = fix.smem_len as usize;
        let expected = (var.yres as usize)
            .checked_mul(fix.line_length as usize)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "framebuffer size overflow"))?;
        if map_bytes < expected {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "framebuffer mapping is smaller than geometry",
            ));
        }

        // SAFETY: The framebuffer fd supports MAP_SHARED mmap. Pointer is
        // checked against MAP_FAILED and owned by SoftwareOutput until Drop.
        let pixels = unsafe {
            mmap(
                ptr::null_mut(),
                map_bytes,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };
        if pixels == MAP_FAILED {
            return Err(io::Error::last_os_error());
        }

        println!(
            "twland: framebuffer {}x{} stride={} bytes",
            var.xres, var.yres, fix.line_length
        );

        Ok(Self {
            _file: file,
            width: var.xres as usize,
            height: var.yres as usize,
            stride: fix.line_length as usize,
            map_bytes,
            pixels: pixels.cast(),
        })
    }

    fn clear(&mut self, color: u32) -> io::Result<()> {
        for y in 0..self.height {
            let row_offset = y
                .checked_mul(self.stride)
                .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "row offset overflow"))?;
            // SAFETY: row_offset is inside the framebuffer mapping and each row
            // writes exactly `width * 4` bytes, which is <= stride by fb init.
            let row = unsafe {
                std::slice::from_raw_parts_mut(
                    self.pixels.add(row_offset).cast::<u32>(),
                    self.width,
                )
            };
            row.fill(color);
        }
        self.sync()
    }

    fn sync(&mut self) -> io::Result<()> {
        // SAFETY: ioctl is called on the live framebuffer fd and does not read
        // the null third argument for FBIOPAN_DISPLAY in Twilight.
        let result = unsafe {
            ioctl(
                self._file.as_raw_fd(),
                FBIOPAN_DISPLAY,
                ptr::null::<c_void>(),
            )
        };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl Drop for SoftwareOutput {
    fn drop(&mut self) {
        if !self.pixels.is_null() && self.map_bytes != 0 {
            // SAFETY: `pixels` and `map_bytes` come from a successful mmap in
            // SoftwareOutput::open and are unmapped exactly once here.
            let _ = unsafe { munmap(self.pixels.cast(), self.map_bytes) };
        }
    }
}

impl Drop for ShmPoolState {
    fn drop(&mut self) {
        if !self.mapped_addr.is_null() && self.size != 0 {
            // SAFETY: `mapped_addr` and `size` come from a successful mmap in
            // unsafe_mmap_shm and are unmapped exactly once when the pool dies.
            let _ = unsafe { munmap(self.mapped_addr.cast(), self.size) };
        }
        if self.fd >= 0 {
            // SAFETY: fd is owned by this pool after OwnedFdRaw::into_raw.
            let _ = unsafe { close(self.fd) };
            self.fd = -1;
        }
    }
}

impl OwnedFdRaw {
    fn into_raw(mut self) -> i32 {
        let fd = self.fd;
        self.fd = -1;
        fd
    }
}

impl Drop for OwnedFdRaw {
    fn drop(&mut self) {
        if self.fd >= 0 {
            // SAFETY: fd is owned by this wrapper unless it was moved out using
            // into_raw, in which case it is set to -1.
            let _ = unsafe { close(self.fd) };
            self.fd = -1;
        }
    }
}

impl Rect {
    fn union(self, other: Rect) -> Rect {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = self
            .x
            .saturating_add(self.width)
            .max(other.x.saturating_add(other.width));
        let y1 = self
            .y
            .saturating_add(self.height)
            .max(other.y.saturating_add(other.height));
        Rect {
            x: x0,
            y: y0,
            width: x1.saturating_sub(x0),
            height: y1.saturating_sub(y0),
        }
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| io::Error::new(ErrorKind::UnexpectedEof, "missing u32 argument"))?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn read_i32(bytes: &[u8], offset: usize) -> io::Result<i32> {
    Ok(read_u32(bytes, offset)? as i32)
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

fn cmsg_align(value: usize) -> usize {
    align4_to(value, size_of::<usize>())
}

fn align4_to(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}
