use super::{enqueue_packet, PS2_PACKET_SIZE};
use core::sync::atomic::{AtomicBool, Ordering};
use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::instructions::port::Port;

const DATA_PORT: u16 = 0x60;
const STATUS_PORT: u16 = 0x64;
const CONTROLLER_CMD_PORT: u16 = 0x64;

const ENABLE_AUX_PORT: u8 = 0xA8;
const READ_CONFIG: u8 = 0x20;
const WRITE_CONFIG: u8 = 0x60;
const WRITE_AUX: u8 = 0xD4;

const CMD_SET_DEFAULTS: u8 = 0xF6;
const CMD_ENABLE_DATA: u8 = 0xF4;

const ACK: u8 = 0xFA;
const RESEND: u8 = 0xFE;

static INITIALIZED: AtomicBool = AtomicBool::new(false);

lazy_static! {
    static ref PACKET_STATE: Mutex<MousePacketState> = Mutex::new(MousePacketState::new());
}

struct MousePacketState {
    bytes: [u8; PS2_PACKET_SIZE],
    index: usize,
}

impl MousePacketState {
    const fn new() -> Self {
        Self {
            bytes: [0; PS2_PACKET_SIZE],
            index: 0,
        }
    }

    fn push(&mut self, byte: u8) -> Option<[u8; PS2_PACKET_SIZE]> {
        if self.index == 0 && (byte & 0x08) == 0 {
            return None;
        }

        self.bytes[self.index] = byte;
        self.index += 1;

        if self.index == PS2_PACKET_SIZE {
            self.index = 0;
            Some(self.bytes)
        } else {
            None
        }
    }
}

pub fn init() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }

    unsafe {
        flush_output_buffer();
        send_controller_command(ENABLE_AUX_PORT);

        let mut config = read_controller_config();
        config |= 0x02; // enable IRQ12
        config &= !0x20; // enable clock for aux port
        write_controller_config(config);

        send_mouse_command(CMD_SET_DEFAULTS);
        send_mouse_command(CMD_ENABLE_DATA);
    }
}

pub fn handle_interrupt_byte(byte: u8) {
    let mut state = PACKET_STATE.lock();
    if let Some(packet) = state.push(byte) {
        enqueue_packet(packet);
    }
}

unsafe fn read_status() -> u8 {
    unsafe { Port::<u8>::new(STATUS_PORT).read() }
}

unsafe fn wait_input_clear() {
    while unsafe { read_status() } & 0x02 != 0 {}
}

unsafe fn wait_output_full() {
    while unsafe { read_status() } & 0x01 == 0 {}
}

unsafe fn read_data() -> u8 {
    unsafe { wait_output_full() };
    unsafe { Port::<u8>::new(DATA_PORT).read() }
}

unsafe fn write_data(data: u8) {
    unsafe { wait_input_clear() };
    unsafe { Port::<u8>::new(DATA_PORT).write(data) };
}

unsafe fn send_controller_command(cmd: u8) {
    unsafe { wait_input_clear() };
    unsafe { Port::<u8>::new(CONTROLLER_CMD_PORT).write(cmd) };
}

unsafe fn read_controller_config() -> u8 {
    unsafe { send_controller_command(READ_CONFIG) };
    unsafe { read_data() }
}

unsafe fn write_controller_config(config: u8) {
    unsafe { send_controller_command(WRITE_CONFIG) };
    unsafe { write_data(config) };
}

unsafe fn flush_output_buffer() {
    while unsafe { read_status() } & 0x01 != 0 {
        let _ = unsafe { Port::<u8>::new(DATA_PORT).read() };
    }
}

unsafe fn send_mouse_command(cmd: u8) {
    for _ in 0..3 {
        unsafe { send_controller_command(WRITE_AUX) };
        unsafe { write_data(cmd) };
        let response = unsafe { read_data() };
        match response {
            ACK => return,
            RESEND => continue,
            _ => return,
        }
    }
}
