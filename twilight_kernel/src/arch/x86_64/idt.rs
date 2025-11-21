use crate::arch::x86_64::gdt;
use crate::{print, println};
use alloc::string::String;
use iced_x86::{Decoder, DecoderOptions, Formatter, IntelFormatter};
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

#[repr(C, align(8))]
pub struct Registers {
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
        idt[interrupt_index(0)].set_handler_fn(timer_interrupt_handler);
        idt[interrupt_index(1)].set_handler_fn(keyboard_interrupt_handler);
        idt[interrupt_index(12)].set_handler_fn(mouse_interrupt_handler);
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
        crate::sys::proc::exit();
        unreachable!()
    }
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, err: u64) -> ! {
    if from_user(&stack_frame) {
        println!(
            "[PROC {}] Segment Fault at RIP={:#x}. Killing.",
            crate::sys::proc::id(),
            stack_frame.instruction_pointer.as_u64()
        );
        crate::sys::proc::exit();
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

    if from_user(&stack_frame) {
        println!(
            "[PROC {}] Page Fault at RIP={:#x}. Killing.",
            crate::sys::proc::id(),
            stack_frame.instruction_pointer.as_u64()
        );
        crate::sys::proc::exit();
        unreachable!()
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
use spin::Mutex;
use x86_64::registers::control::Cr2;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

pub fn init_pics() {
    unsafe {
        PICS.lock().initialize();
        PICS.lock().write_masks(0b11111000, 0b11101111);
    }
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    crate::driver::timer::pit::pit_tick_isr();

    unsafe {
        PICS.lock().notify_end_of_interrupt(interrupt_index(0));
    }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let mut port = Port::<u8>::new(0x60);

    let scancode: u8 = unsafe { port.read() };

    keyboard_interrupt(scancode);

    unsafe {
        PICS.lock().notify_end_of_interrupt(interrupt_index(1));
    }
}

extern "x86-interrupt" fn mouse_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let mut port = Port::<u8>::new(0x60);
    let data: u8 = unsafe { port.read() };

    handle_interrupt_byte(data);

    unsafe {
        PICS.lock().notify_end_of_interrupt(interrupt_index(12));
    }
}
