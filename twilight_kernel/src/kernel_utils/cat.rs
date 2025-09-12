use crate::println;
use crate::sys::console::DIR;
use crate::sys::fs::vfs::VFS;
use alloc::format;
use alloc::string::String;

pub fn main(args: &[&str]) {
    if args.len() < 2 {
        return;
    }

    #[allow(static_mut_refs)]
    let pwd = unsafe { DIR.as_str() };
    if args[1] == ">" {
        #[allow(static_mut_refs)]
        if let Ok(_) = unsafe { VFS.get_mut().read(format!("{}/{}", pwd, args[2]).as_str()) } {
            #[allow(static_mut_refs)]
            if let Err(_) = unsafe { VFS.get_mut().write(format!("{}/{}", pwd, args[2]).as_str(), args[3..].join(" ").as_bytes()) } {
                println!("cat: {}: Failed to write", args[2]);
            }
        } else {
            #[allow(static_mut_refs)]
            if let Err(_) = unsafe { VFS.get_mut().touch(pwd, args[2], 0) } {
                println!("cat: {}: Failed to create", args[2]);
            } else {
                #[allow(static_mut_refs)]
                unsafe { VFS.get_mut().write(format!("{}/{}", pwd, args[2]).as_str(), args[3..].join(" ").as_bytes()) }.unwrap();
            }
        }

        return;
    }

    #[allow(static_mut_refs)]
    if let Ok(content) = unsafe { VFS.get_mut().read(format!("{}/{}", pwd, args[1]).as_str()) } {
        println!("{}", String::from_utf8_lossy(content.as_slice()));
    } else {
        println!("cat: {}: No such file or directory", args[1]);
    }
}
