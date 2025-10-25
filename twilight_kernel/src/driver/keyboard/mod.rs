use lazy_static::lazy_static;
use pc_keyboard::{DecodedKey, HandleControl, KeyCode, Keyboard, ScancodeSet1, layouts};
use spin::Mutex;

pub mod ps2;

lazy_static! {
    pub static ref KEYBOARD: Mutex<Keyboard<layouts::Us104Key, ScancodeSet1>> =
        Mutex::new(Keyboard::new(
            ScancodeSet1::new(),
            layouts::Us104Key,
            HandleControl::Ignore
        ));
}

pub fn keyboard_interrupt(scancode: u8) {
    let mut keyboard = KEYBOARD.lock();

    if let Ok(Some(key_event)) = keyboard.add_byte(scancode)
        && let Some(key) = keyboard.process_keyevent(key_event)
    {
        match key {
            DecodedKey::Unicode(character) => {
                send_char(character);
            }
            DecodedKey::RawKey(key) => {
                match key {
                    KeyCode::ArrowUp => {
                        crate::sys::buffer::stdin::send_special("up");
                    }
                    KeyCode::ArrowDown => {
                        crate::sys::buffer::stdin::send_special("down");
                    }
                    KeyCode::ArrowLeft => {
                        crate::sys::buffer::stdin::send_special("left");
                    }
                    KeyCode::ArrowRight => {
                        crate::sys::buffer::stdin::send_special("right");
                    }
                    _ => {}
                }
            }
        }
    }
}

fn send_char(c: char) {
    // get_stdio_keypress(c);
    crate::sys::buffer::stdin::send_char(c);
}