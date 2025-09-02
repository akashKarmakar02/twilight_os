use alloc::string::String;
use crate::print;
use crate::sys::tty::{read_char, read_line};

pub fn read(handler: usize, buf: &mut [u8], len: usize) -> usize {
    if handler == 0 {
        let mut str = if len != 1 {
            read_line()
        } else {
            let res = String::from(read_char());
            print!("{}", res);
            res
        };
        str.truncate(len);

        let string_bytes = str.as_bytes();
        let copy_len = string_bytes.len().min(buf.len());
        buf[..copy_len].copy_from_slice(&string_bytes[..copy_len]);

        return copy_len;
    }

    0
}

pub fn exit() -> usize {
    crate::sys::proc::exit();

    0
}
