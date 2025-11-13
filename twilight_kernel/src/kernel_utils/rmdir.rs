use crate::println;
use crate::sys::console::DIR;
use crate::sys::fs::vfs::VFS;
use alloc::format;
use alloc::string::String;

pub fn main(args: &[&str]) {
    if args.len() < 2 {
        println!("Usage: rm <file>");
        return;
    }

    #[allow(static_mut_refs)]
    let pwd = unsafe { DIR.as_str() };

    let rm_path = if args[1].starts_with('/') {
        String::from(args[1])
    } else {
        format!("{}/{}", pwd, args[1])
    };

    #[allow(static_mut_refs)]
    if let Err(_) = unsafe { VFS.get_mut().rmdir(rm_path.as_str()) } {
        println!("rmdir: {}: No such file or directory", args[1]);
    }
}
