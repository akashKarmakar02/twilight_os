use crate::println;
use crate::sys::fs::vfs::VFS;
use crate::sys::console;

pub fn main(_args: &[&str], _ctx: &str) {
    #[allow(static_mut_refs)]
    let pwd = unsafe { console::DIR.as_str() };

    #[allow(static_mut_refs)]
    if let Ok(entries) = unsafe { VFS.get_mut().ls(pwd) } {
        for entry in entries {
            println!("{}", entry);
        }
    }
}
