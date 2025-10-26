use crate::sys::console::put_char_in_tty;
use alloc::sync::Arc;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use pc_keyboard::{layouts, DecodedKey, HandleControl, KeyCode, Keyboard, ScancodeSet1};
use spin::{Mutex, RwLock};
use crate::serial_println;

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

pub fn register_keyboard_listener(listener: Arc<dyn Fn(u8) + Send + Sync>) {
    KEYBOARD_LISTENER.write().push(listener);
}

pub fn keyboard_interrupt(scancode: u8) {
    let mut keyboard = KEYBOARD.lock();

    if let Ok(Some(key_event)) = keyboard.add_byte(scancode)
        && let Some(key) = keyboard.process_keyevent(key_event)
    {
        match key {
            DecodedKey::Unicode(character) => {
                put_char_in_tty(character as u8);
            }
            DecodedKey::RawKey(key) => {
                match key {
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
                }
            }
        }
    }
}
