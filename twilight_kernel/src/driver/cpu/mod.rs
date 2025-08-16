use alloc::string::String;
use core::alloc::Layout;
use limine::response::MpResponse;
use raw_cpuid::CpuId;
use x86_64::VirtAddr;
use crate::{driver, extern_sym, println};
use crate::arch::x86_64::syscall::wrmsr;

unsafe extern "C" fn ap_main(_cpu: &limine::mp::Cpu) -> ! {
    use x86_64::instructions::{hlt, interrupts};

    interrupts::enable();

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
            println!("\x1b[93m[{:.6}]\x1b[0m BSP Core {}: APIC ID {}", time, i, apic_id, );
        } else {
            println!("\x1b[93m[{:.6}]\x1b[0m AP Core {}: APIC ID {}", time, i, apic_id);

            cpu.goto_address.write(ap_main);
        }
    }
}

const IA32_GS_BASE: u32 = 0xc0000101;
const IA32_KERNEL_GS_BASE: u32 = 0xc0000102;

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

    let start = VirtAddr::new(extern_sym!(__cpu_local_start).addr() as u64);
    let end = VirtAddr::new(extern_sym!(__cpu_local_end).addr() as u64);

    unsafe {
        let size = end - start;

        let layout = Layout::from_size_align_unchecked(size as _, 64);
        let data = alloc::alloc::alloc_zeroed(layout);

        core::ptr::copy_nonoverlapping(start.as_ptr(), data, size as usize);
        *data.cast::<u64>() = data as u64;

        wrmsr(IA32_GS_BASE, data as u64);
        wrmsr(IA32_KERNEL_GS_BASE, data as u64);
    }

    crate::print!(
        "\x1b[93m[{:.6}]\x1b[0m CPU [{:04x}:{:04x}] {}\n",
        time,
        vendor_id,
        device_id,
        name
    );
    init_smp(mp_response);
}
