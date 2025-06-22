use alloc::string::String;
use crate::{println};
use crate::sys::fs;

pub fn main(args: &[&str]) {
    if args.is_empty() {
        return;
    }
    
    let mut fs = unsafe { fs::MFS.get_unchecked().lock() };

    if args[0] == ">" {
        let inode = fs.find_dir_entry(1, args[1]).unwrap().unwrap();
        
        fs.write_file(inode + 1, args[2..].join(" ").as_bytes()).unwrap();
        
        return;
    }
    
    let inode = fs.find_dir_entry(1, args[0]).unwrap().unwrap();
    
    let content_buf = fs.read_file(inode+1);
    
    println!("{}", String::from_utf8_lossy(content_buf.unwrap().as_slice()))
}
