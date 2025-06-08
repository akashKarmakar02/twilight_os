use alloc::string::String;
use crate::{fs, print, println};

pub fn main(args: &[&str]) {
    if args.is_empty() {
        return;
    }

    if args[0] == ">" && args.len() == 2 {
        if fs::create(args[1]).is_some() {

        } else {
            println!("Error: Could not open fs file");
            return;
        }

        fs::write(args[1], 0, b"hello world\n")
            .expect("TODO: panic message");

        return;
    }

    if let Some(data) = fs::read(args[0], 0) {
        print!("{}", String::from_utf8_lossy(data.as_slice()));
    }
}
