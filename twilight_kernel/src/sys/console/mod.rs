extern crate alloc;

use crate::sys::console::writer::clear_screen;
use crate::{print, println, serial_prtinln};
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use crate::arch::x86_64::halt;
use crate::sys::tty::read_char;

pub mod writer;
pub mod font;

pub static STDIO: Mutex<String> = Mutex::new(String::new());
pub static CURSOR_POSITION: Mutex<usize> = Mutex::new(0);

pub static mut DIR: String = String::new();

static mut CONSOLE_HISTORY: Vec<String> = Vec::new();
static mut CONSOLE_HISTORY_INDEX: Mutex<usize> = Mutex::new(0);

pub fn init_console() {
    unsafe {
        DIR = String::from("/");
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
        let c = read_char();
        match c {
            '\n' => {
                print!("\n");
                let mut cmd_line = STDIO.lock();
                unsafe {
                    #[allow(static_mut_refs)]
                    CONSOLE_HISTORY.push(cmd_line.clone());
                }
                let args: Vec<&str> = cmd_line.split_whitespace().collect();

                if args.len() > 1 {
                    exec(args[0], &args[1..]);
                } else if !args.is_empty() {
                    exec(args[0], &[]);
                }
                cmd_line.clear();
                start_kernel_console();

                // reset history index
                #[allow(static_mut_refs)]
                let mut idx= unsafe { CONSOLE_HISTORY_INDEX.lock() };
                *idx = 0;
            }
            '\t' => {
                print!("{}", c);
                STDIO.lock().push(' ');
                STDIO.lock().push(' ');
                STDIO.lock().push(' ');
                STDIO.lock().push(' ');
                let mut cur_pos = CURSOR_POSITION.lock();
                *cur_pos += 4;
            }
            '\x08' => {
                if *CURSOR_POSITION.lock() > 2 {
                    print!("{}", c);
                    let mut cmd_line = STDIO.lock();
                    if !cmd_line.trim().is_empty() {
                        cmd_line.pop();
                    }
                    *CURSOR_POSITION.lock() -= 1;
                }
            }
            // up arrow key
            '\u{F700}' => {
                print!("\r");
                start_kernel_console();

                #[allow(static_mut_refs)]
                let mut idx= unsafe { CONSOLE_HISTORY_INDEX.lock() };

                #[allow(static_mut_refs)]
                let len = unsafe { CONSOLE_HISTORY.len() };

                // there must be some history to go back to & the index must be less than the length of the history
                if len != 0 && len > *idx {
                    #[allow(static_mut_refs)]
                    let cmd = unsafe { CONSOLE_HISTORY.get_unchecked(len - *idx - 1) };

                    serial_prtinln!("{}", *idx);

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
                let mut idx= unsafe { CONSOLE_HISTORY_INDEX.lock() };

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
            '\u{F702}' => {}
            '\u{F703}' => {}
            _ => {
                print!("{}", c);
                STDIO.lock().push(c);
                let mut cur_pos = CURSOR_POSITION.lock();
                *cur_pos += 1;
            }
        };
    }
}

fn exec(cmd: &str, args: &[&str]) {
    match cmd {
        "echo" => crate::kernel_utils::echo::main(args),
        "clear" => {
            clear_screen(true);
        }
        "uptime" => {
            println!("{:.6} seconds", crate::driver::timer::pit::uptime());
        }
        "shutdown" => crate::kernel_utils::shutdown::main(),
        "date" => crate::kernel_utils::date::main(),
        "meminfo" => crate::kernel_utils::meminfo::main(),
        "uname" => {
            println!("TwilightOS Twilight-Kernel 0.1 DevBuild (23/03/25)")
        },
        "dealloc" => crate::kernel_utils::dealloc::main(args),
        "allocate" => crate::kernel_utils::allocate::main(),
        "mkfs" => crate::kernel_utils::mkfs::main(args),
        "cat" => crate::kernel_utils::cat::main(args),
        "ls" => crate::kernel_utils::ls::main(args, "/"),
        "pitch" => {
            println!("{}", crate::sys::framebuffer::get_pitch());
        },
        "df" => crate::kernel_utils::df::main(args),
        "touch" => crate::kernel_utils::touch::main(args),
        "mkdir" => crate::kernel_utils::mkdir::main(args),
        "cd" => crate::kernel_utils::cd::main(args),
        "rm" => crate::kernel_utils::rm::main(args),
        "readelf" => crate::kernel_utils::readelf::main(args),
        "install" => crate::kernel_utils::install::main(),
        "exec" => crate::kernel_utils::exec::main(args),
        "dhcp" => crate::kernel_utils::dhcp::main(),
        "vi" => crate::kernel_utils::vi::main(args),
        _ => {
            println!("{}: not a command", cmd);
        }
    }
}
