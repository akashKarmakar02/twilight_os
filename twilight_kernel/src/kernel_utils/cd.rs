use alloc::format;
use crate::println;
use crate::sys::console::DIR;
use crate::sys::fs::MFS;
use alloc::string::String;

pub fn main(args: &[&str]) {
    if args.is_empty() {
        println!("cd: cd <directory>");
        return;
    }

    let mut fs = unsafe { MFS.get_unchecked().lock() };

    #[allow(static_mut_refs)]
    let cur = unsafe { DIR.as_str() };

    let parent_inode = if cur != "/" {
        fs.find_dir_entry(1, &cur[1..]).unwrap().unwrap()
    } else {
        1
    };
    
    if let Ok(inode) = fs.find_dir_entry(parent_inode, args[0]) {
        if inode.is_none() {
            println!("cd: no such file or directory");
            return;
        }
        if inode.unwrap() == 1 {
            unsafe {
                DIR = String::from("/");
            };
            return;
        }
    }

    unsafe {
        DIR = format!("{}{}", cur, args[0]);
    };
}
