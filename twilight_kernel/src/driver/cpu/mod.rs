use alloc::string::String;
use raw_cpuid::CpuId;

pub fn init() {
    let cpuid = CpuId::new();
    let time = crate::driver::timer::pit::uptime();

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

    crate::print!(
        "\x1b[93m[{:.6}]\x1b[0m CPU [{:04x}:{:04x}] {}\n",
        time,
        vendor_id,
        device_id,
        name
    );
}
