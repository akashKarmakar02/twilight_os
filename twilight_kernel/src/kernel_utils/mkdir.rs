use crate::println;
use crate::sys::console::DIR;
use crate::sys::fs::vfs::VFS;

pub fn main(args: &[&str]) {
    #[allow(static_mut_refs)]
    let pwd = unsafe { DIR.as_str() };

    #[allow(static_mut_refs)]
    if let Err(_) = unsafe { VFS.get_mut().mkdir(pwd, args[1]) } {
        println!("mkdir: {}: Failed to create", args[1]);
    }
}
