use crate::println;
use crate::sys::fs;

pub fn main(_args: &[&str], _ctx: &str) {
    let mut fs = unsafe { fs::MFS.get_unchecked().lock() };
    
    if let Err(e) = fs.list_dir(1) {
        println!("Error: {}", e);
    }
}
