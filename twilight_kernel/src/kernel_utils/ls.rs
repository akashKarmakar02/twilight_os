use crate::println;
use crate::sys::{console, fs};

pub fn main(_args: &[&str], _ctx: &str) {
    #[allow(static_mut_refs)]
    let pwd = unsafe { console::DIR.as_str() };
    let mut fs = unsafe { fs::MFS.get_unchecked().lock() };
    
    let mut inode = 1u16;
    
    if pwd != "/" {
        inode = match fs.find_dir_entry(inode, &pwd[1..]) {
            Ok(inode) => inode.unwrap(),
            Err(_) => 1,
        }
    }

    if let Err(e) = fs.list_dir(inode) {
        println!("Error: {}", e);
    }
}
