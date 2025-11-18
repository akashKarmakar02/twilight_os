extern crate alloc;

use crate::arch::x86_64::halt;
use crate::driver::disk::dummy_blockdev;
pub(crate) use crate::sys::console::tty::{Tty, get_tty};
use crate::sys::fs::vfs::{VFS, VfsNodeOps};
use crate::sys::proc::{PROCESS_TABLE, Process};
use crate::{print, println, serial_println};
use alloc::collections::VecDeque;
use alloc::{format, vec};
use alloc::string::String;
use alloc::vec::Vec;
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

struct PipelineState {
    input: Option<VecDeque<u8>>,
    capture_output: bool,
    output: Vec<u8>,
}

static PIPELINE_STATE: Mutex<Option<PipelineState>> = Mutex::new(None);

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

                    serial_println!("{}", *idx);

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
        "mkdir" => crate::kernel_utils::mkdir::main(args),
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

    let segments: Vec<&str> = cmd_line
        .split('|')
        .filter(|seg| seg.trim() != "")
        .map(|seg| seg.trim())
        .collect();

    serial_println!("{:?}", segments);

    if segments.len() == 1 {
        let args: Vec<&str> = segments[0].split_whitespace().collect();
        if !args.is_empty() {
            exec(args[0], &args);
        }
        return;
    }

    let mut pending_output: Option<Vec<u8>> = None;

    for (idx, segment) in segments.iter().enumerate() {
        if segment.is_empty() {
            println!("pipeline: empty command");
            PIPELINE_STATE.lock().take();
            return;
        }

        let args: Vec<&str> = segment.split_whitespace().collect();
        if args.is_empty() {
            println!("pipeline: empty command");
            PIPELINE_STATE.lock().take();
            return;
        }

        let capture_output = idx < segments.len() - 1;

        start_pipeline_stage(pending_output.take(), capture_output);
        exec(args[0], &args);

        let stage_output = finish_pipeline_stage();
        if capture_output {
            pending_output = Some(stage_output.unwrap_or_default());
        } else {
            pending_output = None;
        }
    }
}

fn start_pipeline_stage(input: Option<Vec<u8>>, capture_output: bool) {
    if !capture_output && input.is_none() {
        PIPELINE_STATE.lock().take();
        return;
    }

    let pipeline_input = input.map(VecDeque::from);

    let mut state = PIPELINE_STATE.lock();
    *state = Some(PipelineState {
        input: pipeline_input,
        capture_output,
        output: Vec::new(),
    });
}

fn finish_pipeline_stage() -> Option<Vec<u8>> {
    let mut state = PIPELINE_STATE.lock();
    state.take().and_then(|stage| {
        if stage.capture_output {
            Some(stage.output)
        } else {
            None
        }
    })
}

pub(crate) fn pipeline_read(buf: &mut [u8]) -> Option<usize> {
    let mut state = PIPELINE_STATE.lock();
    let stage = state.as_mut()?;
    let input = stage.input.as_mut()?;

    let mut bytes_read = 0;
    while bytes_read < buf.len() {
        match input.pop_front() {
            Some(byte) => {
                buf[bytes_read] = byte;
                bytes_read += 1;
            }
            None => break,
        }
    }

    Some(bytes_read)
}

pub(crate) fn pipeline_write(data: &[u8]) -> bool {
    let mut state = PIPELINE_STATE.lock();
    if let Some(stage) = state.as_mut() {
        if stage.capture_output {
            stage.output.extend_from_slice(data);
            return true;
        }
    }

    false
}
