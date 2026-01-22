extern crate alloc;

use crate::arch::x86_64::halt;
use crate::driver::disk::dummy_blockdev;
pub(crate) use crate::sys::console::tty::{Tty, get_tty};
use crate::sys::fs::vfs::{VFS, VfsNodeOps};
use crate::sys::proc::{PROCESS_TABLE, Process};
use crate::{print, println};
use alloc::string::String;
use alloc::vec::Vec;
use alloc::{format, vec};
use spin::{Mutex, Once};

pub mod font;
pub mod framebuffer;
pub mod tty;

pub static STDIO: Mutex<String> = Mutex::new(String::new());
pub static CURSOR_POSITION: Mutex<usize> = Mutex::new(0);
pub static mut TTY: Once<Tty> = Once::new();

pub static mut DIR: String = String::new();

static mut CONSOLE_HISTORY: Vec<String> = Vec::new();
static mut CONSOLE_HISTORY_INDEX: Mutex<usize> = Mutex::new(0);

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
}

pub fn init_console() {
    #[allow(static_mut_refs)]
    unsafe {
        if DIR.is_empty() {
            DIR = String::from("/");
        }
    }
    handle_console_input();
}

pub fn start_kernel_console() {
    #[allow(static_mut_refs)]
    let dir = unsafe { DIR.as_str() };
    print!("\x1b[92mtwilight:{} $\x1b[0m ", dir);
    let mut cur_pos = CURSOR_POSITION.lock();
    *cur_pos = 2;
}

fn handle_console_input() {
    start_kernel_console();
    loop {
        halt();
        let mut buf = [0u8; 1];
        unsafe {
            get_tty()
                .read(&mut dummy_blockdev(), 0, &mut buf)
                .unwrap_unchecked();
        }
        let c = buf[0] as char;
        match c {
            '\u{8}' | '\u{7F}' => {
                if !STDIO.lock().is_empty() {
                    STDIO.lock().pop();
                    let mut cur_pos = CURSOR_POSITION.lock();
                    *cur_pos -= 1;
                }
            }
            '\n' => {
                let cmd_line;
                {
                    let mut stdio = STDIO.lock();
                    cmd_line = stdio.clone();
                    unsafe {
                        #[allow(static_mut_refs)]
                        CONSOLE_HISTORY.push(stdio.clone());
                    }

                    stdio.clear();
                }
                execute_command_line(&cmd_line);
                start_kernel_console();

                // reset history index
                #[allow(static_mut_refs)]
                let mut idx = unsafe { CONSOLE_HISTORY_INDEX.lock() };
                *idx = 0;
            }
            '\t' => {
                STDIO.lock().push(' ');
                STDIO.lock().push(' ');
                STDIO.lock().push(' ');
                STDIO.lock().push(' ');
                let mut cur_pos = CURSOR_POSITION.lock();
                *cur_pos += 4;
            }
            // up arrow key
            '\u{F700}' => {
                print!("\r");
                start_kernel_console();

                #[allow(static_mut_refs)]
                let mut idx = unsafe { CONSOLE_HISTORY_INDEX.lock() };

                #[allow(static_mut_refs)]
                let len = unsafe { CONSOLE_HISTORY.len() };

                // there must be some history to go back to & the index must be less than the length of the history
                if len != 0 && len > *idx {
                    #[allow(static_mut_refs)]
                    let cmd = unsafe { CONSOLE_HISTORY.get_unchecked(len - *idx - 1) };

                    let mut stdio = STDIO.lock();
                    stdio.clear();
                    stdio.push_str(cmd);

                    print!("{}", cmd);

                    // incrementing the history index
                    *idx += 1;
                    // changing the cursor position so backspace works
                    *CURSOR_POSITION.lock() = 2 + cmd.len();
                }
            }
            // down arrow key
            '\u{F701}' => {
                print!("\r");
                start_kernel_console();

                #[allow(static_mut_refs)]
                let mut idx = unsafe { CONSOLE_HISTORY_INDEX.lock() };

                #[allow(static_mut_refs)]
                let len = unsafe { CONSOLE_HISTORY.len() };

                if len != 0 && len >= *idx && *idx > 0 {
                    #[allow(static_mut_refs)]
                    let cmd = unsafe { CONSOLE_HISTORY.get(len - *idx) };

                    let mut stdio = STDIO.lock();
                    stdio.clear();

                    if let Some(cmd) = cmd {
                        stdio.push_str(cmd);

                        print!("{}", cmd);

                        // changing the cursor position so backspace works
                        *CURSOR_POSITION.lock() = 2 + cmd.len();

                        *idx -= 1;
                    }
                }
            }
            '\u{F702}' => {
                let tty = get_tty();
                tty.move_cursor_left();
            }
            '\u{F703}' => {
                let tty = get_tty();
                tty.move_cursor_right();
            }
            _ => {
                STDIO.lock().push(c);
                let mut cur_pos = CURSOR_POSITION.lock();
                *cur_pos += 1;
            }
        };
    }
}

fn exec(cmd: &str, args: &[&str]) {
    match cmd {
        "uptime" => {
            println!("{:.6} seconds", crate::driver::timer::pit::uptime());
        }
        "shutdown" => crate::kernel_utils::shutdown::main(),
        "meminfo" => crate::kernel_utils::meminfo::main(),
        "pitch" => {
            println!("{}", crate::sys::framebuffer::get_pitch());
        }
        "gs" => crate::kernel_utils::gs::main(),
        "df" => crate::kernel_utils::df::main(args),
        "cd" => crate::kernel_utils::cd::main(args),
        "readelf" => crate::kernel_utils::readelf::main(args),
        "install" => crate::kernel_utils::install::main(),
        "dhcp" => crate::kernel_utils::dhcp::main(),
        "anirect" => crate::kernel_utils::anirect::main(),
        "curl" => crate::kernel_utils::curl::main(args),
        "serve" => crate::kernel_utils::serve::main(args),
        _ => {
            #[allow(static_mut_refs)]
            let fs = unsafe { VFS.get_mut() };

            if let Ok(mut node) =
                fs.open(format!("/bin/{}", cmd.split_whitespace().next().unwrap()).as_str())
            {
                let mut buf = vec![0u8; node.metadata.size];
                let Ok(_) = node.read(0, &mut buf) else {
                    println!("{}: failed to read from file", cmd);
                    return;
                };

                #[allow(static_mut_refs)]
                if let Ok(process) = Process::new(buf.clone(), unsafe { DIR.as_str() }, args, 1) {
                    unsafe {
                        PROCESS_TABLE.get_mut().unwrap().run(process);
                    }
                }
            } else {
                println!("{}: not a command", cmd);
            }
        }
    }
}

fn execute_command_line(cmd_line: &str) {
    if cmd_line.trim().is_empty() {
        return;
    }
    // Kernel console does not support pipelines. (Userspace shell `tsh` does.)
    let args: Vec<&str> = cmd_line.split_whitespace().collect();
    if !args.is_empty() {
        exec(args[0], &args);
    }
}
