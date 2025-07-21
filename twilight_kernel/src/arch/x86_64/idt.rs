use crate::arch::x86_64::gdt;
use crate::println;
use crate::sys::syscall::syscall_handler;
use core::arch::naked_asm;
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

#[repr(align(8), C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Registers {
    // Saved scratch registers
    pub r11: usize,
    pub r10: usize,
    pub r9: usize,
    pub r8: usize,
    pub rdi: usize,
    pub rsi: usize,
    pub rdx: usize,
    pub rcx: usize,
    pub rax: usize,
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

            let f = syscall_wrapper as *mut fn();
            idt[0x80]
                .set_handler_fn(core::mem::transmute(f))
                .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
        }
        idt[interrupt_index(0)].set_handler_fn(timer_interrupt_handler);
        idt[interrupt_index(1)].set_handler_fn(keyboard_interrupt_handler);
        idt
    };
}

pub fn init() {
    IDT.load();
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, err: u64) -> ! {
    panic!(
        "EXCEPTION: DOUBLE FAULT\n{:#?}\nERROR CODE: {:#?}",
        stack_frame.instruction_pointer, err
    );
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    panic!(
        "[GP FAULT] at {:#x}, Error Code: {:#x}",
        stack_frame.instruction_pointer.as_u64(),
        error_code
    );
}

extern "x86-interrupt" fn page_fault_handler(
    _stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    panic!("Error Code: {:?} \n{:#?}", error_code, _stack_frame);
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

// Naked function wrapper saving all scratch registers to the stack
// See: https://os.phil-opp.com/returning-from-exceptions/
macro_rules! wrap {
    ($fn: ident => $w:ident) => {
        #[unsafe(naked)]
        pub unsafe extern "sysv64" fn $w() {
            naked_asm!(
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
                "call {}",
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
                sym $fn
            );
        }
    };
}

wrap!(syscall_handler => syscall_wrapper);

// device interrupt
use crate::driver::keyboard::keyboard_interrupt;
use spin::Mutex;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

pub fn init_pics() {
    unsafe {
        PICS.lock().initialize();
        PICS.lock().write_masks(0b11111100, 0b11111111);
    }
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    crate::driver::timer::pit::tick();

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
