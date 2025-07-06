use x86_64::structures::idt::InterruptStackFrame;
use crate::arch::x86_64::idt::Registers;
use crate::println;

pub extern "sysv64" fn syscall_handler(
    _stack_frame: &mut InterruptStackFrame,
    regs: &mut Registers
) {
    println!("SYSCALL: {}", regs.rax);
}
