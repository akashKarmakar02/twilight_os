#![allow(dead_code)]

use crate::sys::console::put_char_in_tty;
use alloc::sync::Arc;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use pc_keyboard::{DecodedKey, HandleControl, KeyCode, Keyboard, ScancodeSet1, layouts};
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

pub fn keyboard_interrupt(scancode: u8) {
    let mut keyboard = KEYBOARD.lock();

    let mut ps2_keyboard_state = PS2_KEYBOARD_STATE.lock();

    if scancode == 29 {
        ps2_keyboard_state.is_ctrl_pressed = true;
    } else if scancode == 157 {
        ps2_keyboard_state.is_ctrl_pressed = false;
    }

    if let Ok(Some(key_event)) = keyboard.add_byte(scancode)
        && let Some(key) = keyboard.process_keyevent(key_event)
    {
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
