use crate::arch::x86_64::io::{IA32_FS_BASE, IA32_GS_BASE, rdmsr, wrmsr};
use crate::sys::fs::vfs::VFS;
use crate::sys::proc::{Handler, PROCESS_TABLE};
use crate::sys::tty::{read_char, read_line};
use crate::{print, serial_prtinln};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use core::arch::asm;
use spin::mutex::Mutex;
use twilight_common::syscall::types::*;

pub fn write(arg1: i32, arg2: usize, arg3: usize) -> i64 {
    let file_descriptor = arg1;
    let buf = arg2 as *const u8;
    let len = arg3;
    let buf = unsafe { core::slice::from_raw_parts(buf, len) };

    let res = match file_descriptor {
        1 => {
            print!("{}", String::from_utf8_lossy(buf));

            len as i64
        }
        2 => {
            print!("\x1b[91m{}\x1b[0m", String::from_utf8_lossy(buf));

            len as i64
        }
        n => {
            #[allow(static_mut_refs)]
            let process = unsafe {
                PROCESS_TABLE
                    .get_mut()
                    .unwrap()
                    .get_process(crate::sys::proc::id())
                    .unwrap()
            };

            if let Some(node) = process.handler.get_mut(n as usize - 3) {
                if let Ok(_) = node.handler.lock().write(buf) {
                    return len as i64;
                }
            }

            -1
        }
    };

    res
}

pub fn read(handler: usize, buf: &mut [u8], len: usize) -> i64 {
    if handler == 0 || handler <= 2 {
        let mut str = if len != 1 {
            read_line()
        } else {
            let res = String::from(read_char());
            print!("{}", res);
            res
        };
        str.truncate(len);

        let string_bytes = str.as_bytes();
        let copy_len = string_bytes.len().min(buf.len());
        buf[..copy_len].copy_from_slice(&string_bytes[..copy_len]);

        return copy_len as i64;
    }

    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
            .unwrap()
    };

    if let Some(node) = process.handler.get_mut(handler - 3) {
        let seek = node.seek;
        if let Ok(content) = node.handler.lock().read() {
            let copy_len = if seek < content.len() {
                (content.len() - seek).min(buf.len())
            } else {
                0
            };
            if copy_len > 0 {
                buf[..copy_len].copy_from_slice(&content[seek..(seek + copy_len)]);
            }
            node.seek += copy_len;
            return copy_len as i64;
        }
    }

    -1
}

#[allow(dead_code)]
pub fn open(path: &str, _flags: i32, _mode: u32) -> i64 {
    #[allow(static_mut_refs)]
    if let Some(process) = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    } {
        let node = if path.starts_with("/") {
            #[allow(static_mut_refs)]
            unsafe {
                VFS.get_mut().open(path)
            }
        } else {
            #[allow(static_mut_refs)]
            unsafe {
                VFS.get_mut()
                    .open(format!("{}/{}", process.pwd, path).as_str())
            }
        };

        if let Ok(node) = node {
            let handler = process.handler.len() + 3;
            let h = Box::leak(Box::new(Handler {
                handler: Arc::new(Mutex::new(node)),
                seek: 0,
            }));
            process.handler.push(h);
            return handler as i64;
        }
    }
    -1
}

pub fn exit() -> i64 {
    unsafe { asm!("swapgs") };

    crate::sys::proc::exit();

    0
}

pub fn uname(ptr: usize) -> i64 {
    let uname_ptr = ptr as *mut UtsName;

    fn fill(buf: &mut [u8; 65], s: &str) {
        buf.fill(0);
        let bytes = s.as_bytes();
        let n = core::cmp::min(bytes.len(), 64); // leave room for NUL
        buf[..n].copy_from_slice(&bytes[..n]);
        buf[n] = 0;
    }

    unsafe {
        let uname_s = &mut *uname_ptr;

        fill(&mut uname_s.sysname, "TwilightOS");
        fill(&mut uname_s.nodename, "twilight");
        fill(&mut uname_s.release, "0.1.0-testing-build.x86_64");
        fill(&mut uname_s.version, "#1 NON-SMP 09-09-2025");
        fill(&mut uname_s.machine, "x86_64");
        fill(&mut uname_s.domainname, "-");
    }

    0
}

pub fn arch_prctl(code: u64, addr: u64) -> i64 {
    serial_prtinln!("LOG: code: {}, addr: {}", code, addr);
    match code {
        ARCH_SET_FS => {
            wrmsr(IA32_FS_BASE, addr);
            0
        }
        ARCH_GET_FS => rdmsr(IA32_FS_BASE) as i64,
        ARCH_SET_GS => {
            wrmsr(IA32_GS_BASE, addr);
            0
        }
        ARCH_GET_GS => rdmsr(IA32_GS_BASE) as i64,
        _ => -1,
    }
}

pub fn writev(fd: i32, iov_ptr: u64, iovcnt: i32) -> i64 {
    if iovcnt < 0 {
        return -1;
    }
    let n = iovcnt as usize;

    // SAFETY: trusting user pointers here; in production, copy to kernel buffer
    let iov = unsafe { core::slice::from_raw_parts(iov_ptr as *const Iovec, n) };

    let mut total: i64 = 0;
    for iv in iov {
        // Skip empty segments
        if iv.iov_len == 0 {
            continue;
        }

        // Write this segment
        let r = write(fd, iv.iov_base as usize, iv.iov_len);
        total = total.saturating_add(r);

        // Stop on partial write (short write semantics)
        if (r as usize) < iv.iov_len {
            break;
        }
    }
    total
}
