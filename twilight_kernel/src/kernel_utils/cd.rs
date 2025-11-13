use crate::println;
use crate::sys::console::DIR;
use crate::sys::fs::vfs::{FileType, VFS};
use alloc::format;
use alloc::string::String;

pub fn main(args: &[&str]) {
    if args.len() < 2 {
        println!("cd: cd <directory>");
        return;
    }

    #[allow(static_mut_refs)]
    let fs = unsafe { VFS.get_mut() };

    #[allow(static_mut_refs)]
    let cur = unsafe { DIR.as_str() };

    let path = if args[1].starts_with('/') {
        String::from(args[1])
    } else if cur == "/" {
        format!("{}{}", cur, args[1])
    } else {
        format!("{}/{}", cur, args[1])
    };

    if let Ok(inode) = fs.open(path.as_str()) {
        if inode.metadata.file_type != FileType::Dir {
            println!("cd: {} is not a directory", args[1]);
            return;
        }
    } else {
        println!("cd: {} is not a directory", args[1]);
        return;
    }

    unsafe {
        if cur == "/" {
            DIR = format!("{}{}", cur, args[1]);
        } else {
            DIR = format!("{}/{}", cur, args[1]);
        }
    };
}
