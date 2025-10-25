use alloc::collections::VecDeque;
use core::pin::Pin;
use core::task::{Context, Poll};
use futures_util::Stream;
use futures_util::task::AtomicWaker;
use spin::Mutex;

pub static STDIN: Mutex<VecDeque<char>> = Mutex::new(VecDeque::new());
pub static STDIN_WAKER: AtomicWaker = AtomicWaker::new();

pub fn send_char(c: char) {
    let mut stdin = STDIN.lock();
    stdin.push_back(c);
    STDIN_WAKER.wake();
}

pub fn send_special(key: &str) {
    let mut stdin = STDIN.lock();
    match key {
        "up" => {
            for ch in "\x1b[A".chars() {
                stdin.push_back(ch);
            }
        },
        "down" => {
            for ch in "\x1b[B".chars() {
                stdin.push_back(ch);
            }
        },
        "left" => {
            for ch in "\x1b[D".chars() {
                stdin.push_back(ch);
            }
        },
        "right" => {
            for ch in "\x1b[C".chars() {
                stdin.push_back(ch);
            }
        },
        _ => return,
    }
    STDIN_WAKER.wake();
}


pub fn stdin_available() -> bool {
    !STDIN.lock().is_empty()
}

pub struct StdinStream {
    _private: (),
}

impl Default for StdinStream {
    fn default() -> Self {
        Self::new()
    }
}

impl StdinStream {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Stream for StdinStream {
    type Item = char;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut stdin = STDIN.lock();

        STDIN_WAKER.register(cx.waker());
        if let Some(c) = stdin.pop_front() {
            STDIN_WAKER.take();
            Poll::Ready(Some(c))
        } else {
            Poll::Pending
        }
    }
}
