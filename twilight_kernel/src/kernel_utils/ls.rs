use crate::println;
use crate::sys::fs::vfs::VFS;
use crate::sys::console;

pub fn main(args: &[&str]) {
    let pwd = if args.len() > 0 {
        args[0]
    } else {
        #[allow(static_mut_refs)]
        unsafe { console::DIR.as_str() } 
    };

    #[allow(static_mut_refs)]
    if let Ok(entries) = unsafe { VFS.get_mut().ls(pwd) } {
        for entry in entries {
            println!("{}", entry);
        }
    }
}
