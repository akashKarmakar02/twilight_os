use crate::println;
use crate::sys::console::DIR;
use crate::sys::fs::vfs::VFS;

pub fn main(args: &[&str]) {
    #[allow(static_mut_refs)]
    let pwd = unsafe { DIR.as_str() };

    #[allow(static_mut_refs)]
    if let Err(_) = unsafe { VFS.get_mut().touch(pwd, args[1]) } {
        println!("touch: {}: File exists", args[1]);
    }
}
