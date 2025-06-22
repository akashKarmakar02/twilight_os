use alloc::format;
use alloc::string::String;
use crate::println;
use crate::sys::console::DIR;
use crate::sys::fs::MFS;

pub fn main(args: &[&str]) {
    if args.is_empty() {
        println!("Usage: rm <file>");
        return;
    }
    let mut fs = unsafe { MFS.get_unchecked().lock() };
    
    #[allow(static_mut_refs)]
    let pwd = unsafe { DIR.as_str() };
    
    let rm_path = if args[0].starts_with('/') {
        String::from(args[0])
    } else {
        format!("{}/{}", pwd, args[0])
    };
    
    if fs.resolve_path(rm_path.as_str()).is_ok() {
        println!("rm: {}: Removed", rm_path);
        fs.remove_entry(rm_path.as_str()).unwrap();
    } else {
        println!("rm: {}: No such file or directory", args[0]);
    }
}