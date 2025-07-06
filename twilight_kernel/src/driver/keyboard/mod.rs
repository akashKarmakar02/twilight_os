use crate::println;
use conquer_once::spin::OnceCell;
use core::pin::Pin;
use core::task::{Context, Poll};
use crossbeam_queue::ArrayQueue;
use futures_util::task::AtomicWaker;
use futures_util::{Stream, StreamExt};
use pc_keyboard::{DecodedKey, HandleControl, KeyCode, Keyboard, ScancodeSet1, layouts};

pub mod ps2;

static SCANCODE_QUEUE: OnceCell<ArrayQueue<u8>> = OnceCell::uninit();
static WAKER: AtomicWaker = AtomicWaker::new();

pub(crate) fn add_scancode(scancode: u8) {
    if let Ok(queue) = SCANCODE_QUEUE.try_get() {
        if queue.push(scancode).is_err() {
            println!("WARNING: scancode queue full; dropping keyboard input");
        } else {
            WAKER.wake();
        }
    } else {
        println!("WARNING: scancode queue uninitialized");
    }
}

pub fn keyboard_interrupt(scancode: u8) {
    let mut keyboard = Keyboard::new(
        ScancodeSet1::new(),
        layouts::Us104Key,
        HandleControl::Ignore,
    );

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
                        // handle up arrow
                        crate::sys::buffer::stdin::send_special("up");
                    }
                    KeyCode::ArrowDown => {
                        // handle down arrow
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