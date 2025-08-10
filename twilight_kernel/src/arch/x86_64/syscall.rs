use crate::sys::syscall::syscall_handler;
use crate::arch::x86_64::gdt::GdtEntryIndex;
use crate::{println};
use core::arch::{asm, naked_asm};

pub const IA32_EFER: u32 = 0xc0000080;
/// System Call Target Address (R/W).
pub const IA32_STAR: u32 = 0xc0000081;

/// IA-32e Mode System Call Target Address (R/W).
pub const IA32_LSTAR: u32 = 0xc0000082;

/// System Call Flag Mask (R/W).
pub const IA32_FMASK: u32 = 0xc0000084;

/// Wrapper function to the `wrmsr` assembly instruction used
/// to write 64 bits to msr register.
pub fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;

    unsafe { asm!("wrmsr", in("ecx") msr, in("eax") low, in("edx") high, options(nomem)) };
}

/// Wrapper function to the `rdmsr` assembly instruction used
// to read 64 bits msr register.
#[inline]
pub fn rdmsr(msr: u32) -> u64 {
    let (high, low): (u32, u32);

    unsafe { asm!("rdmsr", out("eax") low, out("edx") high, in("ecx") msr, options(nomem)) };

    ((high as u64) << 32) | (low as u64)
}

pub fn init() {
    // Enable support for `syscall` and `sysret` instructions if the current
    // CPU supports them and the target pointer width is 64.
    let kernel_code_sel = (GdtEntryIndex::KERNEL_CODE << 3) as u64;
    let user_code_sel = ((GdtEntryIndex::USER_CODE << 3) | 3) as u64;
    let star_val = ((user_code_sel & 0xffff) << 48) | ((kernel_code_sel & 0xffff) << 32);
    wrmsr(IA32_STAR, star_val);

    // LSTAR -> entry point address for syscall in 64-bit mode
    wrmsr(IA32_LSTAR, x86_64_syscall_handler as u64);

    // FMASK -> which RFLAGS bits to clear on syscall entry. Usually clear IF (bit 9).
    // Clear IF only:
    wrmsr(IA32_FMASK, 1 << 9);

    // Enable EFER.SCE
    let efer = rdmsr(IA32_EFER);
    wrmsr(IA32_EFER, efer | 1);

    // Optional debug read-back
    let rstar = rdmsr(IA32_STAR);
    let rlstar = rdmsr(IA32_LSTAR);
    let rfmask = rdmsr(IA32_FMASK);
    let refer = rdmsr(IA32_EFER);
    println!(
        "STAR={:#x} LSTAR={:#x} FMASK={:#x} EFER={:#x}",
        rstar, rlstar, rfmask, refer
    );
}

#[unsafe(naked)]
#[allow(named_asm_labels)]
unsafe extern "C" fn x86_64_syscall_handler() {
    naked_asm!(
    // make the GS base point to the kernel TLS
    "push rax",
    "push rcx",
    "push rdx",
    "push rsi",
    "push rdi",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "mov rsi, rsp", // Arg #2: register list
    "mov rdi, rsp", // Arg #1: interupt frame
    "add rdi, 9 * 8", // 9 registers * 8 bytes
    "call {x86_64_do_syscall}",

    "cmp rax, 0x10000000",
    "je terminate",

    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rdi",
    "pop rsi",
    "pop rdx",
    "pop rcx",
    "pop rax",
    "iretq",

    "terminate:",
    "add rsp, 9 * 8",
    // At this point, we're back in kernel mode and can handle process cleanup
    "call {terminate_process_cleanup}",
    // This should not return - the cleanup function should handle switching
    // to another process or returning to kernel idle state
    "ud2", // Undefined instruction - should never reach here

    // constants:
    // userland_cs = const USER_CS.bits(),
    // userland_ss = const USER_SS.bits(),
    // // XXX: add 8 bytes to skip the x86_64 cpu local self ptr
    // tss_temp_ustack_off = const offset_of!(Tss, reserved2) + core::mem::size_of::<usize>(),
    // tss_rsp0_off = const offset_of!(Tss, rsp) + core::mem::size_of::<usize>(),
    x86_64_do_syscall = sym syscall_handler,
    terminate_process_cleanup = sym terminate_process_cleanup,
    )
}

extern "C" fn terminate_process_cleanup() -> ! {
    println!("Process terminated via sys_exit, cleaning up...");

    unsafe {
        loop {
            asm!("hlt");
        }
    }
}