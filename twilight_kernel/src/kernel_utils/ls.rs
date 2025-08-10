use crate::println;
use crate::sys::{console, fs};

pub fn main(_args: &[&str], _ctx: &str) {
    #[allow(static_mut_refs)]
    let pwd = unsafe { console::DIR.as_str() };
    let mut fs = unsafe { fs::MFS.get_unchecked().lock() };
    
    let mut inode = 1u16;
    
    if pwd != "/" {
        inode = fs.resolve_path(pwd).unwrap_or_else(|_| 1)
    }
    
    if let Err(e) = fs.list_dir(inode) {
        println!("Error: {}", e);
    }
}
