use core::ffi::c_void;
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind};
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
const BTN_LEFT: u32 = 0x110;
const KEY_SPACE: u32 = 57;
const TWLAND_ALLOW_ROLELESS_DEBUG_SURFACES: bool = false;
const TWLAND_DEBUG_INPUT: bool = true;
const TWLAND_DEBUG_POINTER_MOTION: bool = false;
const TITLEBAR_HEIGHT: i32 = 24;
const BORDER_WIDTH: i32 = 2;
const CLOSE_BUTTON_SIZE: i32 = 18;
const DESKTOP_BACKGROUND: u32 = 0xff101018;
const WINDOW_TEST_APP_ID: &str = "twland-window-test";

const FBIOGET_VSCREENINFO: u64 = 0x4600;
const FBIOGET_FSCREENINFO: u64 = 0x4602;
const FBIOPAN_DISPLAY: u64 = 0x4606;

const PROT_READ: i32 = 0x1;
const PROT_WRITE: i32 = 0x2;
const MAP_SHARED: i32 = 0x01;
const MAP_FAILED: *mut c_void = usize::MAX as *mut c_void;

const SOL_SOCKET: i32 = 1;
const SCM_RIGHTS: i32 = 1;
const MSG_DONTWAIT: i32 = 0x40;
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
    Global {
        name: 5,
        interface: "xdg_wm_base",
        version: 6,
        kind: WaylandObjectKind::XdgWmBase,
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
    fn sendmsg(fd: i32, msg: *const Msghdr, flags: i32) -> isize;
    fn recvmsg(fd: i32, msg: *mut Msghdr, flags: i32) -> isize;
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
    seats: HashMap<u32, SeatState>,
    pointers: HashMap<u32, PointerState>,
    keyboards: HashMap<u32, KeyboardState>,
    xdg_surfaces: HashMap<u32, XdgSurfaceState>,
    xdg_toplevels: HashMap<u32, XdgToplevelState>,
    queued_messages: VecDeque<ReceivedMessage>,
    input: InputState,
    compositor: CompositorState,
    next_serial: u32,
    next_window_offset: i32,
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
    Pointer,
    Keyboard,
    Output,
    XdgWmBase,
    XdgSurface,
    XdgToplevel,
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
    xdg_surface_id: Option<u32>,
    frame_callbacks: Vec<u32>,
    mapped: bool,
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
    synthetic_sent: bool,
}

#[derive(Debug, Clone, Copy)]
enum TwlandInputEvent {
    PointerMove { dx: i32, dy: i32 },
    PointerAbsolute { x: i32, y: i32 },
    PointerButton { button: u32, pressed: bool },
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

/// Runs the compositor and exits with a fatal error message if startup fails.
fn main() {
    if let Err(error) = run() {
        eprintln!("twland: fatal: {error}");
        std::process::exit(1);
    }
}

/// Starts the compositor event loop.
///
/// Creates the runtime directories, opens and clears the framebuffer, binds the Wayland socket,
/// and serves each connecting client until the process exits.
///
/// # Examples
///
/// ```
/// let result = run();
/// assert!(result.is_ok() || result.is_err());
/// ```
fn run() -> io::Result<()> {
    println!("twland: starting");
    println!("twland: runtime dir {RUNTIME_DIR}");
    ensure_dir("/run")?;
    ensure_dir("/run/user")?;
    ensure_dir(RUNTIME_DIR)?;

    let mut output = SoftwareOutput::open()?;
    output.clear(DESKTOP_BACKGROUND)?;
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
                let _ = output.clear(DESKTOP_BACKGROUND);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => eprintln!("twland: accept failed: {error}"),
        }
    }

    Ok(())
}

/// Ensures a directory exists at the given path.
///
/// # Examples
///
/// ```
/// ensure_dir("/tmp/twland");
/// ```
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

/// Processes requests and input events for a single Wayland client connection.
///
/// Reads incoming Wayland messages from the socket, dispatches them to the compositor,
/// and synthesizes any pending input events for the connected client.
///
/// # Errors
///
/// Returns an error if socket I/O fails or if request or input dispatching fails.
///
/// # Examples
///
/// ```
/// # use std::io;
/// # use std::os::unix::net::UnixStream;
/// # fn example(output: &mut SoftwareOutput, stream: UnixStream) -> io::Result<()> {
/// handle_client(stream, output)
/// # }
/// ```
fn handle_client(mut stream: UnixStream, output: &mut SoftwareOutput) -> io::Result<()> {
    let mut client = Client::new();

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

/// Cooperatively yields the CPU for a brief idle period.
///
/// # Examples
///
/// ```
/// compositor_idle();
/// ```
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
    /// Creates a new client with the display object registered.
    ///
    /// # Examples
    ///
    /// ```
    /// let client = Client::new();
    /// assert!(client.objects.contains_key(&1));
    /// ```
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
            seats: HashMap::new(),
            pointers: HashMap::new(),
            keyboards: HashMap::new(),
            xdg_surfaces: HashMap::new(),
            xdg_toplevels: HashMap::new(),
            queued_messages: VecDeque::new(),
            input: InputState {
                pointer_x: 80,
                pointer_y: 80,
                buttons: 0,
                focused_surface: None,
                synthetic_sent: false,
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

    /// Inserts a Wayland object into the client object table.
    ///
    /// # Examples
    ///
    /// ```
    /// let result = client.insert_object(1, WaylandObjectKind::Surface);
    /// assert!(result.is_ok());
    /// ```
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

    /// Generates the next protocol serial number.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut state = State { next_serial: 1 };
    /// let first = state.next_serial();
    /// let second = state.next_serial();
    ///
    /// assert_eq!(first, 1);
    /// assert_eq!(second, 2);
    /// ```
    ///
    /// @returns The current serial value, then advances the next serial and keeps it greater than zero.
    fn next_serial(&mut self) -> u32 {
        let serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1).max(1);
        serial
    }

    /// Removes destroyed shared-memory pools that are no longer referenced by any buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::collections::HashMap;
    /// # struct BufferState { pool_id: u32 }
    /// # struct ShmPoolState { destroyed: bool }
    /// # struct Client {
    /// #     buffers: HashMap<u32, BufferState>,
    /// #     pools: HashMap<u32, ShmPoolState>,
    /// # }
    /// # impl Client {
    /// #     fn cleanup_destroyed_pools(&mut self) {
    /// #         let live_pool_ids = self
    /// #             .buffers
    /// #             .values()
    /// #             .map(|buffer| buffer.pool_id)
    /// #             .collect::<Vec<_>>();
    /// #         self.pools
    /// #             .retain(|id, pool| !pool.destroyed || live_pool_ids.contains(id));
    /// #     }
    /// # }
    /// let mut client = Client {
    ///     buffers: HashMap::from([(1, BufferState { pool_id: 10 })]),
    ///     pools: HashMap::from([
    ///         (10, ShmPoolState { destroyed: true }),
    ///         (11, ShmPoolState { destroyed: true }),
    ///     ]),
    /// };
    ///
    /// client.cleanup_destroyed_pools();
    ///
    /// assert!(client.pools.contains_key(&10));
    /// assert!(!client.pools.contains_key(&11));
    /// ```
    fn cleanup_destroyed_pools(&mut self) {
        let live_pool_ids = self
            .buffers
            .values()
            .map(|buffer| buffer.pool_id)
            .collect::<Vec<_>>();
        self.pools
            .retain(|id, pool| !pool.destroyed || live_pool_ids.contains(id));
    }

    /// Chooses the next default position for a new window.
    ///
    /// # Returns
    ///
    /// The `(x, y)` coordinates for the next window origin.
    #[example]
    /// ```
    /// let pos = client.next_window_position();
    /// assert!(pos.0 >= 60 && pos.1 >= 60);
    /// ```
    fn next_window_position(&mut self) -> (i32, i32) {
        let offset = self.next_window_offset;
        self.next_window_offset = (self.next_window_offset + 40).min(320);
        (60 + offset, 60 + offset)
    }

    /// Collects the IDs of pointers whose seat is still present.
    ///
    /// # Examples
    ///
    /// ```
    /// let ids = client.pointer_ids();
    /// assert!(ids.contains(&pointer_id));
    /// ```
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

    /// Collects the IDs of keyboards whose seat is still present.
    ///
    /// # Examples
    ///
    /// ```
    /// let ids = client.keyboard_ids();
    /// assert!(ids.iter().all(|id| client.seats.contains_key(&client.keyboards[id].seat_id)));
    /// ```
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

    /// Finds the surface ID of the first mapped window.
    ///
    /// # Examples
    ///
    /// ```
    /// let surface_id = first_mapped_surface();
    /// assert_eq!(surface_id, Some(1));
    /// ```
    ///
    /// @returns `Some(surface_id)` for the first mapped window, or `None` if no mapped window exists.
    fn first_mapped_surface(&self) -> Option<u32> {
        self.compositor
            .windows
            .iter()
            .find(|window| window.mapped)
            .map(|window| window.surface_id)
    }

    /// Gets the size of the buffer attached to a surface.
    ///
    /// # Examples
    ///
    /// ```
    /// let size = client.surface_size(surface_id);
    /// assert_eq!(size, Some((400, 300)));
    /// ```
    ///
    /// @returns The attached buffer size as `(width, height)` if the surface and buffer exist, `None` otherwise.
    fn surface_size(&self, surface_id: u32) -> Option<(i32, i32)> {
        let surface = self.surfaces.get(&surface_id)?;
        let buffer_id = surface.attached_buffer.or(surface.pending_buffer)?;
        let buffer = self.buffers.get(&buffer_id)?;
        Some((buffer.width, buffer.height))
    }

    /// Finds the window associated with a surface ID.
    ///
    /// # Examples
    ///
    /// ```
    /// let window = window_for_surface(42);
    /// assert!(window.is_none());
    /// ```
    fn window_for_surface(&self, surface_id: u32) -> Option<&Window> {
        self.compositor
            .windows
            .iter()
            .find(|window| window.surface_id == surface_id)
    }

    /// Finds the window associated with a surface.
    ///
    /// # Returns
    ///
    /// A mutable reference to the matching window, or `None` if no window uses the
    /// given surface ID.
    ///
    /// # Examples
    ///
    /// ```
    /// let window = compositor.window_for_surface_mut(surface_id);
    /// assert!(window.is_some());
    /// ```
    fn window_for_surface_mut(&mut self, surface_id: u32) -> Option<&mut Window> {
        self.compositor
            .windows
            .iter_mut()
            .find(|window| window.surface_id == surface_id)
    }

    /// Gets the xdg surface and toplevel IDs for a surface.
    ///
    /// # Examples
    ///
    /// ```
    /// if let Some((xdg_surface_id, toplevel_id)) = xdg_ids_for_surface(surface_id) {
    ///     assert!(xdg_surface_id > 0);
    ///     assert!(toplevel_id > 0);
    /// }
    /// ```
    fn xdg_ids_for_surface(&self, surface_id: u32) -> Option<(u32, u32)> {
        let xdg_surface_id = self.surfaces.get(&surface_id)?.xdg_surface_id?;
        let role = self.xdg_surfaces.get(&xdg_surface_id)?.role?;
        let XdgRole::Toplevel(toplevel_id) = role;
        Some((xdg_surface_id, toplevel_id))
    }
}

/// Dispatches a Wayland request to the handler for its object type and opcode.
///
/// # Examples
///
/// ```
/// let _ = dispatch_request(&mut client, &mut output, &mut stream, message);
/// ```
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
        (WaylandObjectKind::Seat, WL_SEAT_GET_POINTER) => {
            handle_seat_get_pointer(client, message.header.object_id, &message.payload)
        }
        (WaylandObjectKind::Seat, WL_SEAT_GET_KEYBOARD) => {
            handle_seat_get_keyboard(client, stream, message.header.object_id, &message.payload)
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
            handle_surface_destroy(client, output, message.header.object_id)?;
            Ok(())
        }
        (WaylandObjectKind::Surface, WL_SURFACE_ATTACH) => {
            handle_surface_attach(client, message.header.object_id, &message.payload)
        }
        (WaylandObjectKind::Surface, WL_SURFACE_DAMAGE) => {
            handle_surface_damage(client, message.header.object_id, &message.payload)
        }
        (WaylandObjectKind::Surface, WL_SURFACE_FRAME) => {
            handle_surface_frame(client, message.header.object_id, &message.payload)
        }
        (WaylandObjectKind::Surface, WL_SURFACE_COMMIT) => {
            handle_surface_commit(client, output, stream, message.header.object_id)
        }
        (WaylandObjectKind::XdgWmBase, XDG_WM_BASE_PONG) => {
            handle_xdg_wm_base_pong(&message.payload)
        }
        (WaylandObjectKind::XdgWmBase, XDG_WM_BASE_GET_XDG_SURFACE) => {
            handle_xdg_wm_base_get_xdg_surface(client, message.header.object_id, &message.payload)
        }
        (WaylandObjectKind::XdgSurface, XDG_SURFACE_DESTROY) => {
            handle_xdg_surface_destroy(client, output, message.header.object_id)?;
            Ok(())
        }
        (WaylandObjectKind::XdgSurface, XDG_SURFACE_GET_TOPLEVEL) => {
            handle_xdg_surface_get_toplevel(client, message.header.object_id, &message.payload)
        }
        (WaylandObjectKind::XdgSurface, XDG_SURFACE_ACK_CONFIGURE) => {
            handle_xdg_surface_ack_configure(client, message.header.object_id, &message.payload)
        }
        (WaylandObjectKind::XdgToplevel, XDG_TOPLEVEL_SET_TITLE) => {
            handle_xdg_toplevel_set_title(client, message.header.object_id, &message.payload)
        }
        (WaylandObjectKind::XdgToplevel, XDG_TOPLEVEL_SET_APP_ID) => {
            handle_xdg_toplevel_set_app_id(client, message.header.object_id, &message.payload)
        }
        (WaylandObjectKind::XdgToplevel, XDG_TOPLEVEL_DESTROY) => {
            handle_xdg_toplevel_destroy(client, output, message.header.object_id)?;
            Ok(())
        }
        (WaylandObjectKind::XdgToplevel, opcode) => {
            println!(
                "twland: xdg_toplevel request opcode={opcode} ignored for now object={}",
                message.header.object_id
            );
            Ok(())
        }
        (kind, opcode) => Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("unsupported request kind={kind:?} opcode={opcode}"),
        )),
    }
}

/// Completes a `wl_display.sync` request with a callback serial.
///
/// # Examples
///
/// ```
/// let callback_id = 42;
/// // A sync request installs a callback and later sends `wl_callback.done`.
/// ```
fn handle_display_sync(
client: &mut Client,
stream: &mut UnixStream,
payload: &[u8],
) -> io::Result<()> {
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

/// Creates a registry object and advertises the compositor globals.

///

/// # Examples

///

/// ```

/// handle_get_registry(client, stream, payload)?;

/// # Ok::<(), std::io::Error>(())

/// ```
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

/// Binds a Wayland global and initializes any associated compositor state.
///
/// # Examples
///
/// ```
/// # use std::io;
/// # use std::os::unix::net::UnixStream;
/// # fn example(client: &mut Client, stream: &mut UnixStream, registry_id: u32, payload: &[u8]) -> io::Result<()> {
/// handle_registry_bind(client, stream, registry_id, payload)
/// # }
/// ```
///
/// @returns `Ok(())` if the global was bound successfully.
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

    Ok(())
}

/// Creates a new Wayland surface resource with default compositor state.
///
/// # Examples
///
/// ```
/// # let mut client = Client::new();
/// # let payload = 1u32.to_le_bytes();
/// handle_compositor_create_surface(&mut client, &payload).unwrap();
/// ```
```
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
            xdg_surface_id: None,
            frame_callbacks: Vec::new(),
            mapped: false,
        },
    );
    println!("twland: wl_compositor.create_surface id={surface_id}");
    Ok(())
}

/// Creates a pointer resource for a seat and records the association.
///
/// # Examples
///
/// ```
/// let _ = handle_seat_get_pointer(&mut client, seat_id, payload);
/// ```
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

/// Creates a keyboard resource for a seat and sends its keymap.
///
/// # Examples
///
/// ```
/// let result = handle_seat_get_keyboard(&mut client, &mut stream, seat_id, payload);
/// assert!(result.is_ok());
/// ```
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

/// Creates a shared-memory pool from a passed file descriptor.
///
/// The payload must contain a pool ID and a positive size, and the request must
/// include one `SCM_RIGHTS` file descriptor.
///
/// # Parameters
///
/// * `payload` - Wayland request payload containing the pool ID and size.
/// * `fds` - Ancillary file descriptors passed with the request.
///
/// # Examples
///
/// ```no_run
/// # let mut client = Client::default();
/// # let payload = vec![0, 0, 0, 1, 0, 0, 0, 4096];
/// # let fds = vec![OwnedFdRaw::new(3)];
/// let _ = handle_shm_create_pool(&mut client, payload, fds);
/// ```
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

/// Creates a shared-memory buffer resource from a Wayland SHM pool.
///
/// # Returns
///
/// `Ok(())` when the buffer is registered successfully, or an error if the
/// request is invalid or the buffer exceeds the pool.
///
/// # Examples
///
/// ```
/// let result = handle_shm_pool_create_buffer(&mut client, pool_id, payload);
/// assert!(result.is_ok());
/// ```
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

/// Destroys a shared-memory pool and releases any associated resources.
///
/// # Examples
///
/// ```
/// handle_shm_pool_destroy(&mut client, pool_id);
/// ```
fn handle_shm_pool_destroy(client: &mut Client, pool_id: u32) {
    client.objects.remove(&pool_id);
    if let Some(pool) = client.pools.get_mut(&pool_id) {
        pool.destroyed = true;
    }
    client.cleanup_destroyed_pools();
    println!("twland: wl_shm_pool.destroy id={pool_id}");
}

/// Destroys a shared-memory buffer resource and releases any pools no longer in use.
///
/// # Examples
///
/// ```
/// let buffer_id = 42;
/// handle_buffer_destroy(&mut client, buffer_id);
/// ```
```
fn handle_buffer_destroy(client: &mut Client, buffer_id: u32) {
    client.objects.remove(&buffer_id);
    client.buffers.remove(&buffer_id);
    client.cleanup_destroyed_pools();
    println!("twland: wl_buffer.destroy id={buffer_id}");
}

/// Destroys a surface and clears any compositor state tied to it.
///
/// # Examples
///
/// ```
/// handle_surface_destroy(&mut client, &mut output, surface_id)?;
/// # Ok::<(), std::io::Error>(())
/// ```
fn handle_surface_destroy(
    client: &mut Client,
    output: &mut SoftwareOutput,
    surface_id: u32,
) -> io::Result<()> {
    client.objects.remove(&surface_id);
    client.surfaces.remove(&surface_id);
    remove_window_for_surface(client, surface_id);
    if client.input.focused_surface == Some(surface_id) {
        client.input.focused_surface = None;
    }
    for seat in client.seats.values_mut() {
        if seat.pointer_focus == Some(surface_id) {
            seat.pointer_focus = None;
        }
        if seat.keyboard_focus == Some(surface_id) {
            seat.keyboard_focus = None;
        }
    }
    println!("twland: wl_surface.destroy id={surface_id}");
    redraw_scene(client, output)?;
    Ok(())
}

/// Attaches a buffer to a surface with the given offset.
///
/// # Examples
///
/// ```
/// let mut client = Client::default();
/// let surface_id = 1;
/// client.surfaces.insert(surface_id, SurfaceState::default());
/// client.buffers.insert(2, BufferState::default());
///
/// let payload = [
///     2u32.to_le_bytes(),
///     10i32.to_le_bytes(),
///     20i32.to_le_bytes(),
/// ]
/// .concat();
///
/// handle_surface_attach(&mut client, surface_id, &payload).unwrap();
/// ```
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

/// Records damaged surface bounds for the next redraw.
///
/// If the new damage overlaps existing damage, the stored region expands to cover both.
///
/// # Examples
///
/// ```
/// let mut client = Client::default();
/// let surface_id = 1;
/// client.surfaces.insert(surface_id, SurfaceState::default());
///
/// let payload = [
///     0i32.to_le_bytes(),
///     0i32.to_le_bytes(),
///     100i32.to_le_bytes(),
///     50i32.to_le_bytes(),
/// ].concat();
///
/// handle_surface_damage(&mut client, surface_id, &payload).unwrap();
/// assert!(client.surfaces.get(&surface_id).unwrap().damage.is_some());
/// ```
/**
* @returns `Ok(())` if the damage was recorded, or an error if the payload is invalid or the surface is unknown.
*/
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

/// Registers a frame callback for a surface.

///

/// # Examples

///

/// ```

/// # let mut client = Client::default();

/// # let surface_id = 1;

/// # client.surfaces.insert(surface_id, SurfaceState::default());

/// let payload = 42u32.to_le_bytes();

/// handle_surface_frame(&mut client, surface_id, &payload).unwrap();

/// assert_eq!(client.surfaces.get(&surface_id).unwrap().frame_callbacks, vec![42]);

/// ```
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

/// Handles an `xdg_wm_base.pong` request.
///
/// # Examples
///
/// ```
/// let payload = 1u32.to_le_bytes();
/// handle_xdg_wm_base_pong(&payload).unwrap();
/// ```
fn handle_xdg_wm_base_pong(payload: &[u8]) -> io::Result<()> {
    let serial = read_u32(payload, 0)?;
    println!("twland: xdg_wm_base.pong serial={serial}");
    Ok(())
}

/// Creates an `xdg_surface` for a Wayland surface.
///
/// # Examples
///
/// ```
/// let result = handle_xdg_wm_base_get_xdg_surface(&mut client, 1, payload);
/// assert!(result.is_ok());
/// ```
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
    if surface.xdg_surface_id.is_some() {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "wl_surface already has an xdg role",
        ));
    }

    surface.xdg_surface_id = Some(xdg_surface_id);
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

/// Assigns a toplevel role to an `xdg_surface` and creates its toplevel state.
///
/// # Examples
///
/// ```
/// let _ = handle_xdg_surface_get_toplevel(&mut client, xdg_surface_id, payload);
/// ```
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

/// Acknowledges the pending `xdg_surface.configure` serial for a surface.
///
/// # Examples
///
/// ```
/// let _ = handle_xdg_surface_ack_configure(&mut client, xdg_surface_id, &serial_payload);
/// ```
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

/// Sets the title for an `xdg_toplevel` and updates the matching window title.
///
/// # Parameters
///
/// * `toplevel_id` - The `xdg_toplevel` resource to update.
/// * `payload` - Wayland string payload containing the new title.
///
/// # Examples
///
/// ```
/// let _ = handle_xdg_toplevel_set_title(&mut client, toplevel_id, payload);
/// ```
fn handle_xdg_toplevel_set_title(
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
    {
        if let Some(window) = client.window_for_surface_mut(surface_id) {
            window.title = title.clone();
        }
    }
    println!("twland: xdg_toplevel.set_title \"{}\"", title);
    Ok(())
}

/// Sets the application identifier for a toplevel surface.
///
/// # Examples
///
/// ```
/// let _ = handle_xdg_toplevel_set_app_id(&mut client, toplevel_id, payload);
/// ```
```
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
    {
        if let Some(window) = client.window_for_surface_mut(surface_id) {
            window.app_id = app_id.clone();
        }
    }
    println!("twland: xdg_toplevel.set_app_id \"{}\"", app_id);
    Ok(())
}

/// Destroys an `xdg_surface` and clears its associated compositor state.
///
/// # Examples
///
/// ```
/// handle_xdg_surface_destroy(&mut client, &mut output, xdg_surface_id)?;
/// ```
fn handle_xdg_surface_destroy(
    client: &mut Client,
    output: &mut SoftwareOutput,
    xdg_surface_id: u32,
) -> io::Result<()> {
    client.objects.remove(&xdg_surface_id);
    if let Some(xdg_surface) = client.xdg_surfaces.remove(&xdg_surface_id) {
        if let Some(XdgRole::Toplevel(toplevel_id)) = xdg_surface.role {
            client.objects.remove(&toplevel_id);
            client.xdg_toplevels.remove(&toplevel_id);
        }
        if let Some(surface) = client.surfaces.get_mut(&xdg_surface.wl_surface_id) {
            surface.xdg_surface_id = None;
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

/// Destroys an `xdg_toplevel` resource and clears its associated window state.
///
/// # Examples
///
/// ```
/// handle_xdg_toplevel_destroy(&mut client, &mut output, toplevel_id).unwrap();
/// ```
fn handle_xdg_toplevel_destroy(
    client: &mut Client,
    output: &mut SoftwareOutput,
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

/// Commits the current state of a surface and redraws the scene.
///
/// When the surface has an `xdg_surface` role, this requires the surface to be
/// configured before a buffer can be committed. Frame callbacks registered on
/// the surface are completed after the commit is applied.
///
/// # Examples
///
/// ```
/// handle_surface_commit(&mut client, &mut output, &mut stream, surface_id)?;
/// # Ok::<(), std::io::Error>(())
/// ```
```
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

    if surface.pending_buffer.is_none() && surface.attached_buffer.is_none() {
        if let Some(xdg_surface_id) = surface.xdg_surface_id {
            handle_initial_xdg_empty_commit(client, stream, surface_id, xdg_surface_id)?;
        } else {
            println!("twland: wl_surface.commit surface={surface_id} no-buffer");
        }
        client.surfaces.insert(surface_id, surface);
        return Ok(());
    }

    if surface.pending_buffer.is_some() {
        surface.attached_buffer = surface.pending_buffer;
        surface.pending_buffer = None;
    }

    let Some(buffer_id) = surface.attached_buffer else {
        client.surfaces.insert(surface_id, surface);
        println!("twland: wl_surface.commit surface={surface_id} no-buffer");
        return Ok(());
    };

    let xdg_info = if let Some(xdg_surface_id) = surface.xdg_surface_id {
        let xdg_surface = client
            .xdg_surfaces
            .get(&xdg_surface_id)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "unknown xdg_surface"))?;
        if !xdg_surface.configured {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "xdg_surface buffer committed before ack_configure",
            ));
        }
        xdg_surface.role
    } else {
        if !TWLAND_ALLOW_ROLELESS_DEBUG_SURFACES {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "roleless wl_surface commit is disabled; use xdg-shell",
            ));
        }
        None
    };

    let buffer = client.buffers.get(&buffer_id).cloned().ok_or_else(|| {
        io::Error::new(ErrorKind::InvalidData, "commit references unknown buffer")
    })?;

    if let Some(XdgRole::Toplevel(toplevel_id)) = xdg_info {
        if !surface.mapped {
            if let Some(toplevel) = client.xdg_toplevels.get(&toplevel_id) {
                println!(
                    "twland: mapped xdg_toplevel title=\"{}\" app_id=\"{}\"",
                    toplevel.title, toplevel.app_id
                );
            }
            surface.mapped = true;
        }
        map_or_update_window(client, surface_id, toplevel_id, &buffer);
    } else if TWLAND_ALLOW_ROLELESS_DEBUG_SURFACES {
        surface.mapped = true;
    }

    surface.damage = None;
    let callbacks = std::mem::take(&mut surface.frame_callbacks);
    client.surfaces.insert(surface_id, surface);
    redraw_scene(client, output)?;
    send_message(stream, buffer_id, WL_BUFFER_RELEASE, &[])?;

    for callback_id in callbacks {
        let mut payload = Vec::new();
        push_u32(&mut payload, client.next_serial());
        send_message(stream, callback_id, WL_CALLBACK_DONE, &payload)?;
        client.objects.remove(&callback_id);
    }

    println!(
        "twland: wl_surface.commit surface={surface_id} buffer={buffer_id} redraw={}x{}",
        buffer.width, buffer.height
    );
    Ok(())
}

/// Sends the initial `xdg_toplevel` and `xdg_surface` configure events for an empty toplevel commit.
///
/// # Parameters
///
/// - `surface_id`: The `wl_surface` associated with the commit.
/// - `xdg_surface_id`: The linked `xdg_surface` resource.
///
/// # Examples
///
/// ```
/// # use std::io;
/// # use std::os::unix::net::UnixStream;
/// # fn demo(client: &mut Client, stream: &mut UnixStream, surface_id: u32, xdg_surface_id: u32) -> io::Result<()> {
/// handle_initial_xdg_empty_commit(client, stream, surface_id, xdg_surface_id)
/// # }
/// ```
///
/// `Ok(())` after sending the initial configure sequence.
///
/// `Err` if the `xdg_surface` is unknown, has no toplevel role, or the linked
/// `xdg_toplevel` resource is missing.
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

/// Maps a surface to a compositor window or updates the existing window state.
///
/// # Examples
///
/// ```
/// let buffer = BufferState {
///     pool_id: 1,
///     offset: 0,
///     width: 400,
///     height: 300,
///     stride: 1600,
///     format: WL_SHM_FORMAT_ARGB8888,
/// };
/// map_or_update_window(&mut client, surface_id, toplevel_id, &buffer);
/// ```
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

/// Builds window decoration geometry for a window at the given position.
///
/// # Examples
///
/// ```
/// let decoration = decoration_for_window(40, 40, 400);
/// assert!(decoration.enabled);
/// assert_eq!(decoration.titlebar_height, TITLEBAR_HEIGHT);
/// assert_eq!(decoration.border_width, BORDER_WIDTH);
/// ```
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

/// Synchronizes a surface's client-area position with its window decoration.
///
/// # Examples
///
/// ```
/// sync_surface_to_window(&mut client, surface_id);
/// ```
fn sync_surface_to_window(client: &mut Client, surface_id: u32) {
    let Some(window) = client.window_for_surface(surface_id).cloned() else {
        return;
    };
    if let Some(surface) = client.surfaces.get_mut(&surface_id) {
        surface.x = window.x + window.decoration.border_width;
        surface.y = window.y + window.decoration.titlebar_height + window.decoration.border_width;
    }
}

/// Removes the window associated with a surface and updates focus state.
///
/// # Examples
///
/// ```
/// remove_window_for_surface(&mut client, surface_id);
/// assert!(!client.compositor.windows.iter().any(|window| window.surface_id == surface_id));
/// ```
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

/// Redraws all mapped windows onto the framebuffer.
///
/// # Examples
///
/// ```
/// redraw_scene(&mut client, &mut output).unwrap();
/// ```
fn redraw_scene(client: &mut Client, output: &mut SoftwareOutput) -> io::Result<()> {
    output.clear(DESKTOP_BACKGROUND)?;
    let windows = client.compositor.windows.clone();
    for window in windows.iter().filter(|window| window.mapped) {
        draw_window_decoration(output, window)?;
        let Some(surface) = client.surfaces.get(&window.surface_id).cloned() else {
            continue;
        };
        let Some(buffer_id) = surface.attached_buffer else {
            continue;
        };
        let Some(buffer) = client.buffers.get(&buffer_id).cloned() else {
            continue;
        };
        let Some(pool) = client.pools.get(&buffer.pool_id) else {
            continue;
        };
        let mut draw_surface = surface;
        draw_surface.x = window.x + window.decoration.border_width;
        draw_surface.y =
            window.y + window.decoration.titlebar_height + window.decoration.border_width;
        draw_surface.attach_x = 0;
        draw_surface.attach_y = 0;
        let damage = Rect {
            x: 0,
            y: 0,
            width: buffer.width,
            height: buffer.height,
        };
        let _ = blit_shm_buffer_to_output(output, pool, &buffer, &draw_surface, damage)?;
    }
    output.sync()
}

/// Draws the decoration for a window.
///
/// # Examples
///
/// ```
/// draw_window_decoration(&mut output, &window).unwrap();
/// ```
fn draw_window_decoration(output: &mut SoftwareOutput, window: &Window) -> io::Result<()> {
fn draw_window_decoration(output: &mut SoftwareOutput, window: &Window) -> io::Result<()> {
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

/// Draws an X-shaped close glyph inside a rectangle.
///
/// # Examples
///
/// ```no_run
/// # use std::io;
/// # fn demo(mut output: SoftwareOutput, rect: Rect) -> io::Result<()> {
/// draw_close_glyph(&mut output, rect)?;
/// # Ok(())
/// # }
/// ```
fn draw_close_glyph(output: &mut SoftwareOutput, rect: Rect) -> io::Result<()> {
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

/// Determines which part of the topmost mapped window is under a point.
///
/// # Examples
///
/// ```
/// let hit = hit_test(&state, 100, 50);
/// match hit {
///     HitTest::None => {}
///     _ => {}
/// }
/// ```
fn hit_test(state: &CompositorState, x: i32, y: i32) -> HitTest {
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

/// Determines whether a point lies within a rectangle.
///
/// # Examples
///
/// ```
/// let rect = Rect { x: 10, y: 20, width: 30, height: 40 };
/// assert!(rect_contains(rect, 15, 25));
/// assert!(!rect_contains(rect, 5, 25));
/// ```
fn rect_contains(rect: Rect, x: i32, y: i32) -> bool {
    x >= rect.x
        && y >= rect.y
        && x < rect.x.saturating_add(rect.width)
        && y < rect.y.saturating_add(rect.height)
}

/// Focuses a window, raises it to the top of the stack, and updates input focus.
///
/// # Examples
///
/// ```
/// focus_window(client, stream, output, surface_id, true).unwrap();
/// ```
fn focus_window(
client: &mut Client,
stream: &mut UnixStream,
output: &mut SoftwareOutput,
surface_id: u32,
send_configure: bool,
) -> io::Result<()> {
fn focus_window(
    client: &mut Client,
    stream: &mut UnixStream,
    output: &mut SoftwareOutput,
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

    update_keyboard_focus(client, stream, Some(surface_id))?;
    if send_configure {
        if let Some(old) = old_focus {
            send_focus_configure(client, stream, old, false)?;
        }
        send_focus_configure(client, stream, surface_id, true)?;
    }
    println!("twland: focus changed old={old_focus:?} new={surface_id}");
    redraw_scene(client, output)
}

/// Sends a configure event for a mapped xdg toplevel.
///
/// # Examples
///
/// ```
/// let _ = send_focus_configure(&mut client, &mut stream, surface_id, true);
/// ```
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

/// Sends a close request for a surface's toplevel role.
///
/// # Examples
///
/// ```
/// let result = send_close_for_surface(&client, &mut stream, surface_id);
/// assert!(result.is_ok());
/// ```
///
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

/// Generates a one-time sequence of synthetic input events for testing.

///

/// Returns an empty list until a seat has both pointer and keyboard devices, and only emits the events once.
fn poll_input_events(client: &mut Client) -> Vec<TwlandInputEvent> {
    if client.input.synthetic_sent {
        return Vec::new();
    }
    if !client
        .seats
        .values()
        .any(|seat| seat.pointer_id.is_some() && seat.keyboard_id.is_some())
    {
        return Vec::new();
    }

    if client
        .compositor
        .windows
        .iter()
        .any(|window| window.app_id == WINDOW_TEST_APP_ID)
    {
        if client.compositor.windows.len() < 2 {
            return Vec::new();
        }
        client.input.synthetic_sent = true;
        let window = client.compositor.windows[0].clone();
        let client_x = window.x + window.decoration.border_width + 12;
        let client_y =
            window.y + window.decoration.titlebar_height + window.decoration.border_width + 12;
        let title_x = window.x + 24;
        let title_y = window.y + window.decoration.border_width + 10;
        let close_x = window.decoration.close_button_rect.x + CLOSE_BUTTON_SIZE / 2 + 40;
        let close_y = window.decoration.close_button_rect.y + CLOSE_BUTTON_SIZE / 2 + 20;
        return vec![
            TwlandInputEvent::PointerAbsolute {
                x: client_x,
                y: client_y,
            },
            TwlandInputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: true,
            },
            TwlandInputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: false,
            },
            TwlandInputEvent::PointerAbsolute {
                x: title_x,
                y: title_y,
            },
            TwlandInputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: true,
            },
            TwlandInputEvent::PointerMove { dx: 40, dy: 20 },
            TwlandInputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: false,
            },
            TwlandInputEvent::PointerAbsolute {
                x: close_x,
                y: close_y,
            },
            TwlandInputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: true,
            },
            TwlandInputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: false,
            },
            TwlandInputEvent::Key {
                keycode: KEY_SPACE,
                pressed: true,
            },
            TwlandInputEvent::Key {
                keycode: KEY_SPACE,
                pressed: false,
            },
        ];
    }

    let Some(surface_id) = client.first_mapped_surface() else {
        return Vec::new();
    };
    let Some(window) = client.window_for_surface(surface_id).cloned() else {
        return Vec::new();
    };

    client.input.synthetic_sent = true;
    let x = window.x + window.decoration.border_width + 24;
    let y = window.y + window.decoration.titlebar_height + window.decoration.border_width + 24;
    vec![
        TwlandInputEvent::PointerAbsolute { x, y },
        TwlandInputEvent::PointerMove { dx: 12, dy: 8 },
        TwlandInputEvent::PointerButton {
            button: BTN_LEFT,
            pressed: true,
        },
        TwlandInputEvent::PointerButton {
            button: BTN_LEFT,
            pressed: false,
        },
        TwlandInputEvent::Key {
            keycode: KEY_SPACE,
            pressed: true,
        },
        TwlandInputEvent::Key {
            keycode: KEY_SPACE,
            pressed: false,
        },
    ]
}

/// Dispatches a synthesized input event to the compositor.
///
/// # Examples
///
/// ```
/// let event = TwlandInputEvent::PointerAbsolute { x: 100, y: 100 };
/// dispatch_input_event(&mut client, &mut stream, &mut output, event)?;
/// # Ok::<(), std::io::Error>(())
/// ```
fn dispatch_input_event(
client: &mut Client,
stream: &mut UnixStream,
output: &mut SoftwareOutput,
event: TwlandInputEvent,
) -> io::Result<()> {
fn dispatch_input_event(
    client: &mut Client,
    stream: &mut UnixStream,
    output: &mut SoftwareOutput,
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

/// Updates pointer coordinates and dispatches pointer motion to the focused surface.
///
/// When a window drag is active, this updates the dragged window position instead of
/// sending motion events.
///
/// # Returns
///
/// `Ok(())` on success.
///
/// # Examples
///
/// ```
/// # let _ = dispatch_pointer_position(&mut client, &mut stream, &mut output, 10, 20);
/// ```
fn dispatch_pointer_position(
    client: &mut Client,
    stream: &mut UnixStream,
    output: &mut SoftwareOutput,
    x: i32,
    y: i32,
) -> io::Result<()> {
    let max_x = output.width.saturating_sub(1) as i32;
    let max_y = output.height.saturating_sub(1) as i32;
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

    let hit = hit_test(
        &client.compositor,
        client.input.pointer_x,
        client.input.pointer_y,
    );
    let client_focus = match hit {
        HitTest::ClientArea { surface_id } => Some(surface_id),
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

    Ok(())
}

/// Handles a pointer button event and updates focus, dragging, or close requests.
///
/// # Examples
///
/// ```
/// dispatch_pointer_button(&mut client, &mut stream, &mut output, 1, true)?;
/// ```
fn dispatch_pointer_button(
client: &mut Client,
stream: &mut UnixStream,
output: &mut SoftwareOutput,
button: u32,
pressed: bool,
) -> io::Result<()> {
fn dispatch_pointer_button(
    client: &mut Client,
    stream: &mut UnixStream,
    output: &mut SoftwareOutput,
    button: u32,
    pressed: bool,
) -> io::Result<()> {
    if pressed {
        client.input.buttons |= 1;
    } else {
        client.input.buttons &= !1;
    }

    if !pressed {
        if let Some(drag) = client.compositor.drag.take() {
            println!("twland: end drag surface={}", drag.surface_id);
            redraw_scene(client, output)?;
            return Ok(());
        }
    }

    let hit = hit_test(
        &client.compositor,
        client.input.pointer_x,
        client.input.pointer_y,
    );
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
            HitTest::None => {
                if client.input.focused_surface.is_some() {
                    update_pointer_focus(client, stream, None)?;
                    client.input.focused_surface = None;
                }
                return Ok(());
            }
        }
    } else if !matches!(hit, HitTest::ClientArea { .. }) {
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

/// Sends a keyboard key event to the focused surface.
///
/// # Examples
///
/// ```
/// dispatch_keyboard_key(&mut client, &mut stream, 30, true)?;
/// ```
fn dispatch_keyboard_key(
    client: &mut Client,
    stream: &mut UnixStream,
    keycode: u32,
    pressed: bool,
) -> io::Result<()> {
    if client.compositor.focused_surface.is_none() {
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

/// Updates pointer focus for all seats and emits the corresponding enter and leave events.
///
/// # Examples
///
/// ```
/// let _ = update_pointer_focus(&mut client, &mut stream, Some(surface_id));
/// ```
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

/// Updates keyboard focus for all seats and sends the corresponding Wayland enter and leave events.
///
/// # Examples
///
/// ```
/// update_keyboard_focus(&mut client, &mut stream, Some(surface_id))?;
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// @param new_focus The surface to receive keyboard focus, or `None` to clear focus.
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

/// Converts output coordinates into coordinates relative to a surface.
///
/// # Examples
///
/// ```
/// let pos = surface_relative_position(&client, surface_id, 120, 80);
/// assert_eq!(pos, (20, 40));
/// ```
fn surface_relative_position(client: &Client, surface_id: u32, x: i32, y: i32) -> (i32, i32) {
    client
        .surfaces
        .get(&surface_id)
        .map(|surface| (x - surface.x, y - surface.y))
        .unwrap_or((0, 0))
}

/// Receives the next Wayland message from the client socket.
///
/// Returns a queued message first if one is already available, otherwise reads
/// from the socket and parses the received wire data into one or more messages.
///
/// # Examples
///
/// ```
/// if let Some(message) = recv_wayland_message(&mut client, &mut stream)? {
///     dispatch_request(&mut client, &mut output, &mut stream, message)?;
/// }
/// ```
///
/// @returns The next parsed Wayland message, or `None` if the socket was closed.
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
    let received = unsafe { recvmsg(stream.as_raw_fd(), &mut msg, MSG_DONTWAIT) };
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

/// Parses one or more Wayland messages from a wire-format byte chunk.
///
/// The first parsed message receives the provided file descriptors; any remaining
/// messages receive none.
///
/// # Errors
///
/// Returns an error if the chunk contains a truncated header, an invalid message
/// size, or a truncated payload.
///
/// # Examples
///
/// ```
/// use std::collections::VecDeque;
///
/// let bytes = [
///     8, 0, 0, 0,  // size = 8
///     1, 0,        // object_id = 1
///     2, 0,        // opcode = 2
/// ];
/// let mut queue = VecDeque::new();
///
/// parse_wire_chunk(&bytes, Vec::new(), &mut queue).unwrap();
/// assert_eq!(queue.len(), 1);
/// ```
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

/// Extracts file descriptors from a `SCM_RIGHTS` control message.
///
/// Malformed or out-of-bounds control data is ignored.
///
/// # Returns
///
/// A list of received file descriptors.
///
/// # Examples
///
/// ```
/// let fds = parse_received_fds(&[], 0);
/// assert!(fds.is_empty());
/// ```
#[example]
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

/// Parses a Wayland message header from the first 8 bytes.
///
/// # Examples
///
/// ```
/// let raw = [
///     1, 0, 0, 0, // object_id
///     5, 0, 8, 0, // opcode = 5, size = 8
/// ];
/// let header = parse_header(&raw);
/// assert_eq!(header.object_id, 1);
/// assert_eq!(header.opcode, 5);
/// assert_eq!(header.size, 8);
/// ```
fn parse_header(raw: &[u8]) -> WaylandHeader {
    let object_id = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let packed = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
    WaylandHeader {
        object_id,
        opcode: (packed & 0xffff) as u16,
        size: (packed >> 16) as u16,
    }
}

/// Sends a Wayland registry global announcement.
///
/// # Returns
///
/// `Ok(())` if the message was written successfully.
```
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

/// Sends a shared-memory format announcement.
///
/// # Examples
///
/// ```
/// send_shm_format(&mut stream, shm_id, WL_SHM_FORMAT_ARGB8888)?;
/// ```
fn send_shm_format(stream: &mut UnixStream, shm_id: u32, format: u32) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, format);
    send_message(stream, shm_id, WL_SHM_FORMAT, &payload)
}

/// Sends an `xdg_toplevel.configure` event.
///
/// # Examples
///
/// ```
/// send_xdg_toplevel_configure(&mut stream, toplevel_id, 800, 600, true)?;
/// # Ok::<(), std::io::Error>(())
/// ```
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

/// Sends an `xdg_surface.configure` event with a serial number.
///
/// # Examples
///
/// ```
/// send_xdg_surface_configure(&mut stream, xdg_surface_id, serial)?;
/// ```
fn send_xdg_surface_configure(
    stream: &mut UnixStream,
    xdg_surface_id: u32,
    serial: u32,
) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, serial);
    send_message(stream, xdg_surface_id, XDG_SURFACE_CONFIGURE, &payload)
}

/// Sends the advertised capabilities for a seat.
///
/// # Examples
///
/// ```
/// send_seat_capabilities(&mut stream, seat_id, capabilities)?;
/// # Ok::<(), std::io::Error>(())
/// ```
fn send_seat_capabilities(
    stream: &mut UnixStream,
    seat_id: u32,
    capabilities: u32,
) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, capabilities);
    send_message(stream, seat_id, WL_SEAT_CAPABILITIES, &payload)
}

/// Sends the seat name to a client.
///
/// # Examples
///
/// ```
/// send_seat_name(&mut stream, seat_id, "seat0")?;
/// ```
fn send_seat_name(stream: &mut UnixStream, seat_id: u32, name: &str) -> io::Result<()> {
    let mut payload = Vec::new();
    push_wayland_string(&mut payload, name);
    send_message(stream, seat_id, WL_SEAT_NAME, &payload)
}

/// Sends a `wl_keyboard.keymap` event using the no-keymap format.
///
/// # Examples
///
/// ```
/// let _ = send_keyboard_keymap(&mut stream, keyboard_id);
/// ```
fn send_keyboard_keymap(stream: &mut UnixStream, keyboard_id: u32) -> io::Result<()> {
    let mut payload = Vec::new();
    push_u32(&mut payload, WL_KEYBOARD_KEYMAP_FORMAT_NO_KEYMAP);
    // wl_keyboard.keymap is (format, fd, size).  This first input milestone uses
    // format=NO_KEYMAP, so there is no real keymap fd to pass yet; keep the fd
    // slot as 0 for Twilight's minimal test clients and size as 0.  A later XKB
    // stage should send a memfd through SCM_RIGHTS here.
    push_u32(&mut payload, 0);
    push_u32(&mut payload, 0);
    send_message(stream, keyboard_id, WL_KEYBOARD_KEYMAP, &payload)
}

/// Sends a pointer enter event for a surface.
///
/// # Examples
///
/// ```
/// send_pointer_enter(&mut stream, pointer_id, serial, surface_id, 10, 20)?;
/// # Ok::<(), std::io::Error>(())
/// ```
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

/// Sends a pointer leave event for a surface.
///
/// # Examples
///
/// ```
/// send_pointer_leave(&mut stream, pointer_id, serial, surface_id).unwrap();
/// ```
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

/// Sends a pointer motion event to a Wayland pointer object.
///
/// # Parameters
///
/// - `pointer_id`: The object ID of the `wl_pointer` resource.
/// - `time`: Event timestamp in milliseconds.
/// - `surface_x`: Pointer X coordinate in surface space.
/// - `surface_y`: Pointer Y coordinate in surface space.
///
/// # Examples
///
/// ```
/// # use std::io;
/// # use std::os::unix::net::UnixStream;
/// # fn send_pointer_motion(_: &mut UnixStream, _: u32, _: u32, _: i32, _: i32) -> io::Result<()> { Ok(()) }
/// # let (mut a, _b) = UnixStream::pair().unwrap();
/// send_pointer_motion(&mut a, 1, 123, 10 << 8, 20 << 8).unwrap();
/// ```
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

/// Sends a pointer button event.
///
/// # Examples
///
/// ```
/// send_pointer_button(&mut stream, pointer_id, serial, button, state)?;
/// ```
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

/// Sends a `wl_keyboard.enter` event for a surface.
///
/// # Examples
///
/// ```
/// send_keyboard_enter(&mut stream, keyboard_id, serial, surface_id)?;
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// @param serial The event serial.
/// @param surface_id The surface that gained keyboard focus.
/// @returns `Ok(())` if the event was sent successfully.
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

/// Sends a keyboard leave event for a surface.
///
/// # Examples
///
/// ```
/// send_keyboard_leave(&mut stream, keyboard_id, serial, surface_id)?;
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// @param serial The event serial.
/// @param surface_id The surface that lost keyboard focus.
/// @returns `Ok(())` if the event was sent successfully.
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

/// Sends a `wl_keyboard.key` event.

///

/// # Examples

///

/// ```

/// send_keyboard_key(&mut stream, keyboard_id, serial, keycode, state)?;

/// ```
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

/// Sends a Wayland protocol message to the client.
///
/// # Examples
///
/// ```
/// let payload = [];
/// send_message(&mut stream, 1, 0, &payload).unwrap();
/// ```
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
    send_wayland_bytes(stream, &message)
}

/// Writes a complete Wayland message to a Unix stream.
///
/// # Examples
///
/// ```
/// let message = [0u8; 8];
/// send_wayland_bytes(&mut stream, &message).unwrap();
/// ```
fn send_wayland_bytes(stream: &mut UnixStream, message: &[u8]) -> io::Result<()> {
    let mut offset = 0;
    while offset < message.len() {
        let mut iov = Iovec {
            // sendmsg does not mutate the buffer, but musl's iovec field is a
            // mutable pointer in C. Keep the cast local to this syscall wrapper.
            iov_base: message[offset..].as_ptr() as *mut c_void,
            iov_len: message.len() - offset,
        };
        let hdr = Msghdr {
            msg_name: ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: &mut iov,
            msg_iovlen: 1,
            __pad_iovlen: 0,
            msg_control: ptr::null_mut(),
            msg_controllen: 0,
            __pad_controllen: 0,
            msg_flags: 0,
        };

        // SAFETY: `hdr` and its single iovec point at the immutable `message`
        // slice for the duration of the syscall. `msg_name` and control are
        // null because this is a connected AF_UNIX stream.
        let sent = unsafe { sendmsg(stream.as_raw_fd(), &hdr, 0) };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        if sent == 0 {
            return Err(io::Error::new(
                ErrorKind::WriteZero,
                "sendmsg wrote zero bytes",
            ));
        }
        offset += sent as usize;
    }
    Ok(())
}

/// Copies a damaged region from a shared-memory buffer into the framebuffer.

///

/// The copied area is clipped to the buffer and output bounds. Only

/// `WL_SHM_FORMAT_ARGB8888` and `WL_SHM_FORMAT_XRGB8888` are supported.

///

/// # Examples

///

/// ```

/// let damage = Rect { x: 0, y: 0, width: 100, height: 100 };

/// let copied = blit_shm_buffer_to_output(&mut output, &pool, &buffer, &surface, damage)?;

/// assert!(copied.width >= 0 && copied.height >= 0);

/// # Ok::<(), std::io::Error>(())

/// ```
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

/// Maps a shared, read-only memory region from a file descriptor.
///
/// # Examples
///
/// ```
/// let ptr = unsafe_mmap_shm(fd, size)?;
/// assert!(!ptr.is_null());
/// # Ok::<(), std::io::Error>(())
/// ```
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
    /// Opens the framebuffer and maps it into memory.
    ///
    /// # Errors
    ///
    /// Returns an error if the framebuffer cannot be opened, queried, validated, or mapped.
    ///
    /// # Examples
    ///
    /// ```
    /// let output = SoftwareOutput::open().unwrap();
    /// assert!(output.width > 0);
    /// ```
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

    /// Fills the framebuffer with a solid color.
    ///
    /// # Examples
    ///
    /// ```
    /// output.clear(0x00000000).unwrap();
    /// ```
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

    /// Fills a rectangle in the framebuffer with a solid color.
    ///
    /// # Examples
    ///
    /// ```
    /// output.fill_rect(Rect { x: 0, y: 0, width: 100, height: 50 }, 0xff0000ff)?;
    /// ```
    fn fill_rect չուն?
    fn fill_rect(&mut self, rect: Rect, color: u32) -> io::Result<()> {
        let x0 = rect.x.max(0) as usize;
        let y0 = rect.y.max(0) as usize;
        let x1 = rect
            .x
            .saturating_add(rect.width)
            .clamp(0, self.width as i32) as usize;
        let y1 = rect
            .y
            .saturating_add(rect.height)
            .clamp(0, self.height as i32) as usize;
        if x1 <= x0 || y1 <= y0 {
            return Ok(());
        }

        for y in y0..y1 {
            let offset = y
                .checked_mul(self.stride)
                .and_then(|row| row.checked_add(x0 * 4))
                .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "fill offset overflow"))?;
            let len = x1 - x0;
            if offset + len * 4 > self.map_bytes {
                return Err(io::Error::new(ErrorKind::InvalidData, "fill out of bounds"));
            }
            // SAFETY: The row slice is fully bounds-checked against the mmap
            // size above and is within the framebuffer's 32-bit pixel format.
            let row = unsafe {
                std::slice::from_raw_parts_mut(self.pixels.add(offset).cast::<u32>(), len)
            };
            row.fill(color);
        }
        Ok(())
    }

    /// Presents the current framebuffer contents on screen.
    ///
    /// # Errors
    ///
    /// Returns an error if the framebuffer pan ioctl fails.
    ///
    /// # Examples
    ///
    /// ```
    /// output.sync().unwrap();
    /// ```
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
    /// Unmaps the framebuffer mapping when the output is dropped.
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
    /// Transfers ownership of the file descriptor and returns its raw value.
    ///
    /// # Examples
    ///
    /// ```
    /// let fd = OwnedFdRaw { fd: 3 }.into_raw();
    /// assert_eq!(fd, 3);
    /// ```
    fn into_raw(mut self) -> i32 {
        let fd = self.fd;
        self.fd = -1;
        fd
    }
}

impl Drop for OwnedFdRaw {
    /// Closes the owned file descriptor when the wrapper is dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::os::unix::io::RawFd;
    /// # struct OwnedFdRaw { fd: RawFd }
    /// # impl Drop for OwnedFdRaw {
    /// #     fn drop(&mut self) {
    /// #         if self.fd >= 0 {
    /// #             self.fd = -1;
    /// #         }
    /// #     }
    /// # }
    /// let fd = OwnedFdRaw { fd: 3 };
    /// drop(fd);
    /// ```
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
    /// Computes the smallest rectangle that contains both rectangles.
    ///
    /// # Examples
    ///
    /// ```
    /// let a = Rect { x: 10, y: 10, width: 20, height: 20 };
    /// let b = Rect { x: 25, y: 5, width: 10, height: 10 };
    ///
    /// let r = a.union(b);
    /// assert_eq!(r.x, 10);
    /// assert_eq!(r.y, 5);
    /// assert_eq!(r.width, 25);
    /// assert_eq!(r.height, 25);
    /// ```
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

/// Reads a little-endian `u32` from a byte slice at the given offset.
///
/// # Examples
///
/// ```
/// let bytes = [0x78, 0x56, 0x34, 0x12];
/// let value = read_u32(&bytes, 0).unwrap();
/// assert_eq!(value, 0x12345678);
/// ```
fn read_u32(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| io::Error::new(ErrorKind::UnexpectedEof, "missing u32 argument"))?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

/// Reads a little-endian `i32` from a byte slice at the given offset.
///
/// # Examples
///
/// ```
/// let bytes = 42i32.to_le_bytes();
/// assert_eq!(read_i32(&bytes, 0).unwrap(), 42);
/// ```
fn read_i32(bytes: &[u8], offset: usize) -> io::Result<i32> {
    Ok(read_u32(bytes, offset)? as i32)
}

/// Reads a Wayland string argument from a byte slice.

///

/// The returned offset points to the next 4-byte aligned argument boundary.

///

/// # Examples

///

/// ```

/// let bytes = [

///     5, 0, 0, 0,  // length = 5

///     b'h', b'i', 0,

/// ];

/// let (value, next) = read_wayland_string(&bytes, 0).unwrap();

/// assert_eq!(value, "hi");

/// assert_eq!(next, 8);

/// ```
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

/// Appends a 32-bit unsigned integer in little-endian order.
///
/// # Examples
///
/// ```
/// let mut bytes = Vec::new();
/// push_u32(&mut bytes, 0x12345678);
/// assert_eq!(bytes, vec![0x78, 0x56, 0x34, 0x12]);
/// ```
fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

/// Appends a 32-bit signed integer in little-endian byte order.
///
/// # Examples
///
/// ```
/// let mut bytes = Vec::new();
/// push_i32(&mut bytes, -2);
/// assert_eq!(bytes, (-2i32).to_le_bytes());
/// ```
fn push_i32(bytes: &mut Vec<u8>, value: i32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

/// Appends a 24.8 fixed-point value to a byte buffer.
///
/// # Examples
///
/// ```
/// let mut bytes = Vec::new();
/// push_fixed(&mut bytes, 12);
/// assert_eq!(bytes, 12_i32.saturating_mul(256).to_le_bytes());
/// ```
fn push_fixed(bytes: &mut Vec<u8>, value: i32) {
    push_i32(bytes, value.saturating_mul(256));
}

/// Appends a Wayland string to a byte buffer.
///
/// # Examples
///
/// ```
/// let mut bytes = Vec::new();
/// push_wayland_string(&mut bytes, "hello");
/// assert_eq!(bytes, vec![6, 0, 0, 0, b'h', b'e', b'l', b'l', b'o', 0, 0, 0]);
/// ```
fn push_wayland_string(bytes: &mut Vec<u8>, value: &str) {
    let length = value.len() + 1;
    push_u32(bytes, length as u32);
    bytes.extend_from_slice(value.as_bytes());
    bytes.push(0);
    while bytes.len() % 4 != 0 {
        bytes.push(0);
    }
}

/// Rounds a value up to the next multiple of 4.
///
/// # Examples
///
/// ```
/// assert_eq!(align4(0), 0);
/// assert_eq!(align4(1), 4);
/// assert_eq!(align4(4), 4);
/// assert_eq!(align4(5), 8);
/// ```
fn align4(value: usize) -> usize {
    (value + 3) & !3
}

/// Aligns a value to the next boundary for control message headers.
///
/// # Examples
///
/// ```
/// assert_eq!(cmsg_align(0), 0);
/// assert_eq!(cmsg_align(1), 8);
/// assert_eq!(cmsg_align(9), 16);
/// ```
fn cmsg_align(value: usize) -> usize {
    align4_to(value, size_of::<usize>())
}

/// Rounds a value up to the next multiple of the given alignment.
///
/// # Examples
///
/// ```
/// assert_eq!(align4_to(5, 4), 8);
/// assert_eq!(align4_to(8, 4), 8);
/// ```
fn align4_to(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}
