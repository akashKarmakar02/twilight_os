use lazy_static::lazy_static;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use crate::{println, print};
use crate::arch::x86_64::gdt;
use pic8259::ChainedPics;


// Translate IRQ into system interrupt
fn interrupt_index(irq: u8) -> u8 {
    PIC_1_OFFSET + irq
}

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.general_protection_fault.set_handler_fn(general_protection_fault_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
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
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}\nERROR CODE: {:#?}", stack_frame.instruction_pointer.as_u64(), err);
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

extern "x86-interrupt" fn page_fault_handler(_stack_frame: InterruptStackFrame, error_code: PageFaultErrorCode) {
    println!("EXCEPTION: PAGE FAULT");
    println!("Accessed Address: {:?}", Cr2::read());
    println!("Error Code: {:?}", error_code);
    println!("{:#?}", _stack_frame);
}

use spin::Mutex;
use x86_64::registers::control::Cr2;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe {
    ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET)
});

pub fn init_pics() {
    unsafe {
        PICS.lock().initialize();
        PICS.lock().write_masks(0b11111100, 0b11111111);
    }
}


extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    unsafe {
        PICS.lock().notify_end_of_interrupt(interrupt_index(0));
    }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let mut port = Port::<u8>::new(0x60);

    let scancode: u8 = unsafe { port.read() };

    crate::driver::keyboard::add_scancode(scancode);

    unsafe {
        PICS.lock().notify_end_of_interrupt(interrupt_index(1));
    }
}