use alloc::string::String;
use crate::{fs, print, println};

pub fn main(args: &[&str]) {
    if args.len() == 0 {
        return;
    }

    if args[0] == ">" && args.len() == 2 {
        if let Some(_) = fs::create(args[1]) {

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
