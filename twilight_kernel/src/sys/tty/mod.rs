use alloc::string::{String, ToString};
use x86_64::instructions::interrupts;
use crate::arch::x86_64::halt;
use crate::driver::disk::ata::{FileIO, IO};
use crate::{print, println};
use crate::sys::buffer::stdin::STDIN;

#[allow(dead_code)]
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

    fn poll(&mut self, _event: IO) -> bool {
        todo!()
    }
}

pub fn read_char() -> char {
    loop {
        halt();
        let res = interrupts::without_interrupts(|| {
            let mut stdin = STDIN.lock();
            if !stdin.is_empty() {
                Some(stdin.remove(0).unwrap())
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
    let mut line = String::new();
    loop {
        halt();
        let res = interrupts::without_interrupts(|| {
            let mut stdin = STDIN.lock();
            let ch = stdin.back();
            match ch {
                Some('\n') => {
                    let mut result_line = line.clone();
                    result_line.push('\n');
                    print!("\n");
                    stdin.clear();
                    Some(result_line)
                }
                Some(ch) => {
                    print!("{}", ch);
                    line.push(*ch);
                    stdin.clear();
                    None
                },
                None => None,
            }
        });
        if let Some(line) = res {
            return line;
        }
    }
}