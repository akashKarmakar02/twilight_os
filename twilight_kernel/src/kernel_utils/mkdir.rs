use crate::{println, sys};

pub fn main(args: &[&str]) {
    let mut fs = unsafe { sys::fs::MFS.get_unchecked().lock() };
    
    if args.len() < 1 {
        println!("USAGE: mkdir <dir name>");
        return;
    }
    
    let dir_name = args[0];
    
    if fs.create_dir(1, dir_name).is_err() {
        println!("mkdir: failed to create directory");
    }
}