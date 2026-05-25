use crate::arch::x86_64::io;
use crate::sys::memory::bitmap::with_frame_allocator;
use crate::sys::memory::phys_to_virt;
use crate::sys::proc::Process;
use x86_64::instructions::interrupts;
use x86_64::structures::paging::mapper::MapToError;
use x86_64::structures::paging::{FrameAllocator, Size4KiB};
use x86_64::VirtAddr;

#[derive(Default)]
#[repr(C)]
pub struct Context {
    pub cr3: u64,

    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,

    pub rbx: u64,
    pub rbp: u64,

    pub rip: u64,
}

#[unsafe(naked)]
pub unsafe extern "C" fn task_spinup(prev: &mut *mut Context, next: *mut Context) {
    core::arch::naked_asm!(
        // save callee-saved registers
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        // save CR3
        "mov rax, cr3",
        "push rax",
        // save old RSP (type now matches: &mut *mut Context)
        "mov [rdi], rsp",
        // switch to new stack
        "mov rsp, rsi",
        "pop rax",
        "mov cr3, rax",
        // restore callee-saved registers
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        // resume the next thread
        "ret",
    );
}

#[derive(Debug, Copy, Clone)]
#[repr(C, align(16))]
pub struct FpuState {
    /// x87 FPU Control Word (16 bits). See Figure 8-6 in the Intel® 64 and IA-32 Architectures
    /// Software Developer’s Manual Volume 1, for the layout of the x87 FPU control word.
    pub fcw: u16,
    /// x87 FPU Status Word (16 bits).
    pub fsw: u16,
    /// x87 FPU Tag Word (8 bits) + reserved (8 bits).
    pub ftw: u16,
    /// x87 FPU Opcode (16 bits).
    pub fop: u16,
    /// x87 FPU Instruction Pointer Offset ([31:0]). The contents of this field differ depending on
    /// the current addressing mode (32-bit, 16-bit, or 64-bit) of the processor when the
    /// FXSAVE instruction was executed: 32-bit mode — 32-bit IP offset. 16-bit mode — low 16
    /// bits are IP offset; high 16 bits are reserved. 64-bit mode with REX.W — 64-bit IP
    /// offset. 64-bit mode without REX.W — 32-bit IP offset.
    pub fip: u32,
    /// x87 FPU Instruction Pointer Selector (16 bits) + reserved (16 bits).
    pub fcs: u32,
    /// x87 FPU Instruction Operand (Data) Pointer Offset ([31:0]). The contents of this field
    /// differ depending on the current addressing mode (32-bit, 16-bit, or 64-bit) of the
    /// processor when the FXSAVE instruction was executed: 32-bit mode — 32-bit DP offset.
    /// 16-bit mode — low 16 bits are DP offset; high 16 bits are reserved. 64-bit mode
    /// with REX.W — 64-bit DP offset. 64-bit mode without REX.W — 32-bit DP offset.
    pub fdp: u32,
    /// x87 FPU Instruction Operand (Data) Pointer Selector (16 bits) + reserved.
    pub fds: u32,
    /// MXCSR Register State (32 bits).
    pub mxcsr: u32,
    /// This mask can be used to adjust values written to the MXCSR register, ensuring that
    /// reserved bits are set to 0. Set the mask bits and flags in MXCSR to the mode of
    /// operation desired for SSE and SSE2 SIMD floating-point instructions.
    pub mxcsr_mask: u32,
    /// x87 FPU or MMX technology registers. Layout: [12 .. 9 | 9 ... 0] LHS = reserved; RHS = mm.
    pub mm: [u128; 8],
    /// XMM registers (128 bits per field).
    pub xmm: [u128; 16],
    /// reserved.
    pub _pad: [u64; 12],
}

impl Default for FpuState {
    fn default() -> Self {
        Self {
            mxcsr: 0x1f80,
            mxcsr_mask: 0x037f,
            // rest are zeroed
            fcw: 0,
            fsw: 0,
            ftw: 0,
            fop: 0,
            fip: 0,
            fcs: 0,
            fdp: 0,
            fds: 0,
            mm: [0; 8],
            xmm: [u128::MAX; 16],
            _pad: [0; 12],
        }
    }
}

pub fn xsave(fpu: &mut FpuState) {
    // The implicit EDX:EAX register pair specifies a 64-bit instruction mask. The specific state
    // components saved correspond to the bits set in the requested-feature bitmap (RFBM), which is
    // the logical-AND of EDX:EAX and XCR0.
    // unsafe {
    //     asm!("xsave64 [{}]", in(reg) fpu.as_ptr(), in("eax") u32::MAX, in("edx") u32::MAX,
    // options(nomem, nostack)) }

    use core::arch::x86_64::_fxsave64;

    unsafe { _fxsave64((fpu as *mut FpuState).cast()) }
}

pub fn xrstor(fpu: &FpuState) {
    // unsafe {
    //     asm!("xrstor [{}]", in(reg) fpu.as_ptr(), in("eax") u32::MAX, in("edx") u32::MAX,
    // options(nomem, nostack)); }
    use core::arch::x86_64::_fxrstor64;

    unsafe { _fxrstor64((fpu as *const FpuState).cast()) }
}

pub fn allocate_switch_stack() -> Result<VirtAddr, MapToError<Size4KiB>> {
    let mut first_phys_addr = None;

    with_frame_allocator(|frame_allocator| {
        for _ in 0..4 {
            if first_phys_addr.is_none() {
                first_phys_addr = Some(frame_allocator.allocate_frame().unwrap());
            } else {
                frame_allocator.allocate_frame();
            }
        }
    });

    let stack_virt_addr = phys_to_virt(first_phys_addr.unwrap().start_address()) + (4096 * 4);

    Ok(stack_virt_addr)
}


pub fn switch_tasks(prev_task: &mut Process, next_task: &mut Process) {
    interrupts::without_interrupts(|| unsafe {
        crate::serial_println!(
            "[switch] enter prev pid={} state={:?} ctx={:p} k_rsp={:#x} preempt={:#x} next pid={} state={:?} ctx={:p} k_rsp={:#x} preempt={:#x}",
            prev_task.pid,
            prev_task.state,
            prev_task.context,
            prev_task.kernel_gs.kernel_rsp,
            prev_task.preempt_frame,
            next_task.pid,
            next_task.state,
            next_task.context,
            next_task.kernel_gs.kernel_rsp,
            next_task.preempt_frame,
        );

        log_context("prev", prev_task.context);
        log_context("next", next_task.context);

        if let Some(fpu) = prev_task.fpu_storage.as_mut() {
            crate::serial_println!("[switch] xsave prev pid={}", prev_task.pid);
            xsave(fpu);
            crate::serial_println!("[switch] xsave prev done pid={}", prev_task.pid);
        } else {
            crate::serial_println!("[switch] xsave prev skipped pid={}", prev_task.pid);
        }

        if let Some(fpu) = next_task.fpu_storage.as_mut() {
            crate::serial_println!("[switch] xrstor next pid={}", next_task.pid);
            xrstor(fpu);
            crate::serial_println!("[switch] xrstor next done pid={}", next_task.pid);
        } else {
            crate::serial_println!("[switch] xrstor next skipped pid={}", next_task.pid);
        }

        // Keep SYSENTER/TSS pointed at the top of the next task's kernel stack, not
        // the saved context frame.
        let kstack_top = next_task.kernel_gs.kernel_rsp;
        crate::serial_println!("[switch] set kernel rsp pid={} rsp={:#x}", next_task.pid, kstack_top);
        crate::arch::x86_64::gdt::TSS.rsp[0] = kstack_top;
        io::wrmsr(io::IA32_SYSENTER_ESP, kstack_top);
        crate::serial_println!("[switch] kernel rsp done pid={}", next_task.pid);

        crate::serial_println!("[switch] save prev fs/gs pid={}", prev_task.pid);
        prev_task.fs_base = io::get_fsbase()();
        prev_task.gs_base = io::get_inactive_gsbase()();
        crate::serial_println!(
            "[switch] saved prev fs={:#x} gs={:#x} pid={}",
            prev_task.fs_base.as_u64(),
            prev_task.gs_base.as_u64(),
            prev_task.pid,
        );

        crate::serial_println!(
            "[switch] load next fs={:#x} user_gs={:#x} kernel_gs={:#x} pid={}",
            next_task.fs_base.as_u64(),
            next_task.gs_base.as_u64(),
            (&*next_task.kernel_gs as *const _ as u64),
            next_task.pid,
        );
        io::set_fsbase()(next_task.fs_base);
        io::wrmsr(
            io::IA32_GS_BASE,
            &*next_task.kernel_gs as *const _ as u64,
        );
        io::set_inactive_gsbase()(next_task.gs_base);
        crate::serial_println!("[switch] load next fs/gs done pid={}", next_task.pid);

        crate::serial_println!(
            "[switch] task_spinup prev_slot={:p} next_ctx={:p}",
            &mut prev_task.context,
            next_task.context,
        );
        task_spinup(&mut prev_task.context, next_task.context);
        crate::serial_println!("[switch] returned to pid={}", prev_task.pid);
    });
}

unsafe fn log_context(label: &str, context: *mut Context) {
    if context.is_null() {
        crate::serial_println!("[switch] {label} context=null");
        return;
    }

    let context = unsafe { &*context };
    crate::serial_println!(
        "[switch] {label} context cr3={:#x} rip={:#x} rbp={:#x} rbx={:#x} r12={:#x} r13={:#x} r14={:#x} r15={:#x}",
        context.cr3,
        context.rip,
        context.rbp,
        context.rbx,
        context.r12,
        context.r13,
        context.r14,
        context.r15,
    );
}
