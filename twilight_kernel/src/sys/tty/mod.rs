use alloc::string::{String, ToString};
use x86_64::instructions::interrupts;
use crate::arch::x86_64::halt;
use crate::driver::disk::ata::{FileIO, IO};
use crate::println;
use crate::sys::buffer::stdin::STDIN;

#[derive(Debug)]
pub struct Tty;


impl FileIO for Tty {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        let mut s = if buf.len() == 4 {
            read_char().to_string()
        } else {
            read_line()
        };
        s.truncate(buf.len());
        let n = s.len();
        buf[0..n].copy_from_slice(s.as_bytes());
        Ok(n)
    }

    fn write(&mut self, buf: &[u8]) -> Result<usize, ()> {
        println!("{}", String::from_utf8_lossy(buf));
        Ok(buf.len())
    }

    fn close(&mut self) {}

    fn poll(&mut self, event: IO) -> bool {
        todo!()
    }
}

pub fn read_char() -> char {
    loop {
        halt();
        let res = interrupts::without_interrupts(|| {
            let mut stdin = STDIN.lock();
            if !stdin.is_empty() {
                Some(stdin.remove(0).unwrap() as char)
            } else {
                None
            }
        });
        if let Some(c) = res {
            return c;
        }
    }
}


pub fn read_line() -> String {
    loop {
        halt();
        let res = interrupts::without_interrupts(|| {
            let mut stdin = STDIN.lock();
            let ch = stdin.back();
            match ch {
                Some('\n') => {
                    let line: String = stdin.clone().iter().collect();
                    stdin.clear();
                    Some(line)
                }
                _ => None,
            }
        });
        if let Some(line) = res {
            return line;
        }
    }
}