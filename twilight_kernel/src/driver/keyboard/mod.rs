#![allow(dead_code)]

use crate::arch::x86_64::halt;
use crate::driver::timer::pit::uptime;
use crate::sys::console::put_char_in_tty;
use crate::sys::fs::vfs::{BlockDev, VfsNodeOps};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem::size_of;
use lazy_static::lazy_static;
use pc_keyboard::{DecodedKey, HandleControl, KeyCode, KeyEvent as PcKeyEvent, KeyState, Keyboard, ScancodeSet1, layouts};
use spin::{Mutex, RwLock};

pub mod ps2;

pub trait KeyboardListener: Send + Sync {
    fn on_key(&self, key: u8, released: bool);
}

lazy_static! {
    pub static ref KEYBOARD: Mutex<Keyboard<layouts::Us104Key, ScancodeSet1>> =
        Mutex::new(Keyboard::new(
            ScancodeSet1::new(),
            layouts::Us104Key,
            HandleControl::Ignore
        ));
}

lazy_static! {
    static ref KEYBOARD_LISTENER: RwLock<Vec<Arc<dyn Fn(u8) + Send + Sync>>> =
        RwLock::new(Vec::new());
}

lazy_static! {
    static ref PS2_KEYBOARD_STATE: Mutex<Ps2KeyboardState> = Mutex::new(Ps2KeyboardState::new());
}

lazy_static! {
    static ref KEY_EVENT_QUEUE: Mutex<VecDeque<PendingKeyEvent>> =
        Mutex::new(VecDeque::with_capacity(MAX_KEY_EVENTS));
}

const MAX_KEY_EVENTS: usize = 256;
const EV_KEY: u16 = 0x01;
const KEY_ESC: u16 = 1;
const KEY_1: u16 = 2;
const KEY_2: u16 = 3;
const KEY_3: u16 = 4;
const KEY_4: u16 = 5;
const KEY_5: u16 = 6;
const KEY_6: u16 = 7;
const KEY_7: u16 = 8;
const KEY_8: u16 = 9;
const KEY_9: u16 = 10;
const KEY_0: u16 = 11;
const KEY_MINUS: u16 = 12;
const KEY_EQUAL: u16 = 13;
const KEY_BACKSPACE: u16 = 14;
const KEY_TAB: u16 = 15;
const KEY_Q: u16 = 16;
const KEY_W: u16 = 17;
const KEY_E: u16 = 18;
const KEY_R: u16 = 19;
const KEY_T: u16 = 20;
const KEY_Y: u16 = 21;
const KEY_U: u16 = 22;
const KEY_I: u16 = 23;
const KEY_O: u16 = 24;
const KEY_P: u16 = 25;
const KEY_LEFTBRACE: u16 = 26;
const KEY_RIGHTBRACE: u16 = 27;
const KEY_ENTER: u16 = 28;
const KEY_LEFTCTRL: u16 = 29;
const KEY_A: u16 = 30;
const KEY_S: u16 = 31;
const KEY_D: u16 = 32;
const KEY_F: u16 = 33;
const KEY_G: u16 = 34;
const KEY_H: u16 = 35;
const KEY_J: u16 = 36;
const KEY_K: u16 = 37;
const KEY_L: u16 = 38;
const KEY_SEMICOLON: u16 = 39;
const KEY_APOSTROPHE: u16 = 40;
const KEY_GRAVE: u16 = 41;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_BACKSLASH: u16 = 43;
const KEY_Z: u16 = 44;
const KEY_X: u16 = 45;
const KEY_C: u16 = 46;
const KEY_V: u16 = 47;
const KEY_B: u16 = 48;
const KEY_N: u16 = 49;
const KEY_M: u16 = 50;
const KEY_COMMA: u16 = 51;
const KEY_DOT: u16 = 52;
const KEY_SLASH: u16 = 53;
const KEY_RIGHTSHIFT: u16 = 54;
const KEY_LEFTALT: u16 = 56;
const KEY_SPACE: u16 = 57;
const KEY_CAPSLOCK: u16 = 58;
const KEY_F1: u16 = 59;
const KEY_F2: u16 = 60;
const KEY_F3: u16 = 61;
const KEY_F4: u16 = 62;
const KEY_F5: u16 = 63;
const KEY_F6: u16 = 64;
const KEY_F7: u16 = 65;
const KEY_F8: u16 = 66;
const KEY_F9: u16 = 67;
const KEY_F10: u16 = 68;
const KEY_NUMLOCK: u16 = 69;
const KEY_SCROLLLOCK: u16 = 70;
const KEY_HOME: u16 = 102;
const KEY_UP: u16 = 103;
const KEY_PAGEUP: u16 = 104;
const KEY_LEFT: u16 = 105;
const KEY_RIGHT: u16 = 106;
const KEY_END: u16 = 107;
const KEY_DOWN: u16 = 108;
const KEY_PAGEDOWN: u16 = 109;
const KEY_INSERT: u16 = 110;
const KEY_DELETE: u16 = 111;
const KEY_F11: u16 = 87;
const KEY_F12: u16 = 88;
const KEY_RIGHTCTRL: u16 = 97;
const KEY_RIGHTALT: u16 = 100;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct InputEvent {
    pub tv_sec: i64,
    pub tv_usec: i64,
    pub type_: u16,
    pub code: u16,
    pub value: i32,
}

impl InputEvent {
    fn new(event_type: u16, code: u16, pressed: bool) -> Self {
        let micros = (uptime() * 1_000_000.0) as u64;
        let tv_sec = (micros / 1_000_000) as i64;
        let tv_usec = (micros % 1_000_000) as i64;

        Self {
            tv_sec,
            tv_usec,
            type_: event_type,
            code,
            value: if pressed { 1 } else { 0 },
        }
    }

    fn to_bytes(self) -> [u8; size_of::<InputEvent>()] {
        unsafe { core::mem::transmute(self) }
    }
}

#[derive(Copy, Clone)]
struct PendingKeyEvent {
    code: u16,
    pressed: bool,
}

fn enqueue_key_event(event: PcKeyEvent) {
    if let Some(code) = keycode_to_linux(event.code) {
        let mut queue = KEY_EVENT_QUEUE.lock();
        if queue.len() >= MAX_KEY_EVENTS {
            queue.pop_front();
        }
        queue.push_back(PendingKeyEvent {
            code,
            pressed: event.state == KeyState::Down,
        });
    }
}

fn keycode_to_linux(code: KeyCode) -> Option<u16> {
    use KeyCode::*;

    let value = match code {
        Escape => KEY_ESC,
        F1 => KEY_F1,
        F2 => KEY_F2,
        F3 => KEY_F3,
        F4 => KEY_F4,
        F5 => KEY_F5,
        F6 => KEY_F6,
        F7 => KEY_F7,
        F8 => KEY_F8,
        F9 => KEY_F9,
        F10 => KEY_F10,
        F11 => KEY_F11,
        F12 => KEY_F12,
        Key1 => KEY_1,
        Key2 => KEY_2,
        Key3 => KEY_3,
        Key4 => KEY_4,
        Key5 => KEY_5,
        Key6 => KEY_6,
        Key7 => KEY_7,
        Key8 => KEY_8,
        Key9 => KEY_9,
        Key0 => KEY_0,
        OemMinus => KEY_MINUS,
        OemPlus => KEY_EQUAL,
        Backspace => KEY_BACKSPACE,
        Tab => KEY_TAB,
        Q => KEY_Q,
        W => KEY_W,
        E => KEY_E,
        R => KEY_R,
        T => KEY_T,
        Y => KEY_Y,
        U => KEY_U,
        I => KEY_I,
        O => KEY_O,
        P => KEY_P,
        Oem4 => KEY_LEFTBRACE,
        Oem6 => KEY_RIGHTBRACE,
        Oem5 => KEY_BACKSLASH,
        CapsLock => KEY_CAPSLOCK,
        A => KEY_A,
        S => KEY_S,
        D => KEY_D,
        F => KEY_F,
        G => KEY_G,
        H => KEY_H,
        J => KEY_J,
        K => KEY_K,
        L => KEY_L,
        Oem1 => KEY_SEMICOLON,
        Oem3 => KEY_APOSTROPHE,
        Return => KEY_ENTER,
        LShift => KEY_LEFTSHIFT,
        Z => KEY_Z,
        X => KEY_X,
        C => KEY_C,
        V => KEY_V,
        B => KEY_B,
        N => KEY_N,
        M => KEY_M,
        OemComma => KEY_COMMA,
        OemPeriod => KEY_DOT,
        Oem2 => KEY_SLASH,
        RShift => KEY_RIGHTSHIFT,
        Spacebar => KEY_SPACE,
        LAlt => KEY_LEFTALT,
        RAltGr | RAlt2 => KEY_RIGHTALT,
        LControl => KEY_LEFTCTRL,
        RControl | RControl2 => KEY_RIGHTCTRL,
        ArrowUp => KEY_UP,
        ArrowDown => KEY_DOWN,
        ArrowLeft => KEY_LEFT,
        ArrowRight => KEY_RIGHT,
        Insert => KEY_INSERT,
        Home => KEY_HOME,
        PageUp => KEY_PAGEUP,
        Delete => KEY_DELETE,
        End => KEY_END,
        PageDown => KEY_PAGEDOWN,
        _ => return None,
    };

    Some(value)
}

struct Ps2KeyboardState {
    is_ctrl_pressed: bool,
    is_shift_pressed: bool,
}

impl Ps2KeyboardState {
    pub fn new() -> Self {
        Self {
            is_ctrl_pressed: false,
            is_shift_pressed: false,
        }
    }
}

pub fn register_keyboard_listener(listener: Arc<dyn Fn(u8) + Send + Sync>) {
    KEYBOARD_LISTENER.write().push(listener);
}

pub struct KeyboardDev;

impl KeyboardDev {
    fn pop_events(&self, capacity: usize) -> Vec<InputEvent> {
        let mut collected = Vec::new();
        let mut queue = KEY_EVENT_QUEUE.lock();

        for _ in 0..capacity {
            if let Some(evt) = queue.pop_front() {
                collected.push(InputEvent::new(EV_KEY, evt.code, evt.pressed));
            } else {
                break;
            }
        }

        collected
    }
}

impl VfsNodeOps for KeyboardDev {
    fn read(&self, _device: &mut BlockDev, _lba: usize, buf: &mut [u8]) -> Result<usize, ()> {
        const EVENT_SIZE: usize = size_of::<InputEvent>();
        if buf.len() < EVENT_SIZE {
            return Ok(0);
        }

        loop {
            let capacity = buf.len() / EVENT_SIZE;
            let events = self.pop_events(capacity.max(1));

            if events.is_empty() {
                halt();
                continue;
            }

            let mut out = Vec::with_capacity(events.len() * EVENT_SIZE);
            for evt in events {
                out.extend_from_slice(&evt.to_bytes());
            }

            return Ok(out.len());
        }
    }

    fn write(&mut self, _device: &mut BlockDev, _lba: usize, _data: &[u8]) -> Result<(), ()> {
        Err(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(!KEY_EVENT_QUEUE.lock().is_empty())
    }

    fn ioctl(&mut self, _device: &mut BlockDev, _cmd: u64, _arg: usize) -> Result<i64, ()> {
        Ok(0)
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}

pub fn keyboard_interrupt(scancode: u8) {
    let mut keyboard = KEYBOARD.lock();

    let mut ps2_keyboard_state = PS2_KEYBOARD_STATE.lock();

    if scancode == 29 {
        ps2_keyboard_state.is_ctrl_pressed = true;
    } else if scancode == 157 {
        ps2_keyboard_state.is_ctrl_pressed = false;
    }

    if let Ok(Some(key_event)) = keyboard.add_byte(scancode) {
        enqueue_key_event(key_event.clone());

        if let Some(key) = keyboard.process_keyevent(key_event) {
            match key {
                DecodedKey::Unicode(character) => {
                    if ps2_keyboard_state.is_ctrl_pressed && character == 's' {
                        put_char_in_tty(0x13);
                    }
                    if ps2_keyboard_state.is_ctrl_pressed && character == 'c' {
                        put_char_in_tty(0x03);
                    }
                    if character == '\t' {
                        put_char_in_tty(b' ');
                        put_char_in_tty(b' ');
                        put_char_in_tty(b' ');
                        put_char_in_tty(b' ');
                    }
                    if !ps2_keyboard_state.is_ctrl_pressed {
                        put_char_in_tty(character as u8);
                    }
                }
                DecodedKey::RawKey(key) => match key {
                    KeyCode::ArrowUp => {
                        for c in "\x1b[A".chars() {
                            put_char_in_tty(c as u8);
                        }
                    }
                    KeyCode::ArrowDown => {
                        for c in "\x1b[B".chars() {
                            put_char_in_tty(c as u8);
                        }
                    }
                    KeyCode::ArrowLeft => {
                        for c in "\x1b[D".chars() {
                            put_char_in_tty(c as u8);
                        }
                    }
                    KeyCode::ArrowRight => {
                        for c in "\x1b[C".chars() {
                            put_char_in_tty(c as u8);
                        }
                    }
                    KeyCode::Backspace => {
                        put_char_in_tty(b'\x08');
                    }
                    _ => {}
                },
            }
        }
    }
}
