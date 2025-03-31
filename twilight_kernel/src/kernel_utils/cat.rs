use crate::{fs, print, println};
use crate::fs::Vfs;

pub fn main(args: &[&str]) {
    if args.len() == 0 {
        return;
    }

    let mut fs = fs::FS.get().unwrap().lock();
    let id: Option<u64>;

    if args[0] == ">" && args.len() == 2 {
        if let Ok(fid) = fs.create(args[1]) {
            id = Some(fid);
        } else {
            println!("Error: Could not open fs file");
            return;
        }

        fs.write(id.unwrap(), 0, b"hello world").expect("TODO: panic message");

        return;
    }

    if let Ok(fid) = fs.open(args[0]) {
        id = Some(fid);
    } else {
        println!("failed to read file {}", args[0]);
        return;
    }

    let mut buf: [u8; 256] = [0u8; 256];

    if let Ok(len) = fs.read(id.unwrap(), 0, &mut buf) {
        for i in 0..len {
            print!("{:x}", buf[i]);
        }
    } else {

    }
}
