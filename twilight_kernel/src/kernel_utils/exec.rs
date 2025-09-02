use crate::println;
use crate::sys::console::DIR;
use crate::sys::fs::vfs::VFS;
use crate::sys::proc::{Process, PROCESS_TABLE};
use alloc::format;
use conquer_once::spin::OnceCell;
use core::arch::asm;
use spin::Mutex;
use x86_64::structures::paging::PageTable;

pub static PREVIOUS_TABLE: OnceCell<Mutex<PageTable>> = OnceCell::uninit();

pub fn main(args: &[&str]) {
    if args.len() < 1 {
        println!("Usage: exec <file>");
        return;
    }

    let content_buf;
    {
        #[allow(static_mut_refs)]
        let pwd = unsafe { DIR.as_str() };

        let path = format!("{}/{}", pwd, args[0]);

        #[allow(static_mut_refs)]
        if let Ok(buf) = unsafe { VFS.get_mut().read(path.as_str()) } {
            content_buf = buf;
        } else {
            println!("exec: file not found");
            return;
        }
    }
    
    
    let process = Process::new(content_buf.clone());

    #[allow(static_mut_refs)]
    unsafe {
        PROCESS_TABLE.get_mut().unwrap().run(process);
    }
}

pub fn jump_to_user(code_addr: u64, entry_point: u64, stack_top: u64, user_cs: u64, user_ss: u64) {
    use x86_64::registers::control::Cr3;

    let (_, flags) = Cr3::read();
    unsafe { Cr3::write(crate::sys::memory::get_page_table_frame(), flags) };

    let rip = code_addr + entry_point;
    unsafe {
        asm!(
        "cli",              // Disable interrupts
        "push {ss}",        // SS (user data segment)
        "push {stack}",     // RSP (stack pointer)
        "push 0x202",       // RFLAGS (IF = 1 | bit 1 always set)
        "push {cs}",        // CS (user code segment)
        "push {rip}",       // RIP (entry point)
        "iretq",            // Return to ring 3
        ss = in(reg) user_ss,
        stack = in(reg) stack_top,
        cs = in(reg) user_cs,
        rip = in(reg) rip,
        options(noreturn)
        );
    }
}

#[allow(dead_code)]
fn read_binary(vaddr: u64, len: usize) -> &'static [u8] {
    unsafe { core::slice::from_raw_parts(vaddr as *const u8, len) }
}
