use crate::arch::x86_64::gdt::GdtEntryIndex;
use crate::arch::x86_64::gdt::{USER_CS, USER_SS};
use crate::sys::syscall::syscall_handler;
use core::arch::{asm, naked_asm};
use raw_cpuid::CpuId;

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
    let cpuid = CpuId::new();

    // Check if syscall is supported as it is a required CPU feature for aero to run.
    let has_syscall = cpuid
        .get_extended_processor_and_feature_identifiers()
        .map_or(false, |i| i.has_syscall_sysret());

    assert!(has_syscall);

    // Enable support for `syscall` and `sysret` instructions if the current
    // CPU supports them and the target pointer width is 64.

    let syscall_base = GdtEntryIndex::KERNEL_CODE << 3;
    let sysret_base = (GdtEntryIndex::KERNEL_TLS << 3) | 3;

    let star_hi = syscall_base as u32 | (sysret_base as u32) << 16;

    wrmsr(IA32_STAR, (star_hi as u64) << 32);

    // LSTAR -> entry point address for syscall in 64-bit mode
    wrmsr(IA32_LSTAR, x86_64_syscall_handler as u64);

    // FMASK -> which RFLAGS bits to clear on syscall entry. Usually clear IF (bit 9).
    // Clear IF only:
    wrmsr(IA32_FMASK, 0x300); // 0x40700

    // Enable EFER.SCE
    let efer = rdmsr(IA32_EFER);
    wrmsr(IA32_EFER, efer | 1);
}

#[unsafe(naked)]
#[allow(named_asm_labels)]
unsafe extern "C" fn x86_64_syscall_handler() {
    naked_asm!(
    "swapgs",

    "mov qword ptr gs:[0x08], rsp",
    // restore kernel stack
    "mov rsp, qword ptr gs:[0x00]",
    "push {userland_ss}",
    // push userspace stack ptr
    "push qword ptr gs:[0x08]",
    "push r11",
    "push {userland_cs}",
    "push rcx",

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

    "cld",
    "call {x86_64_do_syscall}",
    "cli",

    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rdi",
    "pop rsi",
    "pop rdx",
    "pop rcx",
    "pop rax",

    "pop rcx",
    "add rsp, 8",
    "pop r11",
    "pop rsp",
    // "mov rsp, qword ptr gs:[0x08]",

    // restore user stack register
    "swapgs",
    "sysretq",

    // constants:
    userland_cs = const USER_CS.bits(),
    userland_ss = const USER_SS.bits(),
    // tss_temp_ustack_off = const offset_of!(Tss, reserved2) + core::mem::size_of::<usize>(),
    // tss_rsp0_off = const offset_of!(Tss, rsp) + core::mem::size_of::<usize>(),
    x86_64_do_syscall = sym syscall_handler,
    )
}
