use crate::{println, sys};
use crate::sys::console::DIR;

pub fn main(args: &[&str]) {
    let mut fs = unsafe { sys::fs::MFS.get_unchecked().lock() };
    
    #[allow(static_mut_refs)]
    let pwd = unsafe { DIR.as_str() };
    let inode = if pwd == "/" {
        1
    } else {
        fs.resolve_path(pwd).unwrap()
    };
    
    if args.len() < 1 {
        println!("USAGE: mkdir <dir name>");
        return;
    }
    
    let dir_name = args[0];
    
    if fs.create_dir(inode, dir_name).is_err() {
        println!("mkdir: failed to create directory");
    }
}