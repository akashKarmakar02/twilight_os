use lazy_static::lazy_static;
use pc_keyboard::{layouts, DecodedKey, HandleControl, Keyboard, ScancodeSet1};
use spin::Mutex;
use crate::console::get_stdio_keypress;

pub mod ps2;


lazy_static! {
    static ref KEYBOARD: Mutex<Keyboard<layouts::Us104Key, ScancodeSet1>> = Mutex::new(Keyboard::new(ScancodeSet1::new(), layouts::Us104Key, HandleControl::Ignore));
}

pub fn keyboard_interrupt(scancode: u8) {
    let mut keyboard = KEYBOARD.lock();


    if let Ok(Some(key_event)) = keyboard.add_byte(scancode) {
        if let Some(key) = keyboard.process_keyevent(key_event) {
            match key {
                DecodedKey::Unicode(character) => send_char(character),
                DecodedKey::RawKey(_key) => {}
            }
        }
    }
}

fn send_char(c: char) {
    get_stdio_keypress(c);
}