use crate::arch::x86_64::gdt;
use crate::{print, println};
use alloc::string::String;
use core::arch::naked_asm;
use iced_x86::{Decoder, DecoderOptions, Formatter, IntelFormatter};
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use x86_64::VirtAddr;
pub use x86_64::structures::idt::{
    InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode,
};

#[repr(C, align(8))]
pub struct Registers {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r11: u64, // clobbered by SYSCALL
    pub r10: u64, // 4th arg (Linux ABI)
    pub r9: u64,  // 6th arg
    pub r8: u64,  // 5th arg
    pub rdi: u64, // 1st arg
    pub rsi: u64, // 2nd
    pub rdx: u64, // 3rd
    pub rcx: u64, // clobbered by SYSCALL (not an arg)
    pub rax: u64, // syscall nr on entry, return value on exit
}

// Translate IRQ into system interrupt
fn interrupt_index(irq: u8) -> u8 {
    PIC_1_OFFSET + irq
}

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.stack_segment_fault
            .set_handler_fn(stack_segment_fault_handler);
        idt.segment_not_present
            .set_handler_fn(segment_not_present_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        unsafe {
            idt.page_fault
                .set_handler_fn(page_fault_handler)
                .set_stack_index(gdt::PAGE_FAULT_IST);
            idt.general_protection_fault
                .set_handler_fn(general_protection_fault_handler)
                .set_stack_index(gdt::GENERAL_PROTECTION_FAULT_IST);
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        unsafe {
            idt[interrupt_index(0)].set_handler_addr(VirtAddr::new(timer_preempt_isr as u64));
            idt[0xFD].set_handler_addr(VirtAddr::new(apic_timer_preempt_isr as u64));
        }
        idt[interrupt_index(1)].set_handler_fn(keyboard_interrupt_handler);
        idt[interrupt_index(2)].set_handler_fn(irq_handler_2);
        idt[interrupt_index(3)].set_handler_fn(irq_handler_3);
        idt[interrupt_index(4)].set_handler_fn(irq_handler_4);
        idt[interrupt_index(5)].set_handler_fn(irq_handler_5);
        idt[interrupt_index(6)].set_handler_fn(irq_handler_6);
        idt[interrupt_index(7)].set_handler_fn(irq_handler_7);
        idt[interrupt_index(8)].set_handler_fn(irq_handler_8);
        idt[interrupt_index(9)].set_handler_fn(irq_handler_9);
        idt[interrupt_index(10)].set_handler_fn(irq_handler_10);
        idt[interrupt_index(11)].set_handler_fn(irq_handler_11);
        idt[interrupt_index(12)].set_handler_fn(mouse_interrupt_handler);
        idt[interrupt_index(13)].set_handler_fn(irq_handler_13);
        idt[interrupt_index(14)].set_handler_fn(ide_primary_interrupt_handler);
        idt[interrupt_index(15)].set_handler_fn(ide_secondary_interrupt_handler);
        idt
    };
}

pub fn init() {
    IDT.load();
}

#[inline]
fn from_user(sf: &InterruptStackFrame) -> bool {
    (sf.code_segment.0 & 0b11) == 3
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    if from_user(&stack_frame) {
        println!(
            "[PROC {}] Divide-by-zero at RIP={:#x}. Killing.",
            crate::sys::proc::id(),
            stack_frame.instruction_pointer.as_u64()
        );
        crate::sys::proc::exit(1);
        unreachable!()
    }
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, err: u64) -> ! {
    let _fault_guard = crate::sys::preempt::enter_fault_context();
    if from_user(&stack_frame) {
        println!(
            "[PROC {}] Segment Fault at RIP={:#x}. Killing.",
            crate::sys::proc::id(),
            stack_frame.instruction_pointer.as_u64()
        );
        crate::sys::proc::exit(1);
        unreachable!()
    }
    panic!(
        "EXCEPTION: DOUBLE FAULT\n{:#?}\nERROR CODE: {:#?}",
        stack_frame.instruction_pointer, err
    );
}

extern "x86-interrupt" fn invalid_opcode_handler(_sf: InterruptStackFrame) {
    println!("Invalid opcode!");
    println!("{:#?}", _sf);
    loop {}
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    if from_user(&stack_frame) {
        crate::serial_println!(
            "[PROC {}] User #GP at RIP={:#x}, error={:#x}. Killing.",
            crate::sys::proc::id(),
            stack_frame.instruction_pointer.as_u64(),
            error_code,
        );
        crate::sys::proc::terminate_current_by_signal(11);
    }

    let rip = stack_frame.instruction_pointer.as_u64();
    let rip_ptr = rip as *const u8;

    // read bytes
    let mut instr_bytes = [0u8; 16];
    unsafe {
        for i in 0..instr_bytes.len() {
            instr_bytes[i] = *rip_ptr.add(i);
        }
    }

    // decode
    let mut decoder = Decoder::with_ip(64, &instr_bytes, rip, DecoderOptions::NONE);
    let instruction = decoder.decode();
    let mut formatter = IntelFormatter::new();
    let mut output = String::new();
    formatter.format(&instruction, &mut output);

    // CR2 = faulting linear address
    let fault_addr = Cr2::read();

    println!("Decoded: {}", output);
    println!("CR2 (faulting linear/virtual): {:?}", fault_addr);
    println!("Error code: {:?}\n{:#?}", error_code, stack_frame);

    // Check whether instruction has a memory operand
    use iced_x86::OpKind;
    for i in 0..instruction.op_count() {
        match instruction.op_kind(i) {
            OpKind::Memory => {
                println!(
                    "Instruction has memory operand: base={:?} index={:?} scale={} disp={:#x}",
                    instruction.memory_base(),
                    instruction.memory_index(),
                    instruction.memory_index_scale(),
                    instruction.memory_displacement64()
                );
            }
            _ => {}
        }
    }

    let index = (error_code >> 3) & 0x1fff;
    let ti = (error_code >> 2) & 1;
    let rpl = error_code & 0b11;
    crate::serial_println!(
        "#GP err: selector=0x{:04x} index={} TI={}({}) RPL={}",
        error_code,
        index,
        ti,
        if ti == 0 { "GDT" } else { "LDT" },
        rpl
    );
    panic!(
        "[GP FAULT] at {:#x}, Error Code: {:#x}",
        stack_frame.instruction_pointer.as_u64(),
        error_code
    );
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    let _fault_guard = crate::sys::preempt::enter_fault_context();
    let fault_addr = Cr2::read_raw();
    let can_resolve = !error_code.intersects(
        PageFaultErrorCode::PROTECTION_VIOLATION
            | PageFaultErrorCode::MALFORMED_TABLE
            | PageFaultErrorCode::PROTECTION_KEY
            | PageFaultErrorCode::SHADOW_STACK
            | PageFaultErrorCode::SGX,
    );

    if can_resolve {
        use crate::sys::proc::PageFaultResolution;
        match crate::sys::proc::resolve_current_page_fault(
            fault_addr,
            error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE),
            error_code.contains(PageFaultErrorCode::INSTRUCTION_FETCH),
        ) {
            PageFaultResolution::Resolved => return,
            PageFaultResolution::BusError => {
                crate::sys::proc::terminate_current_by_signal(crate::sys::proc::SIGBUS as i32)
            }
            PageFaultResolution::Invalid => {}
        }
    }

    let user_address = fault_addr <= 0x0000_7fff_ffff_ffff;
    if from_user(&stack_frame) || (user_address && crate::sys::proc::id() != 0) {
        crate::serial_println!(
            "[PROC {}] Page fault at RIP={:#x}, address={:#x}, error={:?}",
            crate::sys::proc::id(),
            stack_frame.instruction_pointer.as_u64(),
            fault_addr,
            error_code,
        );

        // Print user stack trace
        let rsp = stack_frame.stack_pointer.as_u64();
        crate::serial_println!("User Stack Pointer (RSP): {:#x}", rsp);

        use x86_64::structures::paging::OffsetPageTable;
        use x86_64::structures::paging::Translate;
        let (level_4_page_table_frame, _) = x86_64::registers::control::Cr3::read();
        let phys_mem_offset = x86_64::VirtAddr::new(crate::sys::memory::phys_mem_offset());
        let mapper = unsafe {
            OffsetPageTable::new(
                &mut *(crate::sys::memory::phys_to_virt(level_4_page_table_frame.start_address())
                    .as_mut_ptr()),
                phys_mem_offset,
            )
        };

        for i in 0..16 {
            let addr = rsp.saturating_add(i * 8);
            if let Some(phys_addr) = mapper.translate_addr(x86_64::VirtAddr::new(addr)) {
                let kernel_virt_addr = crate::sys::memory::phys_to_virt(phys_addr);
                let val = unsafe { *(kernel_virt_addr.as_ptr::<u64>()) };
                crate::serial_println!("  [{:#x}]: {:#x}", addr, val);
            } else {
                crate::serial_println!("  [{:#x}]: (not mapped)", addr);
            }
        }

        crate::sys::proc::terminate_current_by_signal(crate::sys::proc::SIGSEGV as i32);
    }

    let rip = stack_frame.instruction_pointer.as_u64();
    let rip_ptr = rip as *const u8;

    // read bytes
    let mut instr_bytes = [0u8; 16];
    unsafe {
        for i in 0..instr_bytes.len() {
            instr_bytes[i] = *rip_ptr.add(i);
        }
    }

    // decode
    let mut decoder = Decoder::with_ip(64, &instr_bytes, rip, DecoderOptions::NONE);
    let instruction = decoder.decode();
    let mut formatter = IntelFormatter::new();
    let mut output = String::new();
    formatter.format(&instruction, &mut output);

    println!("\nPage fault @ RIP=0x{:x}", rip);
    print!("Instruction bytes: ");
    for b in &instr_bytes[..instruction.len()] {
        print!("{:02x} ", b);
    }
    print!("\n");
    println!("Decoded: {}", output);
    println!("CR2 (faulting linear/virtual): {:?}", fault_addr);
    println!("Error code: {:?}\n{:#?}", error_code, stack_frame);

    // Check whether instruction has a memory operand
    use iced_x86::OpKind;
    for i in 0..instruction.op_count() {
        match instruction.op_kind(i) {
            OpKind::Memory => {
                println!(
                    "Instruction has memory operand: base={:?} index={:?} scale={} disp={:#x}",
                    instruction.memory_base(),
                    instruction.memory_index(),
                    instruction.memory_index_scale(),
                    instruction.memory_displacement64()
                );
            }
            _ => {}
        }
    }

    panic!("page fault");
}
extern "x86-interrupt" fn stack_segment_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    println!("EXCEPTION: STACK SEGMENT FAULT");
    println!("Stack Frame: {:#?}", stack_frame);
    println!("Error: {:?}", error_code);
    panic!();
}

extern "x86-interrupt" fn segment_not_present_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    println!("EXCEPTION: SEGMENT NOT PRESENT");
    println!("Stack Frame: {:#?}", stack_frame);
    println!("Error: {:?}", error_code);
    panic!();
}

// device interrupt
use crate::driver::keyboard::keyboard_interrupt;
use crate::driver::mouse::ps2::handle_interrupt_byte;
use crate::utils::sync::Mutex;
use x86_64::registers::control::Cr2;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

static IRQ_HANDLERS: Mutex<[Option<fn()>; 16]> = Mutex::new([None; 16]);

pub fn register_irq_handler(irq: u8, handler: fn()) -> Result<(), ()> {
    if irq >= 16 {
        return Err(());
    }
    IRQ_HANDLERS.lock()[irq as usize] = Some(handler);

    // Unmask the IRQ in PIC
    unsafe {
        let mut pics = PICS.lock_irq();
        let halves = pics.read_masks();
        let mut masks = (halves[0] as u16) | ((halves[1] as u16) << 8);
        masks &= !(1 << irq);
        if irq >= 8 {
            // Ensure the PIC2 cascade line (IRQ2 on PIC1) is also unmasked.
            masks &= !(1 << 2);
        }
        pics.write_masks(masks as u8, (masks >> 8) as u8);
    }
    Ok(())
}

pub fn irq_vector(irq: u8) -> u8 {
    interrupt_index(irq)
}

fn dispatch_irq(irq: u8) {
    // Account every external IRQ on the depth stack so `irq_depth()` reflects
    // all in-progress interrupts, not only the timer (#70).
    let _ctx = crate::arch::x86_64::irq::IrqCtx::new();
    unsafe {
        PICS.lock_irq().notify_end_of_interrupt(interrupt_index(irq));
    }
    if irq < 16 {
        if let Some(h) = { IRQ_HANDLERS.lock()[irq as usize] } {
            h();
        }
    }
}

pub fn init_pics() {
    unsafe {
        PICS.lock_irq().initialize();
        // PIC1: unmask IRQ0..2 (timer, keyboard, cascade).
        // PIC2: unmask IRQ12 (mouse), IRQ14/15 (IDE).
        PICS.lock_irq().write_masks(0b11111000, 0b00101111);
    }
}

#[unsafe(naked)]
unsafe extern "C" fn timer_preempt_isr() -> ! {
    naked_asm!(
        // Stack at entry:
        //   RIP, CS, RFLAGS, (if CPL change) RSP, SS
        //
        // Save full GPR state so we can iretq into a (potentially different) process.
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rbp",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        // Determine if we came from ring3 by inspecting saved CS.
        // After pushing 15 regs, saved iret CS is at [rsp + 120 + 8].
        "xor rsi, rsi",
        "mov rax, [rsp + 128]",
        "and rax, 3",
        "cmp rax, 3",
        "sete sil",
        // If from user, swap to kernel GS base so kernel helpers work correctly.
        "test sil, sil",
        "jz 2f",
        "swapgs",
        "2:",
        // Push CR3 so the restore path can switch address spaces.
        "mov rax, cr3",
        "push rax",
        // Call Rust scheduler: rdi=frame_ptr, rsi=from_user (0/1)
        "mov rdi, rsp",
        "call {timer_preempt}",
        // Switch to returned frame (may be same task).
        "mov rsp, rax",
        // Restore CR3
        "pop rax",
        "mov cr3, rax",
        // If returning to ring3, swapgs back to user GS base.
        "mov rax, [rsp + 128]",
        "and rax, 3",
        "cmp rax, 3",
        "jne 3f",
        "swapgs",
        "3:",
        // Restore regs and return from interrupt.
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rbp",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rbx",
        "pop rax",
        "iretq",
        timer_preempt = sym crate::sys::proc::timer_preempt,
    );
}

#[unsafe(naked)]
unsafe extern "C" fn apic_timer_preempt_isr() -> ! {
    naked_asm!(
        // Stack at entry:
        //   RIP, CS, RFLAGS, (if CPL change) RSP, SS
        //
        // Save full GPR state so we can iretq into a (potentially different) process.
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rbp",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        // Determine if we came from ring3 by inspecting saved CS.
        // After pushing 15 regs, saved iret CS is at [rsp + 120 + 8].
        "xor rsi, rsi",
        "mov rax, [rsp + 128]",
        "and rax, 3",
        "cmp rax, 3",
        "sete sil",
        // If from user, swap to kernel GS base so kernel helpers work correctly.
        "test sil, sil",
        "jz 2f",
        "swapgs",
        "2:",
        // Push CR3 so the restore path can switch address spaces.
        "mov rax, cr3",
        "push rax",
        // Call Rust scheduler: rdi=frame_ptr, rsi=from_user (0/1)
        "mov rdi, rsp",
        "call {apic_timer_preempt}",
        // Switch to returned frame (may be same task).
        "mov rsp, rax",
        // Restore CR3
        "pop rax",
        "mov cr3, rax",
        // If returning to ring3, swapgs back to user GS base.
        "mov rax, [rsp + 128]",
        "and rax, 3",
        "cmp rax, 3",
        "jne 3f",
        "swapgs",
        "3:",
        // Restore regs and return from interrupt.
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rbp",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rbx",
        "pop rax",
        "iretq",
        apic_timer_preempt = sym crate::sys::proc::apic_timer_preempt,
    );
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let _ctx = crate::arch::x86_64::irq::IrqCtx::new();

    let mut port = Port::<u8>::new(0x60);

    let scancode: u8 = unsafe { port.read() };

    unsafe {
        PICS.lock_irq().notify_end_of_interrupt(interrupt_index(1));
    }

    keyboard_interrupt(scancode);
}

extern "x86-interrupt" fn mouse_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let _ctx = crate::arch::x86_64::irq::IrqCtx::new();

    let mut port = Port::<u8>::new(0x60);
    let data: u8 = unsafe { port.read() };

    handle_interrupt_byte(data);

    unsafe {
        PICS.lock_irq().notify_end_of_interrupt(interrupt_index(12));
    }
}

extern "x86-interrupt" fn ide_primary_interrupt_handler(_stack_frame: InterruptStackFrame) {
    dispatch_irq(14);
}

extern "x86-interrupt" fn ide_secondary_interrupt_handler(_stack_frame: InterruptStackFrame) {
    dispatch_irq(15);
}

extern "x86-interrupt" fn irq_handler_2(_stack_frame: InterruptStackFrame) {
    dispatch_irq(2);
}

extern "x86-interrupt" fn irq_handler_3(_stack_frame: InterruptStackFrame) {
    dispatch_irq(3);
}

extern "x86-interrupt" fn irq_handler_4(_stack_frame: InterruptStackFrame) {
    dispatch_irq(4);
}

extern "x86-interrupt" fn irq_handler_5(_stack_frame: InterruptStackFrame) {
    dispatch_irq(5);
}

extern "x86-interrupt" fn irq_handler_6(_stack_frame: InterruptStackFrame) {
    dispatch_irq(6);
}

extern "x86-interrupt" fn irq_handler_7(_stack_frame: InterruptStackFrame) {
    dispatch_irq(7);
}

extern "x86-interrupt" fn irq_handler_8(_stack_frame: InterruptStackFrame) {
    dispatch_irq(8);
}

extern "x86-interrupt" fn irq_handler_9(_stack_frame: InterruptStackFrame) {
    dispatch_irq(9);
}

extern "x86-interrupt" fn irq_handler_10(_stack_frame: InterruptStackFrame) {
    dispatch_irq(10);
}

extern "x86-interrupt" fn irq_handler_11(_stack_frame: InterruptStackFrame) {
    dispatch_irq(11);
}

extern "x86-interrupt" fn irq_handler_13(_stack_frame: InterruptStackFrame) {
    dispatch_irq(13);
}
