extern crate alloc;

use crate::{print, println};
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

pub static STDIO: Mutex<String> = Mutex::new(String::new());
pub static CURSOR_POSITION: Mutex<usize> = Mutex::new(0);

pub fn start_kernel_console() {
    print!("twilight > ");
    let mut cur_pos = CURSOR_POSITION.lock();
    *cur_pos = 2;
}

pub fn get_stdio_keypress(c: char) {
    match c {
        '\n' => {
            print!("\n");
            let mut cmd_line = STDIO.lock();
            let args: Vec<&str> = cmd_line.split_whitespace().collect();

            if args.len() > 1 {
                exec(args[0], &args[1..]);
            } else if !args.is_empty() {
                exec(args[0], &[]);
            }
            cmd_line.clear();
            start_kernel_console();
        }
        '\x08' => {
            if *CURSOR_POSITION.lock() > 2 {
                print!("{}", c);
                let mut cmd_line = STDIO.lock();
                if cmd_line.trim().len() > 0 {
                    cmd_line.pop();
                }
                *CURSOR_POSITION.lock() -= 1;
            }
        }
        _ => {
            print!("{}", c);
            STDIO.lock().push(c);
            let mut cur_pos = CURSOR_POSITION.lock();
            *cur_pos += 1;
        }
    };
}

fn exec(cmd: &str, args: &[&str]) {
    match cmd {
        "echo" => crate::kernel_utils::echo::main(args),
        "clear" => {
            crate::framebuffer::clear_screen(true);
        }
        "uptime" => {
            println!("{:.6} seconds", crate::driver::timer::pit::uptime());
        }
        "date" => crate::kernel_utils::date::main(),
        "meminfo" => crate::kernel_utils::meminfo::main(),
        "uname" => {
            println!("TwilightOS Twilight-Kernel 0.1 DevBuild (23/03/25)")
        }
        _ => {
            println!("{}: not a command", cmd);
        }
    }
}
