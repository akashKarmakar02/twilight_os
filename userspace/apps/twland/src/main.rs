use core::ffi::c_void;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::ptr;

mod framebuffer;
mod input;
mod output;
mod shell;
mod wire;
use framebuffer::{Frame, Framebuffer, Rect};
use shell::layer::{self, Geometry, KeyboardInteractivity, Layer, LayerShellState};
use wire::{
    create_empty_memfd, parse_chunk, push_fixed, push_i32, push_u32, push_wayland_string,
    read_i32, read_u32, read_wayland_string, recv_raw, send_message, OwnedFdRaw,
    ReceivedMessage,
};

const RUNTIME_DIR: &str = "/run/user/0";
const WAYLAND_SOCKET: &str = "/run/user/0/wayland-0";
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
const WL_SEAT_GET_POINTER: u16 = 0;
const WL_SEAT_GET_KEYBOARD: u16 = 1;
const WL_SEAT_CAPABILITIES: u16 = 0;
const WL_SEAT_NAME: u16 = 1;
const WL_POINTER_ENTER: u16 = 0;
const WL_POINTER_LEAVE: u16 = 1;
const WL_POINTER_MOTION: u16 = 2;
const WL_POINTER_BUTTON: u16 = 3;
const WL_KEYBOARD_KEYMAP: u16 = 0;
const WL_KEYBOARD_ENTER: u16 = 1;
const WL_KEYBOARD_LEAVE: u16 = 2;
const WL_KEYBOARD_KEY: u16 = 3;
const WL_SURFACE_DESTROY: u16 = 0;
const WL_SURFACE_ATTACH: u16 = 1;
const WL_SURFACE_DAMAGE: u16 = 2;
const WL_SURFACE_FRAME: u16 = 3;
const WL_SURFACE_COMMIT: u16 = 6;
const XDG_WM_BASE_PONG: u16 = 3;
const XDG_WM_BASE_GET_XDG_SURFACE: u16 = 2;
const XDG_SURFACE_DESTROY: u16 = 0;
const XDG_SURFACE_GET_TOPLEVEL: u16 = 1;
const XDG_SURFACE_ACK_CONFIGURE: u16 = 4;
const XDG_SURFACE_CONFIGURE: u16 = 0;
const XDG_TOPLEVEL_DESTROY: u16 = 0;
const XDG_TOPLEVEL_CONFIGURE: u16 = 0;
const XDG_TOPLEVEL_CLOSE: u16 = 1;
const XDG_TOPLEVEL_SET_TITLE: u16 = 2;
const XDG_TOPLEVEL_SET_APP_ID: u16 = 3;

const WL_SHM_FORMAT_ARGB8888: u32 = 0;
const WL_SHM_FORMAT_XRGB8888: u32 = 1;
const WL_SEAT_CAPABILITY_POINTER: u32 = 1;
const WL_SEAT_CAPABILITY_KEYBOARD: u32 = 2;
#[allow(dead_code)]
const WL_SEAT_CAPABILITY_TOUCH: u32 = 4;
const WL_POINTER_BUTTON_STATE_RELEASED: u32 = 0;
const WL_POINTER_BUTTON_STATE_PRESSED: u32 = 1;
const WL_KEYBOARD_KEYMAP_FORMAT_NO_KEYMAP: u32 = 0;
const WL_KEYBOARD_KEY_STATE_RELEASED: u32 = 0;
const WL_KEYBOARD_KEY_STATE_PRESSED: u32 = 1;
const TWLAND_ALLOW_ROLELESS_DEBUG_SURFACES: bool = false;
const TWLAND_DEBUG_INPUT: bool = true;
const TWLAND_DEBUG_POINTER_MOTION: bool = false;
const TITLEBAR_HEIGHT: i32 = 24;
const BORDER_WIDTH: i32 = 2;
const CLOSE_BUTTON_SIZE: i32 = 18;
const DESKTOP_BACKGROUND: u32 = 0xff101018;

const PROT_READ: i32 = 0x1;
const MAP_SHARED: i32 = 0x01;
const MAP_FAILED: *mut c_void = usize::MAX as *mut c_void;

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
    Global {
        name: 5,
        interface: "xdg_wm_base",
        version: 6,
        kind: WaylandObjectKind::XdgWmBase,
    },
    Global {
        name: 6,
        interface: layer::wire::INTERFACE,
        version: layer::wire::VERSION,
        kind: WaylandObjectKind::LayerShell,
    },
];

unsafe extern "C" {
    fn sched_yield() -> i32;
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
}

struct Client {
    objects: HashMap<u32, WaylandObject>,
    pools: HashMap<u32, ShmPoolState>,
    buffers: HashMap<u32, BufferState>,
    surfaces: HashMap<u32, SurfaceState>,
    seats: HashMap<u32, SeatState>,
    pointers: HashMap<u32, PointerState>,
    keyboards: HashMap<u32, KeyboardState>,
    xdg_surfaces: HashMap<u32, XdgSurfaceState>,
    xdg_toplevels: HashMap<u32, XdgToplevelState>,
    layer_shell: LayerShellState,
    layer_layout_dirty: bool,
    layer_layout_output: Geometry,
    queued_messages: VecDeque<ReceivedMessage>,
    /// Bytes from a read that did not contain a complete final frame; the next
    /// read appends to this and framing resumes.  AF_UNIX streams split frames
    /// at arbitrary byte boundaries, so a frame can span two reads.
    residual: Vec<u8>,
    /// Out-of-band file descriptors received via `SCM_RIGHTS`, in stream order.
    /// A single read may carry fds for several batched requests, so each
    /// fd-carrying request handler pops exactly the fds its signature requires.
    pending_fds: VecDeque<OwnedFdRaw>,
    /// Real mouse reader (`/dev/input/mice`), opened non-blocking.  `None` when
    /// no mouse is present, in which case `poll_input_events` produces nothing.
    mouse: Option<input::Mouse>,
    /// Real keyboard reader (`/dev/input/event0`), opened non-blocking.  `None`
    /// when no keyboard is present, in which case `poll_input_events` produces
    /// no key events.
    keyboard: Option<input::Keyboard>,
    input: InputState,
    compositor: CompositorState,
    next_serial: u32,
    next_window_offset: i32,
}

#[derive(Debug, Clone, Copy)]
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
    Pointer,
    Keyboard,
    Output,
    XdgWmBase,
    XdgSurface,
    XdgToplevel,
    LayerShell,
    LayerSurface,
}

impl WaylandObjectKind {
    /// The Wayland interface name, for log lines.
    fn as_str(self) -> &'static str {
        match self {
            WaylandObjectKind::Display => "wl_display",
            WaylandObjectKind::Registry => "wl_registry",
            WaylandObjectKind::Callback => "wl_callback",
            WaylandObjectKind::Compositor => "wl_compositor",
            WaylandObjectKind::Shm => "wl_shm",
            WaylandObjectKind::ShmPool => "wl_shm_pool",
            WaylandObjectKind::Buffer => "wl_buffer",
            WaylandObjectKind::Surface => "wl_surface",
            WaylandObjectKind::Seat => "wl_seat",
            WaylandObjectKind::Pointer => "wl_pointer",
            WaylandObjectKind::Keyboard => "wl_keyboard",
            WaylandObjectKind::Output => "wl_output",
            WaylandObjectKind::XdgWmBase => "xdg_wm_base",
            WaylandObjectKind::XdgSurface => "xdg_surface",
            WaylandObjectKind::XdgToplevel => "xdg_toplevel",
            WaylandObjectKind::LayerShell => "zwlr_layer_shell_v1",
            WaylandObjectKind::LayerSurface => "zwlr_layer_surface_v1",
        }
    }
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
    /// `None` means no attach request is pending; `Some(None)` represents
    /// `wl_surface.attach(NULL)` and therefore an explicit unmap.
    pending_buffer: Option<Option<u32>>,
    damage: Option<Rect>,
    x: i32,
    y: i32,
    attach_x: i32,
    attach_y: i32,
    role: Option<SurfaceRole>,
    has_committed: bool,
    frame_callbacks: Vec<u32>,
    mapped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceRole {
    Xdg(u32),
    Layer(u32),
}

#[derive(Debug, Clone)]
struct Window {
    surface_id: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    mapped: bool,
    focused: bool,
    title: String,
    app_id: String,
    decoration: DecorationState,
}

#[derive(Debug, Clone, Copy)]
struct DecorationState {
    enabled: bool,
    titlebar_height: i32,
    border_width: i32,
    close_button_rect: Rect,
}

#[derive(Debug, Clone)]
struct CompositorState {
    windows: Vec<Window>,
    focused_surface: Option<u32>,
    drag: Option<DragState>,
}

#[derive(Debug, Clone, Copy)]
struct DragState {
    surface_id: u32,
    pointer_start_x: i32,
    pointer_start_y: i32,
    window_start_x: i32,
    window_start_y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HitTest {
    None,
    LayerSurface { surface_id: u32 },
    ClientArea { surface_id: u32 },
    Titlebar { surface_id: u32 },
    CloseButton { surface_id: u32 },
    Border { surface_id: u32 },
}

#[derive(Debug, Clone)]
struct SeatState {
    name: String,
    pointer_id: Option<u32>,
    keyboard_id: Option<u32>,
    pointer_focus: Option<u32>,
    keyboard_focus: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
struct PointerState {
    seat_id: u32,
}

#[derive(Debug, Clone, Copy)]
struct KeyboardState {
    seat_id: u32,
}

#[derive(Debug, Clone)]
struct InputState {
    pointer_x: i32,
    pointer_y: i32,
    buttons: u32,
    focused_surface: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
enum TwlandInputEvent {
    PointerMove { dx: i32, dy: i32 },
    /// Not produced by the mouse reader, but kept for future absolute-pointer
    /// devices (tablet/touch).  Dispatched in `dispatch_input_event`.
    #[allow(dead_code)]
    PointerAbsolute { x: i32, y: i32 },
    PointerButton { button: u32, pressed: bool },
    /// Produced by the keyboard reader (`/dev/input/event0`); forwarded to the
    /// focused surface as `wl_keyboard.key`.
    Key { keycode: u32, pressed: bool },
}

#[derive(Debug, Clone)]
struct XdgSurfaceState {
    wl_surface_id: u32,
    configured: bool,
    pending_configure_serial: Option<u32>,
    last_acked_configure_serial: Option<u32>,
    role: Option<XdgRole>,
}

#[derive(Debug, Clone, Copy)]
enum XdgRole {
    Toplevel(u32),
}

#[derive(Debug, Clone)]
struct XdgToplevelState {
    xdg_surface_id: u32,
    title: String,
    app_id: String,
    width: i32,
    height: i32,
    activated: bool,
}

#[derive(Debug, Clone, Copy)]
struct Global {
    name: u32,
    interface: &'static str,
    version: u32,
    kind: WaylandObjectKind,
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

    let mut output = Framebuffer::open()?;
    output.clear_and_present(DESKTOP_BACKGROUND)?;
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
                let _ = output.clear_and_present(DESKTOP_BACKGROUND);
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

fn handle_client(mut stream: UnixStream, output: &mut Framebuffer) -> io::Result<()> {
    let mut client = Client::new(output.geometry());

    loop {
        let mut handled_request = false;
        loop {
            let message = match recv_wayland_message(&mut client, &mut stream) {
                Ok(Some(message)) => message,
                Ok(None) => return Ok(()),
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::UnexpectedEof
                            | ErrorKind::ConnectionReset
                            | ErrorKind::BrokenPipe
                    ) =>
                {
                    return Ok(());
                }
                Err(error) => return Err(error),
            };

            handled_request = true;
            println!(
                "twland: request object={} opcode={} size={}",
                message.header.object_id, message.header.opcode, message.header.size
            );

            dispatch_request(&mut client, output, &mut stream, message)?;
        }

        let input_events = poll_input_events(&mut client);
        for event in input_events {
            dispatch_input_event(&mut client, &mut stream, output, event)?;
        }

        if !handled_request {
            compositor_idle();
        }
    }
}

fn compositor_idle() {
    // Avoid std::thread::sleep here.  Twilight is still growing its Linux time
    // syscall surface and Rust/musl may implement sleep through
    // clock_nanosleep(2); if that syscall is missing or partial, std can panic.
    // A short cooperative yield is enough for twland's current single-client
    // debug compositor loop.
    for _ in 0..4 {
        // SAFETY: sched_yield has no pointer arguments and no Rust aliasing or
        // lifetime requirements.  Twilight implements the Linux ABI syscall.
        unsafe {
            let _ = sched_yield();
        }
    }
}

impl Client {
    fn new(output_size: (i32, i32)) -> Self {
        let mut objects = HashMap::new();
        objects.insert(
            1,
            WaylandObject {
                kind: WaylandObjectKind::Display,
            },
        );
        let output_geometry = Geometry {
            x: 0,
            y: 0,
            width: output_size.0,
            height: output_size.1,
        };

        Self {
            objects,
            pools: HashMap::new(),
            buffers: HashMap::new(),
            surfaces: HashMap::new(),
            seats: HashMap::new(),
            pointers: HashMap::new(),
            keyboards: HashMap::new(),
            xdg_surfaces: HashMap::new(),
            xdg_toplevels: HashMap::new(),
            layer_shell: LayerShellState::new(output_geometry),
            layer_layout_dirty: true,
            layer_layout_output: output_geometry,
            queued_messages: VecDeque::new(),
            residual: Vec::new(),
            pending_fds: VecDeque::new(),
            mouse: match input::Mouse::open() {
                Ok(Some(mouse)) => {
                    println!("twland: mouse opened on {}", input::MICE_PATH);
                    Some(mouse)
                }
                Ok(None) => {
                    println!("twland: no mouse at {}", input::MICE_PATH);
                    None
                }
                Err(error) => {
                    eprintln!("twland: failed to open mouse: {error}");
                    None
                }
            },
            keyboard: match input::Keyboard::open() {
                Ok(Some(keyboard)) => {
                    println!("twland: keyboard opened on {}", input::EVENT0_PATH);
                    Some(keyboard)
                }
                Ok(None) => {
                    println!("twland: no keyboard at {}", input::EVENT0_PATH);
                    None
                }
                Err(error) => {
                    eprintln!("twland: failed to open keyboard: {error}");
                    None
                }
            },
            input: InputState {
                pointer_x: 80,
                pointer_y: 80,
                buttons: 0,
                focused_surface: None,
            },
            compositor: CompositorState {
                windows: Vec::new(),
                focused_surface: None,
                drag: None,
            },
            next_serial: 1,
            next_window_offset: 0,
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

    fn next_window_position(&mut self) -> (i32, i32) {
        let offset = self.next_window_offset;
        self.next_window_offset = (self.next_window_offset + 40).min(320);
        (60 + offset, 60 + offset)
    }

    fn pointer_ids(&self) -> Vec<u32> {
        self.pointers
            .iter()
            .filter_map(|(pointer_id, pointer)| {
                self.seats
                    .contains_key(&pointer.seat_id)
                    .then_some(*pointer_id)
            })
            .collect()
    }

    fn keyboard_ids(&self) -> Vec<u32> {
        self.keyboards
            .iter()
            .filter_map(|(keyboard_id, keyboard)| {
                self.seats
                    .contains_key(&keyboard.seat_id)
                    .then_some(*keyboard_id)
            })
            .collect()
    }

    fn surface_size(&self, surface_id: u32) -> Option<(i32, i32)> {
        let surface = self.surfaces.get(&surface_id)?;
        let buffer_id = surface
            .attached_buffer
            .or(surface.pending_buffer.flatten())?;
        let buffer = self.buffers.get(&buffer_id)?;
        Some((buffer.width, buffer.height))
    }

    fn window_for_surface(&self, surface_id: u32) -> Option<&Window> {
        self.compositor
            .windows
            .iter()
            .find(|window| window.surface_id == surface_id)
    }

    fn window_for_surface_mut(&mut self, surface_id: u32) -> Option<&mut Window> {
        self.compositor
            .windows
            .iter_mut()
            .find(|window| window.surface_id == surface_id)
    }

    fn xdg_ids_for_surface(&self, surface_id: u32) -> Option<(u32, u32)> {
        let SurfaceRole::Xdg(xdg_surface_id) = self.surfaces.get(&surface_id)?.role? else {
            return None;
        };
        let role = self.xdg_surfaces.get(&xdg_surface_id)?.role?;
        let XdgRole::Toplevel(toplevel_id) = role;
        Some((xdg_surface_id, toplevel_id))
    }
}

fn dispatch_request(
    client: &mut Client,
    output: &mut Framebuffer,
    stream: &mut UnixStream,
    message: ReceivedMessage,
) -> io::Result<()> {
    let Some(object) = client.objects.get(&message.header.object_id).copied() else {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("unknown object {}", message.header.object_id),
        ));
    };

    let opcode = message.header.opcode;
    let object_id = message.header.object_id;

    // Route by interface, then by opcode.  Each interface ends in a non-fatal
    // fallback: a minimal compositor may legitimately ignore optional requests
    // it does not implement (set_opaque_region, create_region, get_touch,
    // set_window_geometry, ...).  Disconnecting on them would drop every real
    // toolkit client.  Only a structurally unknown object is fatal, and that
    // is caught above before we reach this match.
    match object.kind {
        WaylandObjectKind::Display => match opcode {
            WL_DISPLAY_SYNC => handle_display_sync(client, stream, &message.payload),
            WL_DISPLAY_GET_REGISTRY => handle_get_registry(client, stream, &message.payload),
            other => ignore_unimplemented("wl_display", other, object_id),
        },
        WaylandObjectKind::Registry => match opcode {
            WL_REGISTRY_BIND => {
                handle_registry_bind(client, output, stream, object_id, &message.payload)
            }
            other => ignore_unimplemented("wl_registry", other, object_id),
        },
        WaylandObjectKind::Compositor => match opcode {
            WL_COMPOSITOR_CREATE_SURFACE => handle_compositor_create_surface(client, &message.payload),
            other => ignore_unimplemented("wl_compositor", other, object_id),
        },
        WaylandObjectKind::Shm => match opcode {
            WL_SHM_CREATE_POOL => handle_shm_create_pool(client, message.payload),
            other => ignore_unimplemented("wl_shm", other, object_id),
        },
        WaylandObjectKind::ShmPool => match opcode {
            WL_SHM_POOL_CREATE_BUFFER => {
                handle_shm_pool_create_buffer(client, object_id, &message.payload)
            }
            WL_SHM_POOL_DESTROY => {
                handle_shm_pool_destroy(client, object_id);
                Ok(())
            }
            other => ignore_unimplemented("wl_shm_pool", other, object_id),
        },
        WaylandObjectKind::Buffer => match opcode {
            WL_BUFFER_DESTROY => {
                handle_buffer_destroy(client, object_id);
                Ok(())
            }
            other => ignore_unimplemented("wl_buffer", other, object_id),
        },
        WaylandObjectKind::Surface => match opcode {
            WL_SURFACE_DESTROY => {
                handle_surface_destroy(client, output, stream, object_id)?;
                Ok(())
            }
            WL_SURFACE_ATTACH => handle_surface_attach(client, object_id, &message.payload),
            WL_SURFACE_DAMAGE => handle_surface_damage(client, object_id, &message.payload),
            WL_SURFACE_FRAME => handle_surface_frame(client, object_id, &message.payload),
            WL_SURFACE_COMMIT => handle_surface_commit(client, output, stream, object_id),
            // set_opaque_region(4), set_input_region(5), set_buffer_transform(7),
            // set_buffer_scale(8), damage_buffer(9): optional, ignored.
            other => ignore_unimplemented("wl_surface", other, object_id),
        },
        WaylandObjectKind::Seat => match opcode {
            WL_SEAT_GET_POINTER => handle_seat_get_pointer(client, object_id, &message.payload),
            WL_SEAT_GET_KEYBOARD => {
                handle_seat_get_keyboard(client, stream, object_id, &message.payload)
            }
            // get_touch(2): optional, ignored.
            other => ignore_unimplemented("wl_seat", other, object_id),
        },
        WaylandObjectKind::XdgWmBase => match opcode {
            XDG_WM_BASE_PONG => handle_xdg_wm_base_pong(&message.payload),
            XDG_WM_BASE_GET_XDG_SURFACE => {
                handle_xdg_wm_base_get_xdg_surface(client, object_id, &message.payload)
            }
            other => ignore_unimplemented("xdg_wm_base", other, object_id),
        },
        WaylandObjectKind::XdgSurface => match opcode {
            XDG_SURFACE_DESTROY => {
                handle_xdg_surface_destroy(client, output, object_id)?;
                Ok(())
            }
            XDG_SURFACE_GET_TOPLEVEL => {
                handle_xdg_surface_get_toplevel(client, object_id, &message.payload)
            }
            XDG_SURFACE_ACK_CONFIGURE => {
                handle_xdg_surface_ack_configure(client, object_id, &message.payload)
            }
            // get_popup(2), set_window_geometry(3): optional, ignored.
            other => ignore_unimplemented("xdg_surface", other, object_id),
        },
        WaylandObjectKind::XdgToplevel => match opcode {
            XDG_TOPLEVEL_SET_TITLE => {
                handle_xdg_toplevel_set_title(client, object_id, &message.payload)
            }
            XDG_TOPLEVEL_SET_APP_ID => {
                handle_xdg_toplevel_set_app_id(client, object_id, &message.payload)
            }
            XDG_TOPLEVEL_DESTROY => {
                handle_xdg_toplevel_destroy(client, output, object_id)?;
                Ok(())
            }
            // move(4), resize(5), set_min/max_size, set_maximized, etc.: optional, ignored.
            other => ignore_unimplemented("xdg_toplevel", other, object_id),
        },
        WaylandObjectKind::LayerShell => {
            let effects =
                layer::dispatch_shell_request(client, object_id, opcode, &message.payload)?;
            apply_layer_dispatch_effects(client, output, stream, effects)
        }
        WaylandObjectKind::LayerSurface => {
            let effects =
                layer::dispatch_surface_request(client, object_id, opcode, &message.payload)?;
            apply_layer_dispatch_effects(client, output, stream, effects)
        }
        // These objects emit only events; clients never send requests to them.
        WaylandObjectKind::Callback
        | WaylandObjectKind::Pointer
        | WaylandObjectKind::Keyboard
        | WaylandObjectKind::Output => {
            ignore_unimplemented(object.kind.as_str(), opcode, object_id)
        }
    }
}

fn apply_layer_dispatch_effects(
    client: &mut Client,
    output: &mut Framebuffer,
    stream: &mut UnixStream,
    effects: layer::DispatchEffects,
) -> io::Result<()> {
    if effects.layer_layout_changed {
        client.layer_layout_dirty = true;
        ensure_layer_layout(client, output);
    }
    if effects.reconcile_pointer_focus {
        reconcile_pointer_focus(client, stream)?;
    }
    if effects.reconcile_keyboard_focus {
        reconcile_keyboard_focus(client, stream)?;
    }
    if effects.redraw {
        redraw_scene(client, output)?;
    }
    Ok(())
}

/// Log and discard an optional request this compositor does not implement.
///
/// Used as the fallback arm for every interface.  A Wayland client may send
/// optional requests (regions, transforms, touch, popup geometry, ...) that a
/// minimal compositor can legitimately ignore; tearing down the connection on
/// them would disconnect every real toolkit client.
fn ignore_unimplemented(interface: &str, opcode: u16, object_id: u32) -> io::Result<()> {
    println!(
        "twland: {interface} request opcode={opcode} ignored (unimplemented) object={object_id}"
    );
    Ok(())
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
    output: &Framebuffer,
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
    if version == 0 || version > global.version {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!(
                "cannot bind {} at version {version}; supported version is {}",
                global.interface, global.version
            ),
        ));
    }

    client.insert_object(new_id, global.kind)?;

    if global.kind == WaylandObjectKind::Shm {
        send_shm_format(stream, new_id, WL_SHM_FORMAT_ARGB8888)?;
        send_shm_format(stream, new_id, WL_SHM_FORMAT_XRGB8888)?;
        println!("twland: wl_shm bound, sent supported formats");
    }
    if global.kind == WaylandObjectKind::Seat {
        client.seats.insert(
            new_id,
            SeatState {
                name: "seat0".to_string(),
                pointer_id: None,
                keyboard_id: None,
                pointer_focus: None,
                keyboard_focus: None,
            },
        );
        let seat_name = client
            .seats
            .get(&new_id)
            .map(|seat| seat.name.clone())
            .unwrap_or_else(|| "seat0".to_string());
        send_seat_capabilities(
            stream,
            new_id,
            WL_SEAT_CAPABILITY_POINTER | WL_SEAT_CAPABILITY_KEYBOARD,
        )?;
        send_seat_name(stream, new_id, &seat_name)?;
        println!("twland: wl_seat bound, sent pointer keyboard capabilities");
    }
    if global.kind == WaylandObjectKind::Output {
        let (width, height) = output.geometry();
        output::send_initial_events(stream, new_id, version, width, height)?;
        println!("twland: wl_output bound, sent geometry/mode/done");
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
            role: None,
            has_committed: false,
            frame_callbacks: Vec::new(),
            mapped: false,
        },
    );
    println!("twland: wl_compositor.create_surface id={surface_id}");
    Ok(())
}

fn handle_seat_get_pointer(client: &mut Client, seat_id: u32, payload: &[u8]) -> io::Result<()> {
    let pointer_id = read_u32(payload, 0)?;
    if !client.seats.contains_key(&seat_id) {
        return Err(io::Error::new(ErrorKind::InvalidData, "unknown wl_seat"));
    }

    client.insert_object(pointer_id, WaylandObjectKind::Pointer)?;
    client.pointers.insert(pointer_id, PointerState { seat_id });
    if let Some(seat) = client.seats.get_mut(&seat_id) {
        seat.pointer_id = Some(pointer_id);
    }

    println!("twland: wl_seat.get_pointer id={pointer_id}");
    Ok(())
}

fn handle_seat_get_keyboard(
    client: &mut Client,
    stream: &mut UnixStream,
    seat_id: u32,
    payload: &[u8],
) -> io::Result<()> {
    let keyboard_id = read_u32(payload, 0)?;
    if !client.seats.contains_key(&seat_id) {
        return Err(io::Error::new(ErrorKind::InvalidData, "unknown wl_seat"));
    }

    client.insert_object(keyboard_id, WaylandObjectKind::Keyboard)?;
    client
        .keyboards
        .insert(keyboard_id, KeyboardState { seat_id });
    if let Some(seat) = client.seats.get_mut(&seat_id) {
        seat.keyboard_id = Some(keyboard_id);
    }

    println!("twland: wl_seat.get_keyboard id={keyboard_id}");
    send_keyboard_keymap(stream, keyboard_id).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to send wl_keyboard.keymap: {err}"),
        )
    })?;
    println!("twland: sent keyboard keymap");
    Ok(())
}

fn handle_shm_create_pool(client: &mut Client, payload: Vec<u8>) -> io::Result<()> {
    let pool_id = read_u32(&payload, 0)?;
    let size = read_i32(&payload, 4)?;
    if size <= 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("invalid shm pool size {size}"),
        ));
    }

    // wl_shm.create_pool carries exactly one fd argument; pop it from the
    // per-client FIFO in stream order.
    let fd = client.pending_fds.pop_front().ok_or_else(|| {
        io::Error::new(
            ErrorKind::InvalidData,
            "wl_shm.create_pool requires one SCM_RIGHTS fd",
        )
    })?;
    let fd = fd.into_raw();
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

fn handle_surface_destroy(
    client: &mut Client,
    output: &mut Framebuffer,
    stream: &mut UnixStream,
    surface_id: u32,
) -> io::Result<()> {
    client.objects.remove(&surface_id);
    let mut layer_layout_changed = false;
    if let Some(surface) = client.surfaces.remove(&surface_id) {
        match surface.role {
            Some(SurfaceRole::Layer(layer_surface_id)) => {
                client.objects.remove(&layer_surface_id);
                client.layer_shell.remove(layer_surface_id);
                layer_layout_changed = true;
            }
            Some(SurfaceRole::Xdg(xdg_surface_id)) => {
                client.objects.remove(&xdg_surface_id);
                if let Some(xdg_surface) = client.xdg_surfaces.remove(&xdg_surface_id)
                    && let Some(XdgRole::Toplevel(toplevel_id)) = xdg_surface.role
                {
                    client.objects.remove(&toplevel_id);
                    client.xdg_toplevels.remove(&toplevel_id);
                }
            }
            None => {}
        }
    }
    remove_window_for_surface(client, surface_id);
    if layer_layout_changed {
        client.layer_layout_dirty = true;
        ensure_layer_layout(client, output);
    }
    reconcile_pointer_focus(client, stream)?;
    reconcile_keyboard_focus(client, stream)?;
    println!("twland: wl_surface.destroy id={surface_id}");
    redraw_scene(client, output)?;
    Ok(())
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
    surface.pending_buffer = Some((buffer_id != 0).then_some(buffer_id));
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

fn handle_surface_frame(client: &mut Client, surface_id: u32, payload: &[u8]) -> io::Result<()> {
    let callback_id = read_u32(payload, 0)?;
    client.insert_object(callback_id, WaylandObjectKind::Callback)?;
    let surface = client
        .surfaces
        .get_mut(&surface_id)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown surface"))?;
    surface.frame_callbacks.push(callback_id);
    println!("twland: wl_surface.frame surface={surface_id} callback={callback_id}");
    Ok(())
}

fn handle_xdg_wm_base_pong(payload: &[u8]) -> io::Result<()> {
    let serial = read_u32(payload, 0)?;
    println!("twland: xdg_wm_base.pong serial={serial}");
    Ok(())
}

fn handle_xdg_wm_base_get_xdg_surface(
    client: &mut Client,
    _wm_base_id: u32,
    payload: &[u8],
) -> io::Result<()> {
    let xdg_surface_id = read_u32(payload, 0)?;
    let wl_surface_id = read_u32(payload, 4)?;

    let surface = client
        .surfaces
        .get_mut(&wl_surface_id)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown wl_surface"))?;
    if surface.role.is_some() {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "wl_surface already has a role",
        ));
    }
    if surface.has_committed {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "xdg role assigned after wl_surface was committed",
        ));
    }

    surface.role = Some(SurfaceRole::Xdg(xdg_surface_id));
    client.insert_object(xdg_surface_id, WaylandObjectKind::XdgSurface)?;
    client.xdg_surfaces.insert(
        xdg_surface_id,
        XdgSurfaceState {
            wl_surface_id,
            configured: false,
            pending_configure_serial: None,
            last_acked_configure_serial: None,
            role: None,
        },
    );

    println!("twland: xdg_wm_base.get_xdg_surface id={xdg_surface_id} surface={wl_surface_id}");
    Ok(())
}

fn handle_xdg_surface_get_toplevel(
    client: &mut Client,
    xdg_surface_id: u32,
    payload: &[u8],
) -> io::Result<()> {
    let toplevel_id = read_u32(payload, 0)?;
    let wl_surface_id = client
        .xdg_surfaces
        .get(&xdg_surface_id)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown xdg_surface"))?
        .wl_surface_id;

    {
        let xdg_surface = client
            .xdg_surfaces
            .get_mut(&xdg_surface_id)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown xdg_surface"))?;
        if xdg_surface.role.is_some() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "xdg_surface already has a role",
            ));
        }
        xdg_surface.role = Some(XdgRole::Toplevel(toplevel_id));
    }

    let (x, y) = client.next_window_position();
    if let Some(surface) = client.surfaces.get_mut(&wl_surface_id) {
        surface.x = x;
        surface.y = y;
    }

    client.insert_object(toplevel_id, WaylandObjectKind::XdgToplevel)?;
    client.xdg_toplevels.insert(
        toplevel_id,
        XdgToplevelState {
            xdg_surface_id,
            title: String::new(),
            app_id: String::new(),
            width: 400,
            height: 300,
            activated: true,
        },
    );

    println!("twland: xdg_surface.get_toplevel id={toplevel_id}");
    Ok(())
}

fn handle_xdg_surface_ack_configure(
    client: &mut Client,
    xdg_surface_id: u32,
    payload: &[u8],
) -> io::Result<()> {
    let serial = read_u32(payload, 0)?;
    let xdg_surface = client
        .xdg_surfaces
        .get_mut(&xdg_surface_id)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown xdg_surface"))?;

    if xdg_surface.pending_configure_serial != Some(serial) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("unknown xdg configure serial {serial}"),
        ));
    }

    xdg_surface.pending_configure_serial = None;
    xdg_surface.last_acked_configure_serial = Some(serial);
    xdg_surface.configured = true;
    println!("twland: xdg_surface.ack_configure serial={serial}");
    Ok(())
}

fn handle_xdg_toplevel_set_title(
    client: &mut Client,
    toplevel_id: u32,
    payload: &[u8],
) -> io::Result<()> {
    let (title, _) = read_wayland_string(payload, 0)?;
    let (xdg_surface_id, title) = {
        let toplevel = client
            .xdg_toplevels
            .get_mut(&toplevel_id)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown xdg_toplevel"))?;
        toplevel.title = title;
        (toplevel.xdg_surface_id, toplevel.title.clone())
    };
    if let Some(surface_id) = client
        .xdg_surfaces
        .get(&xdg_surface_id)
        .map(|xdg_surface| xdg_surface.wl_surface_id)
        && let Some(window) = client.window_for_surface_mut(surface_id)
    {
        window.title = title.clone();
    }
    println!("twland: xdg_toplevel.set_title \"{}\"", title);
    Ok(())
}

fn handle_xdg_toplevel_set_app_id(
    client: &mut Client,
    toplevel_id: u32,
    payload: &[u8],
) -> io::Result<()> {
    let (app_id, _) = read_wayland_string(payload, 0)?;
    let (xdg_surface_id, app_id) = {
        let toplevel = client
            .xdg_toplevels
            .get_mut(&toplevel_id)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown xdg_toplevel"))?;
        toplevel.app_id = app_id;
        (toplevel.xdg_surface_id, toplevel.app_id.clone())
    };
    if let Some(surface_id) = client
        .xdg_surfaces
        .get(&xdg_surface_id)
        .map(|xdg_surface| xdg_surface.wl_surface_id)
        && let Some(window) = client.window_for_surface_mut(surface_id)
    {
        window.app_id = app_id.clone();
    }
    println!("twland: xdg_toplevel.set_app_id \"{}\"", app_id);
    Ok(())
}

fn handle_xdg_surface_destroy(
    client: &mut Client,
    output: &mut Framebuffer,
    xdg_surface_id: u32,
) -> io::Result<()> {
    client.objects.remove(&xdg_surface_id);
    if let Some(xdg_surface) = client.xdg_surfaces.remove(&xdg_surface_id) {
        if let Some(XdgRole::Toplevel(toplevel_id)) = xdg_surface.role {
            client.objects.remove(&toplevel_id);
            client.xdg_toplevels.remove(&toplevel_id);
        }
        if let Some(surface) = client.surfaces.get_mut(&xdg_surface.wl_surface_id)
            && surface.role == Some(SurfaceRole::Xdg(xdg_surface_id))
        {
            surface.role = None;
            surface.mapped = false;
        }
        remove_window_for_surface(client, xdg_surface.wl_surface_id);
        println!(
            "twland: xdg_surface.destroy id={xdg_surface_id} surface={}",
            xdg_surface.wl_surface_id
        );
        redraw_scene(client, output)?;
    }
    Ok(())
}

fn handle_xdg_toplevel_destroy(
    client: &mut Client,
    output: &mut Framebuffer,
    toplevel_id: u32,
) -> io::Result<()> {
    client.objects.remove(&toplevel_id);
    if let Some(toplevel) = client.xdg_toplevels.remove(&toplevel_id) {
        let surface_id = client
            .xdg_surfaces
            .get(&toplevel.xdg_surface_id)
            .map(|xdg_surface| xdg_surface.wl_surface_id);
        if let Some(xdg_surface) = client.xdg_surfaces.get_mut(&toplevel.xdg_surface_id) {
            xdg_surface.role = None;
        }
        if let Some(surface_id) = surface_id {
            remove_window_for_surface(client, surface_id);
            println!("twland: xdg_toplevel.destroy id={toplevel_id} surface={surface_id}");
            redraw_scene(client, output)?;
        }
    }
    Ok(())
}

fn handle_surface_commit(
    client: &mut Client,
    output: &mut Framebuffer,
    stream: &mut UnixStream,
    surface_id: u32,
) -> io::Result<()> {
    let mut surface = client
        .surfaces
        .get(&surface_id)
        .cloned()
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown surface"))?;
    let had_buffer = surface.attached_buffer.is_some();
    let had_assignment = surface.pending_buffer.is_some();
    if let Some(assignment) = surface.pending_buffer.take() {
        surface.attached_buffer = assignment;
    }
    surface.has_committed = true;

    let mut should_redraw = false;
    let mut should_reconcile_focus = false;
    let mut committed_buffer = None;

    match surface.role {
        Some(SurfaceRole::Xdg(xdg_surface_id)) => {
            if let Some(buffer_id) = surface.attached_buffer {
                let xdg_surface = client.xdg_surfaces.get(&xdg_surface_id).ok_or_else(|| {
                    io::Error::new(ErrorKind::InvalidData, "unknown xdg_surface")
                })?;
                if !xdg_surface.configured {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        "xdg_surface buffer committed before ack_configure",
                    ));
                }
                let Some(XdgRole::Toplevel(toplevel_id)) = xdg_surface.role else {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        "xdg_surface committed a buffer before assigning a role",
                    ));
                };
                let buffer = client.buffers.get(&buffer_id).cloned().ok_or_else(|| {
                    io::Error::new(ErrorKind::InvalidData, "commit references unknown buffer")
                })?;

                let newly_mapped = !surface.mapped;
                if newly_mapped {
                    if let Some(toplevel) = client.xdg_toplevels.get(&toplevel_id) {
                        println!(
                            "twland: mapped xdg_toplevel title=\"{}\" app_id=\"{}\"",
                            toplevel.title, toplevel.app_id
                        );
                    }
                    surface.mapped = true;
                }
                map_or_update_window(client, surface_id, toplevel_id, &buffer);
                committed_buffer = Some((buffer_id, buffer));
                should_redraw = true;
                should_reconcile_focus = newly_mapped;
            } else if had_buffer && had_assignment {
                surface.mapped = false;
                if let Some(xdg_surface) = client.xdg_surfaces.get_mut(&xdg_surface_id) {
                    xdg_surface.configured = false;
                    xdg_surface.pending_configure_serial = None;
                    xdg_surface.last_acked_configure_serial = None;
                }
                remove_window_for_surface(client, surface_id);
                should_redraw = true;
                should_reconcile_focus = true;
                println!("twland: unmapped xdg_surface={xdg_surface_id}");
            } else {
                let needs_initial_configure = client
                    .xdg_surfaces
                    .get(&xdg_surface_id)
                    .is_some_and(|xdg| !xdg.configured && xdg.pending_configure_serial.is_none());
                if needs_initial_configure {
                    handle_initial_xdg_empty_commit(
                        client,
                        stream,
                        surface_id,
                        xdg_surface_id,
                    )?;
                }
            }
        }
        Some(SurfaceRole::Layer(layer_surface_id)) => {
            client
                .layer_shell
                .get_mut(layer_surface_id)
                .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown layer surface"))?
                .commit_pending()
                .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;

            if let Some(buffer_id) = surface.attached_buffer {
                if !client
                    .layer_shell
                    .get(layer_surface_id)
                    .is_some_and(|layer_surface| layer_surface.can_map())
                {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        "layer surface attached a buffer before acking its initial configure",
                    ));
                }
                let buffer = client.buffers.get(&buffer_id).cloned().ok_or_else(|| {
                    io::Error::new(ErrorKind::InvalidData, "commit references unknown buffer")
                })?;
                if let Some(layer_surface) = client.layer_shell.get_mut(layer_surface_id) {
                    layer_surface.map();
                }
                surface.mapped = true;
                committed_buffer = Some((buffer_id, buffer));
                should_redraw = true;
                should_reconcile_focus = true;
                println!(
                    "twland: mapped zwlr_layer_surface_v1 id={layer_surface_id} surface={surface_id}"
                );
            } else if had_buffer && had_assignment {
                if let Some(layer_surface) = client.layer_shell.get_mut(layer_surface_id) {
                    layer_surface.unmap();
                }
                surface.mapped = false;
                should_redraw = true;
                should_reconcile_focus = true;
                println!("twland: unmapped zwlr_layer_surface_v1 id={layer_surface_id}");
            } else {
                send_initial_layer_configure(client, output, stream, layer_surface_id)?;
            }
            client.layer_layout_dirty |= should_redraw;
        }
        None => {
            if surface.attached_buffer.is_some() && !TWLAND_ALLOW_ROLELESS_DEBUG_SURFACES {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "roleless wl_surface commit is disabled; use a shell protocol",
                ));
            }
            surface.mapped = surface.attached_buffer.is_some();
            should_redraw = surface.mapped;
        }
    }

    surface.damage = None;
    let callbacks = std::mem::take(&mut surface.frame_callbacks);
    client.surfaces.insert(surface_id, surface);

    if should_reconcile_focus {
        reconcile_keyboard_focus(client, stream)?;
    }
    if should_redraw {
        redraw_scene(client, output)?;
    }
    if let Some((buffer_id, _)) = committed_buffer.as_ref() {
        send_message(stream, *buffer_id, WL_BUFFER_RELEASE, &[])?;
    }

    for callback_id in callbacks {
        let mut payload = Vec::new();
        push_u32(&mut payload, client.next_serial());
        send_message(stream, callback_id, WL_CALLBACK_DONE, &payload)?;
        client.objects.remove(&callback_id);
    }

    if let Some((buffer_id, buffer)) = committed_buffer {
        println!(
            "twland: wl_surface.commit surface={surface_id} buffer={buffer_id} redraw={}x{}",
            buffer.width, buffer.height
        );
    } else {
        println!("twland: wl_surface.commit surface={surface_id} no-buffer");
    }
    Ok(())
}

fn send_initial_layer_configure(
    client: &mut Client,
    output: &Framebuffer,
    stream: &mut UnixStream,
    layer_surface_id: u32,
) -> io::Result<()> {
    if !client
        .layer_shell
        .get(layer_surface_id)
        .is_some_and(|surface| surface.needs_initial_configure())
    {
        return Ok(());
    }

    let output_geometry = framebuffer_geometry(output);
    let (width, height) = client
        .layer_shell
        .configure_size(layer_surface_id, output_geometry)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown layer surface"))?;
    let serial = client.next_serial();
    layer::wire::send_configure(stream, layer_surface_id, serial, width, height)?;
    if let Some(surface) = client.layer_shell.get_mut(layer_surface_id) {
        surface.record_configure(layer::Configure {
            serial,
            width,
            height,
        });
    }
    println!(
        "twland: sent zwlr_layer_surface_v1.configure id={layer_surface_id} serial={serial} size={width}x{height}"
    );
    Ok(())
}

fn handle_initial_xdg_empty_commit(
    client: &mut Client,
    stream: &mut UnixStream,
    surface_id: u32,
    xdg_surface_id: u32,
) -> io::Result<()> {
    let role = client
        .xdg_surfaces
        .get(&xdg_surface_id)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown xdg_surface"))?
        .role;
    let Some(XdgRole::Toplevel(toplevel_id)) = role else {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "xdg_surface initial commit before role assignment",
        ));
    };

    let serial = client.next_serial();
    let (width, height, activated, linked_xdg_surface_id) = {
        let toplevel = client
            .xdg_toplevels
            .get(&toplevel_id)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown xdg_toplevel"))?;
        (
            toplevel.width,
            toplevel.height,
            toplevel.activated,
            toplevel.xdg_surface_id,
        )
    };

    send_xdg_toplevel_configure(stream, toplevel_id, width, height, activated)?;
    send_xdg_surface_configure(stream, xdg_surface_id, serial)?;
    if let Some(xdg_surface) = client.xdg_surfaces.get_mut(&xdg_surface_id) {
        xdg_surface.pending_configure_serial = Some(serial);
        xdg_surface.configured = false;
    }

    println!("twland: initial empty commit surface={surface_id}");
    println!(
        "twland: sent xdg_toplevel.configure width={width} height={height} xdg_surface={linked_xdg_surface_id}"
    );
    println!("twland: sent xdg_surface.configure serial={serial}");
    Ok(())
}

fn map_or_update_window(
    client: &mut Client,
    surface_id: u32,
    toplevel_id: u32,
    buffer: &BufferState,
) {
    let (title, app_id) = client
        .xdg_toplevels
        .get(&toplevel_id)
        .map(|toplevel| (toplevel.title.clone(), toplevel.app_id.clone()))
        .unwrap_or_default();
    let (initial_x, initial_y) = client
        .surfaces
        .get(&surface_id)
        .map(|surface| (surface.x, surface.y))
        .unwrap_or_else(|| client.next_window_position());

    if let Some(window) = client.window_for_surface_mut(surface_id) {
        window.width = buffer.width;
        window.height = buffer.height;
        window.title = title;
        window.app_id = app_id;
        window.mapped = true;
        window.decoration = decoration_for_window(window.x, window.y, window.width);
    } else {
        for existing in &mut client.compositor.windows {
            existing.focused = false;
        }
        let mut window = Window {
            surface_id,
            x: initial_x,
            y: initial_y,
            width: buffer.width,
            height: buffer.height,
            mapped: true,
            focused: true,
            title,
            app_id,
            decoration: decoration_for_window(initial_x, initial_y, buffer.width),
        };
        client.compositor.focused_surface = Some(surface_id);
        client.input.focused_surface = Some(surface_id);
        println!(
            "twland: mapped window surface={} title=\"{}\" app_id=\"{}\" pos={},{} size={}x{}",
            surface_id,
            window.title,
            window.app_id,
            window.x,
            window.y,
            window.width,
            window.height
        );
        window.decoration = decoration_for_window(window.x, window.y, window.width);
        client.compositor.windows.push(window);
    }

    sync_surface_to_window(client, surface_id);
}

fn decoration_for_window(x: i32, y: i32, width: i32) -> DecorationState {
    let close_x = x + BORDER_WIDTH + width - CLOSE_BUTTON_SIZE - 4;
    let close_y = y + BORDER_WIDTH + (TITLEBAR_HEIGHT - CLOSE_BUTTON_SIZE) / 2;
    DecorationState {
        enabled: true,
        titlebar_height: TITLEBAR_HEIGHT,
        border_width: BORDER_WIDTH,
        close_button_rect: Rect {
            x: close_x,
            y: close_y,
            width: CLOSE_BUTTON_SIZE,
            height: CLOSE_BUTTON_SIZE,
        },
    }
}

fn sync_surface_to_window(client: &mut Client, surface_id: u32) {
    let Some(window) = client.window_for_surface(surface_id).cloned() else {
        return;
    };
    if let Some(surface) = client.surfaces.get_mut(&surface_id) {
        surface.x = window.x + window.decoration.border_width;
        surface.y = window.y + window.decoration.titlebar_height + window.decoration.border_width;
    }
}

fn remove_window_for_surface(client: &mut Client, surface_id: u32) {
    let before = client.compositor.windows.len();
    client
        .compositor
        .windows
        .retain(|window| window.surface_id != surface_id);
    if before != client.compositor.windows.len() {
        println!("twland: unmapped window surface={surface_id}");
    }

    if client.compositor.focused_surface == Some(surface_id) {
        client.compositor.focused_surface = client
            .compositor
            .windows
            .iter()
            .rev()
            .find(|window| window.mapped)
            .map(|window| window.surface_id);
    }
    if client.input.focused_surface == Some(surface_id) {
        client.input.focused_surface = client.compositor.focused_surface;
    }
    for window in &mut client.compositor.windows {
        window.focused = Some(window.surface_id) == client.compositor.focused_surface;
    }
}

fn redraw_scene(client: &mut Client, output: &mut Framebuffer) -> io::Result<()> {
    ensure_layer_layout(client, output);
    let mut frame = output.begin_frame(DESKTOP_BACKGROUND)?;

    draw_layer(&mut frame, client, Layer::Background)?;
    draw_layer(&mut frame, client, Layer::Bottom)?;

    let windows = client.compositor.windows.clone();
    for window in windows.iter().filter(|window| window.mapped) {
        draw_window_decoration(&mut frame, window)?;
        let Some(surface) = client.surfaces.get(&window.surface_id).cloned() else {
            continue;
        };
        let mut draw_surface = surface;
        draw_surface.x = window.x + window.decoration.border_width;
        draw_surface.y =
            window.y + window.decoration.titlebar_height + window.decoration.border_width;
        draw_surface.attach_x = 0;
        draw_surface.attach_y = 0;
        draw_surface_buffer(&mut frame, client, &draw_surface)?;
    }

    draw_layer(&mut frame, client, Layer::Top)?;
    draw_layer(&mut frame, client, Layer::Overlay)?;
    draw_cursor(&mut frame, client.input.pointer_x, client.input.pointer_y)?;
    frame.present()
}

fn ensure_layer_layout(client: &mut Client, output: &Framebuffer) {
    if client.layer_layout_dirty || client.layer_layout_output != framebuffer_geometry(output) {
        refresh_layer_layout(client, output);
    }
}

fn framebuffer_geometry(output: &Framebuffer) -> Geometry {
    Geometry {
        x: 0,
        y: 0,
        width: output.width() as i32,
        height: output.height() as i32,
    }
}

fn refresh_layer_layout(client: &mut Client, output: &Framebuffer) {
    let output_geometry = framebuffer_geometry(output);
    let buffer_sizes = client
        .surfaces
        .iter()
        .filter_map(|(surface_id, surface)| {
            let buffer = client.buffers.get(&surface.attached_buffer?)?;
            Some((*surface_id, (buffer.width, buffer.height)))
        })
        .collect::<HashMap<_, _>>();
    client.layer_shell.arrange(output_geometry, &buffer_sizes);

    for layer in [Layer::Background, Layer::Bottom, Layer::Top, Layer::Overlay] {
        for surface_id in client.layer_shell.surface_ids_on(layer) {
            let Some(geometry) = client.layer_shell.geometry_for_surface(surface_id) else {
                continue;
            };
            if let Some(surface) = client.surfaces.get_mut(&surface_id) {
                surface.x = geometry.x;
                surface.y = geometry.y;
                surface.attach_x = 0;
                surface.attach_y = 0;
            }
        }
    }

    let usable = client.layer_shell.usable_area();
    for window in &mut client.compositor.windows {
        let outer_width = window.width + window.decoration.border_width * 2;
        let outer_height =
            window.height + window.decoration.titlebar_height + window.decoration.border_width * 2;
        let max_x = usable
            .x
            .saturating_add(usable.width.saturating_sub(outer_width).max(0));
        let max_y = usable
            .y
            .saturating_add(usable.height.saturating_sub(outer_height).max(0));
        window.x = window.x.clamp(usable.x, max_x);
        window.y = window.y.clamp(usable.y, max_y);
        window.decoration = decoration_for_window(window.x, window.y, window.width);
    }
    let window_ids = client
        .compositor
        .windows
        .iter()
        .map(|window| window.surface_id)
        .collect::<Vec<_>>();
    for surface_id in window_ids {
        sync_surface_to_window(client, surface_id);
    }
    client.layer_layout_dirty = false;
    client.layer_layout_output = output_geometry;
}

fn draw_layer(output: &mut Frame<'_>, client: &Client, layer: Layer) -> io::Result<()> {
    for surface_id in client.layer_shell.surface_ids_on(layer) {
        let Some(surface) = client.surfaces.get(&surface_id) else {
            continue;
        };
        draw_surface_buffer(output, client, surface)?;
    }
    Ok(())
}

fn draw_surface_buffer(
    output: &mut Frame<'_>,
    client: &Client,
    surface: &SurfaceState,
) -> io::Result<()> {
    let Some(buffer_id) = surface.attached_buffer else {
        return Ok(());
    };
    let Some(buffer) = client.buffers.get(&buffer_id) else {
        return Ok(());
    };
    let Some(pool) = client.pools.get(&buffer.pool_id) else {
        return Ok(());
    };
    let damage = Rect {
        x: 0,
        y: 0,
        width: buffer.width,
        height: buffer.height,
    };
    blit_shm_buffer_to_output(output, pool, buffer, surface, damage)?;
    Ok(())
}

/// Draw an X-shaped cursor at the pointer position.  The compositor owns the
/// cursor (Wayland server-side cursor), so this is drawn on top of all windows
/// after the scene is composited.
fn draw_cursor(output: &mut Frame<'_>, x: i32, y: i32) -> io::Result<()> {
    let outline = 0xff000000u32;
    let fill = 0xffffffffu32;

    // An X drawn as two diagonals.  Each row is (y_offset, x_offset, width),
    // centered on the hotspot at (x, y).  The two arms meet at the center.
    const ARM: &[(i32, i32, i32)] = &[
        (0, 0, 2),
        (1, 1, 2),
        (2, 2, 2),
        (3, 3, 2),
        (4, 4, 2),
        (5, 5, 2),
        (6, 6, 2),
        (7, 7, 2),
        (8, 8, 2),
        (9, 9, 2),
    ];

    // Draw both diagonals of the X, plus a black outline around each span.
    for &(dy, dx, w) in ARM {
        // Top-left -> bottom-right diagonal.
        output.fill_rect(Rect { x: x + dx - 1, y: y + dy, width: w + 2, height: 1 }, outline)?;
        output.fill_rect(Rect { x: x + dx, y: y + dy, width: w, height: 1 }, fill)?;
        // Top-right -> bottom-left diagonal (mirror x within the 10px span).
        let mx = 10 - dx - w;
        output.fill_rect(Rect { x: x + mx - 1, y: y + dy, width: w + 2, height: 1 }, outline)?;
        output.fill_rect(Rect { x: x + mx, y: y + dy, width: w, height: 1 }, fill)?;
    }
    Ok(())
}

fn draw_window_decoration(output: &mut Frame<'_>, window: &Window) -> io::Result<()> {
    if !window.decoration.enabled {
        return Ok(());
    }
    let outer_width = window.width + window.decoration.border_width * 2;
    let outer_height =
        window.height + window.decoration.titlebar_height + window.decoration.border_width * 2;
    let border_color = if window.focused {
        0xff7aa2ff
    } else {
        0xff303040
    };
    let title_color = if window.focused {
        0xff304fa8
    } else {
        0xff202838
    };
    let close_color = if window.focused {
        0xffcc4040
    } else {
        0xff703030
    };

    output.fill_rect(
        Rect {
            x: window.x,
            y: window.y,
            width: outer_width,
            height: outer_height,
        },
        border_color,
    )?;
    output.fill_rect(
        Rect {
            x: window.x + window.decoration.border_width,
            y: window.y + window.decoration.border_width,
            width: window.width,
            height: window.decoration.titlebar_height,
        },
        title_color,
    )?;
    output.fill_rect(window.decoration.close_button_rect, close_color)?;
    draw_close_glyph(output, window.decoration.close_button_rect)?;
    Ok(())
}

fn draw_close_glyph(output: &mut Frame<'_>, rect: Rect) -> io::Result<()> {
    for i in 4..(rect.width - 4).max(4) {
        output.fill_rect(
            Rect {
                x: rect.x + i,
                y: rect.y + i,
                width: 2,
                height: 2,
            },
            0xffffffff,
        )?;
        output.fill_rect(
            Rect {
                x: rect.x + rect.width - i - 2,
                y: rect.y + i,
                width: 2,
                height: 2,
            },
            0xffffffff,
        )?;
    }
    Ok(())
}

fn hit_test(client: &Client, x: i32, y: i32) -> HitTest {
    for layer in [Layer::Overlay, Layer::Top] {
        if let Some(surface_id) = client.layer_shell.topmost_at_on(layer, x, y) {
            return HitTest::LayerSurface { surface_id };
        }
    }

    let window_hit = hit_test_window(&client.compositor, x, y);
    if window_hit != HitTest::None {
        return window_hit;
    }

    for layer in [Layer::Bottom, Layer::Background] {
        if let Some(surface_id) = client.layer_shell.topmost_at_on(layer, x, y) {
            return HitTest::LayerSurface { surface_id };
        }
    }
    HitTest::None
}

fn hit_test_window(state: &CompositorState, x: i32, y: i32) -> HitTest {
    for window in state.windows.iter().rev().filter(|window| window.mapped) {
        let border = window.decoration.border_width;
        let titlebar = window.decoration.titlebar_height;
        let outer = Rect {
            x: window.x,
            y: window.y,
            width: window.width + border * 2,
            height: window.height + titlebar + border * 2,
        };
        if !rect_contains(outer, x, y) {
            continue;
        }
        if rect_contains(window.decoration.close_button_rect, x, y) {
            return HitTest::CloseButton {
                surface_id: window.surface_id,
            };
        }
        let title = Rect {
            x: window.x + border,
            y: window.y + border,
            width: window.width,
            height: titlebar,
        };
        if rect_contains(title, x, y) {
            return HitTest::Titlebar {
                surface_id: window.surface_id,
            };
        }
        let client = Rect {
            x: window.x + border,
            y: window.y + titlebar + border,
            width: window.width,
            height: window.height,
        };
        if rect_contains(client, x, y) {
            return HitTest::ClientArea {
                surface_id: window.surface_id,
            };
        }
        return HitTest::Border {
            surface_id: window.surface_id,
        };
    }
    HitTest::None
}

fn rect_contains(rect: Rect, x: i32, y: i32) -> bool {
    x >= rect.x
        && y >= rect.y
        && x < rect.x.saturating_add(rect.width)
        && y < rect.y.saturating_add(rect.height)
}

fn focus_window(
    client: &mut Client,
    stream: &mut UnixStream,
    output: &mut Framebuffer,
    surface_id: u32,
    send_configure: bool,
) -> io::Result<()> {
    let old_focus = client.compositor.focused_surface;
    if old_focus == Some(surface_id) {
        return Ok(());
    }

    if let Some(index) = client
        .compositor
        .windows
        .iter()
        .position(|window| window.surface_id == surface_id)
    {
        let window = client.compositor.windows.remove(index);
        client.compositor.windows.push(window);
        println!("twland: raised window surface={surface_id}");
    }

    client.compositor.focused_surface = Some(surface_id);
    for window in &mut client.compositor.windows {
        window.focused = window.surface_id == surface_id;
    }

    let keyboard_focus = client
        .layer_shell
        .exclusive_keyboard_focus()
        .or(Some(surface_id));
    update_keyboard_focus(client, stream, keyboard_focus)?;
    if send_configure {
        if let Some(old) = old_focus {
            send_focus_configure(client, stream, old, false)?;
        }
        send_focus_configure(client, stream, surface_id, true)?;
    }
    println!("twland: focus changed old={old_focus:?} new={surface_id}");
    redraw_scene(client, output)
}

fn send_focus_configure(
    client: &mut Client,
    stream: &mut UnixStream,
    surface_id: u32,
    activated: bool,
) -> io::Result<()> {
    let Some((xdg_surface_id, toplevel_id)) = client.xdg_ids_for_surface(surface_id) else {
        return Ok(());
    };
    let Some((width, height)) = client.surface_size(surface_id) else {
        return Ok(());
    };
    let serial = client.next_serial();
    send_xdg_toplevel_configure(stream, toplevel_id, width, height, activated)?;
    send_xdg_surface_configure(stream, xdg_surface_id, serial)?;
    if let Some(xdg_surface) = client.xdg_surfaces.get_mut(&xdg_surface_id) {
        xdg_surface.pending_configure_serial = Some(serial);
    }
    Ok(())
}

fn send_close_for_surface(
    client: &Client,
    stream: &mut UnixStream,
    surface_id: u32,
) -> io::Result<()> {
    let Some((_xdg_surface_id, toplevel_id)) = client.xdg_ids_for_surface(surface_id) else {
        return Ok(());
    };
    send_message(stream, toplevel_id, XDG_TOPLEVEL_CLOSE, &[])
}

fn poll_input_events(client: &mut Client) -> Vec<TwlandInputEvent> {
    let mut events = Vec::new();

    if let Some(mouse) = client.mouse.as_mut() {
        match mouse.poll() {
            Ok(raw) => events.extend(raw.into_iter().map(|event| match event {
                input::MouseEvent::Motion { dx, dy } => TwlandInputEvent::PointerMove { dx, dy },
                input::MouseEvent::Button { button, pressed } => {
                    TwlandInputEvent::PointerButton { button, pressed }
                }
            })),
            Err(error) => eprintln!("twland: mouse read error: {error}"),
        }
    }

    if let Some(keyboard) = client.keyboard.as_mut() {
        match keyboard.poll() {
            Ok(raw) => events.extend(raw.into_iter().map(
                |input::KeyboardEvent { keycode, pressed }| TwlandInputEvent::Key { keycode, pressed },
            )),
            Err(error) => eprintln!("twland: keyboard read error: {error}"),
        }
    }

    events
}

fn dispatch_input_event(
    client: &mut Client,
    stream: &mut UnixStream,
    output: &mut Framebuffer,
    event: TwlandInputEvent,
) -> io::Result<()> {
    match event {
        TwlandInputEvent::PointerMove { dx, dy } => {
            let x = client.input.pointer_x.saturating_add(dx);
            let y = client.input.pointer_y.saturating_add(dy);
            dispatch_pointer_position(client, stream, output, x, y)
        }
        TwlandInputEvent::PointerAbsolute { x, y } => {
            dispatch_pointer_position(client, stream, output, x, y)
        }
        TwlandInputEvent::PointerButton { button, pressed } => {
            dispatch_pointer_button(client, stream, output, button, pressed)
        }
        TwlandInputEvent::Key { keycode, pressed } => {
            dispatch_keyboard_key(client, stream, keycode, pressed)
        }
    }
}

fn dispatch_pointer_position(
    client: &mut Client,
    stream: &mut UnixStream,
    output: &mut Framebuffer,
    x: i32,
    y: i32,
) -> io::Result<()> {
    let max_x = output.width().saturating_sub(1) as i32;
    let max_y = output.height().saturating_sub(1) as i32;
    client.input.pointer_x = x.clamp(0, max_x);
    client.input.pointer_y = y.clamp(0, max_y);

    if let Some(drag) = client.compositor.drag {
        let new_x = drag.window_start_x + client.input.pointer_x - drag.pointer_start_x;
        let new_y = drag.window_start_y + client.input.pointer_y - drag.pointer_start_y;
        if let Some(window) = client.window_for_surface_mut(drag.surface_id) {
            window.x = new_x;
            window.y = new_y;
            window.decoration = decoration_for_window(window.x, window.y, window.width);
            sync_surface_to_window(client, drag.surface_id);
            redraw_scene(client, output)?;
            println!(
                "twland: drag move surface={} pos={},{}",
                drag.surface_id, new_x, new_y
            );
        }
        return Ok(());
    }

    let hit = hit_test(client, client.input.pointer_x, client.input.pointer_y);
    let client_focus = match hit {
        HitTest::ClientArea { surface_id } | HitTest::LayerSurface { surface_id } => {
            Some(surface_id)
        }
        _ => None,
    };
    if client.input.focused_surface != client_focus {
        update_pointer_focus(client, stream, client_focus)?;
        client.input.focused_surface = client_focus;
    }

    if let Some(surface_id) = client_focus {
        let (surface_x, surface_y) = surface_relative_position(
            client,
            surface_id,
            client.input.pointer_x,
            client.input.pointer_y,
        );
        for pointer_id in client.pointer_ids() {
            send_pointer_motion(
                stream,
                pointer_id,
                client.next_serial(),
                surface_x,
                surface_y,
            )?;
        }
        if TWLAND_DEBUG_POINTER_MOTION {
            println!(
                "twland: pointer motion x={} y={}",
                client.input.pointer_x, client.input.pointer_y
            );
        }
    }

    // Redraw so the cursor follows the pointer even when not dragging.
    redraw_scene(client, output)
}

fn dispatch_pointer_button(
    client: &mut Client,
    stream: &mut UnixStream,
    output: &mut Framebuffer,
    button: u32,
    pressed: bool,
) -> io::Result<()> {
    if pressed {
        client.input.buttons |= 1;
    } else {
        client.input.buttons &= !1;
    }

    if !pressed && let Some(drag) = client.compositor.drag.take() {
        println!("twland: end drag surface={}", drag.surface_id);
        redraw_scene(client, output)?;
        return Ok(());
    }

    let hit = hit_test(client, client.input.pointer_x, client.input.pointer_y);
    if pressed {
        match hit {
            HitTest::CloseButton { surface_id } => {
                focus_window(client, stream, output, surface_id, true)?;
                send_close_for_surface(client, stream, surface_id)?;
                println!("twland: close requested surface={surface_id}");
                return Ok(());
            }
            HitTest::Titlebar { surface_id } => {
                focus_window(client, stream, output, surface_id, true)?;
                if let Some(window) = client.window_for_surface(surface_id).cloned() {
                    client.compositor.drag = Some(DragState {
                        surface_id,
                        pointer_start_x: client.input.pointer_x,
                        pointer_start_y: client.input.pointer_y,
                        window_start_x: window.x,
                        window_start_y: window.y,
                    });
                    println!("twland: begin drag surface={surface_id}");
                }
                return Ok(());
            }
            HitTest::Border { surface_id } => {
                focus_window(client, stream, output, surface_id, true)?;
                return Ok(());
            }
            HitTest::ClientArea { surface_id } => {
                focus_window(client, stream, output, surface_id, true)?;
                if client.input.focused_surface != Some(surface_id) {
                    update_pointer_focus(client, stream, Some(surface_id))?;
                    client.input.focused_surface = Some(surface_id);
                }
            }
            HitTest::LayerSurface { surface_id } => {
                if client.input.focused_surface != Some(surface_id) {
                    update_pointer_focus(client, stream, Some(surface_id))?;
                    client.input.focused_surface = Some(surface_id);
                }
                if client
                    .layer_shell
                    .keyboard_interactivity_for_surface(surface_id)
                    .is_some_and(|mode| mode != KeyboardInteractivity::None)
                {
                    update_keyboard_focus(client, stream, Some(surface_id))?;
                }
            }
            HitTest::None => {
                if client.input.focused_surface.is_some() {
                    update_pointer_focus(client, stream, None)?;
                    client.input.focused_surface = None;
                }
                return Ok(());
            }
        }
    } else if !matches!(
        hit,
        HitTest::ClientArea { .. } | HitTest::LayerSurface { .. }
    ) {
        return Ok(());
    }

    if client.input.focused_surface.is_none() {
        return Ok(());
    }
    let state = if pressed {
        WL_POINTER_BUTTON_STATE_PRESSED
    } else {
        WL_POINTER_BUTTON_STATE_RELEASED
    };
    for pointer_id in client.pointer_ids() {
        send_pointer_button(stream, pointer_id, client.next_serial(), button, state)?;
    }
    if TWLAND_DEBUG_INPUT {
        println!("twland: pointer button button={button} pressed={pressed}");
    }
    Ok(())
}

fn dispatch_keyboard_key(
    client: &mut Client,
    stream: &mut UnixStream,
    keycode: u32,
    pressed: bool,
) -> io::Result<()> {
    if client
        .seats
        .values()
        .all(|seat| seat.keyboard_focus.is_none())
    {
        return Ok(());
    }

    let state = if pressed {
        WL_KEYBOARD_KEY_STATE_PRESSED
    } else {
        WL_KEYBOARD_KEY_STATE_RELEASED
    };
    for keyboard_id in client.keyboard_ids() {
        send_keyboard_key(stream, keyboard_id, client.next_serial(), keycode, state)?;
    }
    if TWLAND_DEBUG_INPUT {
        println!("twland: keyboard key keycode={keycode} pressed={pressed}");
    }
    Ok(())
}

fn update_pointer_focus(
    client: &mut Client,
    stream: &mut UnixStream,
    new_focus: Option<u32>,
) -> io::Result<()> {
    let old_focus = client
        .seats
        .values()
        .find_map(|seat| seat.pointer_focus)
        .filter(|old| Some(*old) != new_focus);

    if let Some(surface_id) = old_focus {
        for pointer_id in client.pointer_ids() {
            send_pointer_leave(stream, pointer_id, client.next_serial(), surface_id)?;
        }
        if TWLAND_DEBUG_INPUT {
            println!("twland: pointer leave surface={surface_id}");
        }
    }

    if let Some(surface_id) = new_focus {
        let (surface_x, surface_y) = surface_relative_position(
            client,
            surface_id,
            client.input.pointer_x,
            client.input.pointer_y,
        );
        for pointer_id in client.pointer_ids() {
            send_pointer_enter(
                stream,
                pointer_id,
                client.next_serial(),
                surface_id,
                surface_x,
                surface_y,
            )?;
        }
        if TWLAND_DEBUG_INPUT {
            println!("twland: pointer enter surface={surface_id}");
        }
    }

    for seat in client.seats.values_mut() {
        seat.pointer_focus = new_focus;
    }
    Ok(())
}

fn reconcile_pointer_focus(client: &mut Client, stream: &mut UnixStream) -> io::Result<()> {
    let focus = match hit_test(client, client.input.pointer_x, client.input.pointer_y) {
        HitTest::ClientArea { surface_id } | HitTest::LayerSurface { surface_id } => {
            Some(surface_id)
        }
        _ => None,
    };
    let focus_changed = client.input.focused_surface != focus
        || client
            .seats
            .values()
            .any(|seat| seat.pointer_focus != focus);
    if focus_changed {
        update_pointer_focus(client, stream, focus)?;
        client.input.focused_surface = focus;
    }
    Ok(())
}

fn update_keyboard_focus(
    client: &mut Client,
    stream: &mut UnixStream,
    new_focus: Option<u32>,
) -> io::Result<()> {
    let old_focus = client
        .seats
        .values()
        .find_map(|seat| seat.keyboard_focus)
        .filter(|old| Some(*old) != new_focus);

    if let Some(surface_id) = old_focus {
        for keyboard_id in client.keyboard_ids() {
            send_keyboard_leave(stream, keyboard_id, client.next_serial(), surface_id)?;
        }
        if TWLAND_DEBUG_INPUT {
            println!("twland: keyboard leave surface={surface_id}");
        }
    }

    if let Some(surface_id) = new_focus {
        for keyboard_id in client.keyboard_ids() {
            send_keyboard_enter(stream, keyboard_id, client.next_serial(), surface_id)?;
        }
        if TWLAND_DEBUG_INPUT {
            println!("twland: keyboard enter surface={surface_id}");
        }
    }

    for seat in client.seats.values_mut() {
        seat.keyboard_focus = new_focus;
    }
    Ok(())
}

fn reconcile_keyboard_focus(client: &mut Client, stream: &mut UnixStream) -> io::Result<()> {
    let current_layer_focus = client
        .seats
        .values()
        .find_map(|seat| seat.keyboard_focus)
        .filter(|surface_id| client.layer_shell.can_keep_keyboard_focus(*surface_id));
    let focus = client
        .layer_shell
        .exclusive_keyboard_focus()
        .or(current_layer_focus)
        .or(client.compositor.focused_surface);
    update_keyboard_focus(client, stream, focus)
}

fn surface_relative_position(client: &Client, surface_id: u32, x: i32, y: i32) -> (i32, i32) {
    client
        .surfaces
        .get(&surface_id)
        .map(|surface| (x - surface.x, y - surface.y))
        .unwrap_or((0, 0))
}

fn recv_wayland_message(
    client: &mut Client,
    stream: &mut UnixStream,
) -> io::Result<Option<ReceivedMessage>> {
    if let Some(message) = client.queued_messages.pop_front() {
        return Ok(Some(message));
    }

    let Some((bytes, fds)) = recv_raw(stream)? else {
        return Ok(None);
    };

    // Fds from this read go into the per-client FIFO; request handlers pop them
    // in stream order as their signatures require.
    client.pending_fds.extend(fds);

    // Append to any leftover from a previous read, frame as many complete
    // messages as are available, then retain the unconsumed tail.
    client.residual.extend_from_slice(&bytes);
    let consumed = parse_chunk(&client.residual, &mut client.queued_messages)?;
    client.residual.drain(..consumed);
    Ok(client.queued_messages.pop_front())
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

fn send_xdg_toplevel_configure(
    stream: &mut UnixStream,
    toplevel_id: u32,
    width: i32,
    height: i32,
    activated: bool,
) -> io::Result<()> {
    let mut payload = Vec::new();
    push_i32(&mut payload, width);
    push_i32(&mut payload, height);
    // xdg_toplevel.configure carries an array of state enums. State 2 is
    // "activated" in xdg-shell.
    if activated {
        push_u32(&mut payload, 4);
        push_u32(&mut payload, 2);
    } else {
        push_u32(&mut payload, 0);
    }
    send_message(stream, toplevel_id, XDG_TOPLEVEL_CONFIGURE, &payload)
}

fn send_xdg_surface_configure(
    stream: &mut UnixStream,
    xdg_surface_id: u32,
    serial: u32,
) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, serial);
    send_message(stream, xdg_surface_id, XDG_SURFACE_CONFIGURE, &payload)
}

fn send_seat_capabilities(
    stream: &mut UnixStream,
    seat_id: u32,
    capabilities: u32,
) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, capabilities);
    send_message(stream, seat_id, WL_SEAT_CAPABILITIES, &payload)
}

fn send_seat_name(stream: &mut UnixStream, seat_id: u32, name: &str) -> io::Result<()> {
    let mut payload = Vec::new();
    push_wayland_string(&mut payload, name);
    send_message(stream, seat_id, WL_SEAT_NAME, &payload)
}

fn send_keyboard_keymap(stream: &mut UnixStream, keyboard_id: u32) -> io::Result<()> {
    // wl_keyboard.keymap(format, fd, size): the fd is a Wayland fd argument,
    // so it travels out-of-band via SCM_RIGHTS and occupies zero payload bytes.
    // The payload is just (format, size).  For NO_KEYMAP we still must pass a
    // real (empty) file descriptor; an empty memfd satisfies the protocol
    // without carrying any keymap data.  A later XKB stage should replace this
    // with a memfd holding the compiled keymap string.
    let keymap_fd = create_empty_memfd().map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to create keymap memfd: {err}"),
        )
    })?;
    let size = 0u32;

    let mut payload = Vec::new();
    push_u32(&mut payload, WL_KEYBOARD_KEYMAP_FORMAT_NO_KEYMAP);
    push_u32(&mut payload, size);
    wire::send_message_with_fds(stream, keyboard_id, WL_KEYBOARD_KEYMAP, &payload, &[keymap_fd.as_raw()])
}

fn send_pointer_enter(
    stream: &mut UnixStream,
    pointer_id: u32,
    serial: u32,
    surface_id: u32,
    surface_x: i32,
    surface_y: i32,
) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, serial);
    push_u32(&mut payload, surface_id);
    push_fixed(&mut payload, surface_x);
    push_fixed(&mut payload, surface_y);
    send_message(stream, pointer_id, WL_POINTER_ENTER, &payload)
}

fn send_pointer_leave(
    stream: &mut UnixStream,
    pointer_id: u32,
    serial: u32,
    surface_id: u32,
) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, serial);
    push_u32(&mut payload, surface_id);
    send_message(stream, pointer_id, WL_POINTER_LEAVE, &payload)
}

fn send_pointer_motion(
    stream: &mut UnixStream,
    pointer_id: u32,
    time: u32,
    surface_x: i32,
    surface_y: i32,
) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, time);
    push_fixed(&mut payload, surface_x);
    push_fixed(&mut payload, surface_y);
    send_message(stream, pointer_id, WL_POINTER_MOTION, &payload)
}

fn send_pointer_button(
    stream: &mut UnixStream,
    pointer_id: u32,
    serial: u32,
    button: u32,
    state: u32,
) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, serial);
    push_u32(&mut payload, serial);
    push_u32(&mut payload, button);
    push_u32(&mut payload, state);
    send_message(stream, pointer_id, WL_POINTER_BUTTON, &payload)
}

fn send_keyboard_enter(
    stream: &mut UnixStream,
    keyboard_id: u32,
    serial: u32,
    surface_id: u32,
) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, serial);
    push_u32(&mut payload, surface_id);
    push_u32(&mut payload, 0);
    send_message(stream, keyboard_id, WL_KEYBOARD_ENTER, &payload)
}

fn send_keyboard_leave(
    stream: &mut UnixStream,
    keyboard_id: u32,
    serial: u32,
    surface_id: u32,
) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, serial);
    push_u32(&mut payload, surface_id);
    send_message(stream, keyboard_id, WL_KEYBOARD_LEAVE, &payload)
}

fn send_keyboard_key(
    stream: &mut UnixStream,
    keyboard_id: u32,
    serial: u32,
    keycode: u32,
    state: u32,
) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, serial);
    push_u32(&mut payload, serial);
    push_u32(&mut payload, keycode);
    push_u32(&mut payload, state);
    send_message(stream, keyboard_id, WL_KEYBOARD_KEY, &payload)
}

fn blit_shm_buffer_to_output(
    output: &mut Frame<'_>,
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
    if dst_x0 >= output.width() as i32 || dst_y0 >= output.height() as i32 {
        return Ok(Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        });
    }

    let max_width = (output.width() as i32 - dst_x0).max(0);
    let max_height = (output.height() as i32 - dst_y0).max(0);
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
    let row_bytes = width
        .checked_mul(4)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "source row size overflow"))?;

    if pool.mapped_addr.is_null() || pool.size == 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "shared-memory pool is not mapped",
        ));
    }
    for row in 0..height {
        let src_row = src_y0
            .checked_add(row)
            .and_then(|y| y.checked_mul(stride))
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "source row overflow"))?;
        let src_offset = buffer
            .offset
            .checked_add(src_row)
            .and_then(|offset| offset.checked_add(src_x0 * 4))
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "source offset overflow"))?;
        let src_end = src_offset
            .checked_add(row_bytes)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "source end overflow"))?;
        if src_end > pool.size {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "blit source out of bounds",
            ));
        }

        let dst_offset = dst_y0
            .checked_add(row)
            .and_then(|y| y.checked_mul(output.stride()))
            .and_then(|offset| offset.checked_add(dst_x0 * 4))
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "destination offset overflow"))?;
        // SAFETY: ShmPoolState owns a shared, readable mapping of `pool.size`
        // bytes and cannot be destroyed during this borrow. The bounds check
        // above proves this row is readable. Keeping the source as a raw
        // pointer avoids claiming that client-writable shared memory is an
        // immutable Rust slice.
        unsafe {
            output.copy_from_ptr(dst_offset, pool.mapped_addr.add(src_offset), row_bytes)?;
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
