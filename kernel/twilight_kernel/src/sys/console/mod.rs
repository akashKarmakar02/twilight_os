extern crate alloc;
pub(crate) use crate::sys::console::tty::{Tty, get_tty};
use crate::sys::fs::vfs::VFS;
use crate::sys::proc::{PROCESS_TABLE, Process};
use alloc::string::String;
use crate::utils::sync::Mutex;
use spin::Once;

pub mod font;
pub mod framebuffer;
pub mod tty;

pub static STDIO: Mutex<String> = Mutex::new(String::new());
pub static CURSOR_POSITION: Mutex<usize> = Mutex::new(0);
pub static mut TTY: Once<Tty> = Once::new();

pub static mut DIR: String = String::new();

pub fn init_tty() {
    #[allow(static_mut_refs)]
    unsafe {
        TTY.call_once(|| {
            let tty = Tty::new();
            let mut cur_pos = CURSOR_POSITION.lock();
            *cur_pos = 2;
            tty
        });
    }
}

pub fn put_char_in_tty(c: u8) {
    let tty = get_tty();
    tty.put_input(c);
    // Wake processes that are blocked waiting for TTY input
    crate::sys::console::tty::notify_input_waiters();
}

pub fn init_console() {
    #[allow(static_mut_refs)]
    let fs = unsafe { VFS.get_mut() };
    let init = fs
        .open("/sbin/twinit")
        .map(|node| (node, "/sbin/twinit"))
        .or_else(|_| fs.open("/bin/twinit").map(|node| (node, "/bin/twinit")))
        .or_else(|_| fs.open("/bin/init").map(|node| (node, "/bin/init")));
    drop(fs);

    if let Ok((node, path)) = init {
        #[allow(static_mut_refs)]
        let process_table = unsafe { PROCESS_TABLE.get_mut_unchecked() };

        process_table.run(Process::new(node, path, "/", &[path], 0).unwrap());
    }
}
