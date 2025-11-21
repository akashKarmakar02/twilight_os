use alloc::boxed::Box;
use crate::arch::x86_64::io;
use crate::sys::memory::bitmap::with_frame_allocator;
use crate::sys::memory::phys_to_virt;
use crate::sys::proc::Process;
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

#[repr(C)]
struct KernelGsData {
    kernel_rsp: u64, // offset 0
    user_rsp: u64,   // offset 8
    // ... any other fields
}


pub fn switch_tasks(prev_task: &mut Process, next_task: &mut Process) {
    unsafe {
        if let Some(fpu) = prev_task.fpu_storage.as_mut() {
            xsave(fpu);
        }

        if let Some(fpu) = next_task.fpu_storage.as_mut() {
            xrstor(fpu);
        }

        let kstack_top = next_task.context_switch_rsp.as_u64();
        crate::arch::x86_64::gdt::TSS.rsp[0] = kstack_top;
        io::wrmsr(io::IA32_SYSENTER_ESP, kstack_top);

        let mut kgs = Box::new(KernelGsData { kernel_rsp: 0, user_rsp: 0 });

        kgs.kernel_rsp = kstack_top;

        let kgs_va = VirtAddr::new(&*kgs as *const _ as u64);
        next_task.gs_base = kgs_va;

        prev_task.fs_base = io::get_fsbase()();
        prev_task.gs_base = io::get_inactive_gsbase()();
        
        io::set_fsbase()(next_task.fs_base);
        io::set_inactive_gsbase()(next_task.gs_base);
        
        task_spinup(&mut prev_task.context, next_task.context);
    }
}