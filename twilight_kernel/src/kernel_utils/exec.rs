use conquer_once::spin::OnceCell;
use core::arch::asm;
use spin::Mutex;
use x86_64::structures::paging::PageTable;

pub static PREVIOUS_TABLE: OnceCell<Mutex<PageTable>> = OnceCell::uninit();

pub fn jump_to_user(entry_point: u64, stack_top: u64, user_cs: u64, user_ss: u64) {
    let rip = entry_point;
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
