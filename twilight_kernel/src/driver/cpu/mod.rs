use crate::arch::x86_64::syscall::{IA32_EFER, rdmsr, wrmsr};
use crate::{driver, println};
use alloc::string::String;
use limine::response::MpResponse;
use raw_cpuid::CpuId;
use x86_64::registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags};
use x86_64::registers::xcontrol::{XCr0, XCr0Flags};

unsafe extern "C" fn ap_main(cpu: &limine::mp::Cpu) -> ! {
    use x86_64::instructions::hlt;

    crate::arch::x86_64::cpu_local::init(cpu.id as usize);

    x86_64::instructions::interrupts::enable();
    loop {
        hlt();
    }
}

pub fn init_smp(mp_response: &'static MpResponse) {
    let smp = mp_response;
    let bsp_id = mp_response.bsp_lapic_id();

    let time = driver::timer::pit::uptime();

    for i in 0..smp.cpus().len() {
        let cpu = smp.cpus().get(i).unwrap();
        let apic_id = cpu.lapic_id;

        if apic_id == bsp_id {
            println!(
                "\x1b[93m[{:.6}]\x1b[0m BSP Core {}: APIC ID {}",
                time, i, apic_id,
            );
        } else {
            println!(
                "\x1b[93m[{:.6}]\x1b[0m AP Core {}: APIC ID {}",
                time, i, apic_id
            );

            cpu.goto_address.write(ap_main);
        }
    }
}

pub const IA32_GS_BASE: u32 = 0xc0000101;
pub const IA32_KERNEL_GS_BASE: u32 = 0xc0000102;

pub fn init(mp_response: &'static MpResponse) {
    let cpuid = CpuId::new();
    let time = driver::timer::pit::uptime();

    let name = if let Some(cpu) = cpuid.get_processor_brand_string() {
        String::from(cpu.as_str())
    } else {
        String::from("Unknown CPU")
    };

    let vendor_id = cpuid
        .get_vendor_info()
        .map(|v| {
            let s = v.as_str().as_bytes();
            s.iter().fold(0u16, |acc, &b| acc.wrapping_add(b as u16))
        })
        .unwrap_or(0xffff);

    let device_id = cpuid
        .get_feature_info()
        .map(|f| ((f.family_id() as u16) << 8) | (f.model_id() as u16))
        .unwrap_or(0);

    crate::arch::x86_64::cpu_local::init(0);

    crate::print!(
        "\x1b[93m[{:.6}]\x1b[0m CPU [{:04x}:{:04x}] {}\n",
        time,
        vendor_id,
        device_id,
        name
    );
    init_smp(mp_response);
}

pub fn has_fsgsbase() -> bool {
    CpuId::new()
        .get_extended_feature_info()
        .unwrap()
        .has_fsgsbase()
}

pub fn init_cpu_x86_64() {
    wrmsr(IA32_EFER, rdmsr(IA32_EFER) | (1 << 11));

    let mut cr0 = Cr0::read();

    let extensions = CpuId::new().get_extended_feature_info().unwrap();
    let features = CpuId::new().get_feature_info().unwrap();

    cr0.remove(Cr0Flags::EMULATE_COPROCESSOR);
    cr0.insert(Cr0Flags::MONITOR_COPROCESSOR);

    unsafe {
        Cr0::write(cr0);
    }

    let mut cr4 = Cr4::read();

    cr4.insert(Cr4Flags::OSFXSR);
    cr4.insert(Cr4Flags::OSXMMEXCPT_ENABLE);

    if extensions.has_fsgsbase() {
        cr4.insert(Cr4Flags::FSGSBASE);
    }

    unsafe {
        Cr4::write(cr4);
    }

    if features.has_xsave() {
        enable_xsave();
    }
}

fn enable_xsave() {
    // Enable XSAVE and x{get,set}bv
    let mut cr4 = Cr4::read();
    cr4.insert(Cr4Flags::OSXSAVE);
    unsafe { Cr4::write(cr4) }

    let mut xcr0 = XCr0::read();
    xcr0.insert(XCr0Flags::X87 | XCr0Flags::SSE | XCr0Flags::AVX);
    // xcr0.insert(XCr0Flags::BNDREG | XCr0Flags::BNDCSR);
    // xcr0.insert(XCr0Flags::ZMM_HI256 | XCr0Flags::HI16_ZMM | XCr0Flags::OPMASK);
    unsafe { XCr0::write(xcr0) }
}
