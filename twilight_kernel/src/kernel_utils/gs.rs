use crate::arch::x86_64::io::{IA32_FS_BASE, IA32_SYSENTER_ESP};
use crate::arch::x86_64::syscall::rdmsr;
use crate::driver::cpu::{IA32_GS_BASE, IA32_KERNEL_GS_BASE};
use crate::println;

pub fn main() {
    let data = rdmsr(IA32_GS_BASE);
    println!("GS: {:#x}", data);
    let k_data = rdmsr(IA32_KERNEL_GS_BASE);
    println!("Kernel GS: {:#x}", k_data);

    let rsp = rdmsr(IA32_SYSENTER_ESP);

    println!("RSP: {:#x}", rsp);

    let fsbase = rdmsr(IA32_FS_BASE);
    println!("FSBASE: {:#x}", fsbase);
}
