use alloc::format;
use crate::println;
use crate::sys::console::DIR;
use crate::sys::fs::MFS;
use alloc::string::String;
use alloc::vec::Vec;

pub fn main(args: &[&str]) {
    if args.len() < 2 {
        println!("cd: cd <directory>");
        return;
    }

    let mut fs = unsafe { MFS.get_unchecked().lock() };

    #[allow(static_mut_refs)]
    let cur = unsafe { DIR.as_str() };

    let parent_inode = if cur != "/" {
        fs.resolve_path(cur).unwrap()
    } else {
        1
    };

    if let Ok(inode) = fs.find_dir_entry(parent_inode, args[1]) {
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
        if args[1] == "." {
            return;
        }
        if args[1] == ".." {
            let mut cur: Vec<&str> = cur.split("/").collect();
            cur.pop();
            unsafe  {
                DIR = cur.join("/");
            }
            return;
        }
    }

    unsafe {
        if cur == "/" {
            DIR = format!("{}{}", cur, args[1]);
        } else {
            DIR = format!("{}/{}", cur, args[1]);
        }
    };
}
