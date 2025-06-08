use x86_64::instructions::port::Port;
use crate::{print};

const CONFIG_ADDR: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

pub fn init() {
    for i in 0..256 {
        check_bus(i as u8);
    }
}

fn check_bus(bus: u8) {
    for device in 0..32 {
        check_device(bus, device);
    }
}

fn check_device(bus: u8, device: u8) {
    let function = 0;

    let vendor_id = vendor_id(bus, device, function);
    if vendor_id == 0xFFFF {
    }
}

pub fn lookup_device_name(vendor: u16, device: u16) -> &'static str {
    match (vendor, device) {
        // Intel
        (0x8086, 0x1237) => "Intel 82441FX PMC (Host Bridge)",         // Common in QEMU
        (0x8086, 0x7000) => "Intel 82371SB PIIX3 ISA (Southbridge)",
        (0x8086, 0x100e) => "Intel PRO/1000 MT Desktop Adapter",
        (0x8086, 0x10d3) => "Intel 82574L Gigabit Network Connection",
        (0x8086, 0x2922) => "Intel ICH9 SATA Controller [AHCI mode]",
        (0x8086, 0x1e31) => "Intel USB xHCI Host Controller",
        (0x8086, 0x2415) => "Intel AC'97 Audio Controller",

        // Realtek
        (0x10ec, 0x8139) => "Realtek RTL-8139",
        (0x10ec, 0x8168) => "Realtek RTL8111/8168/8411 PCI Express Gigabit Ethernet Controller",

        // NVIDIA
        (0x10de, 0x1cb3) => "NVIDIA GeForce GTX 1050",
        (0x10de, 0x1b80) => "NVIDIA GeForce GTX 1080",
        (0x10de, 0x1f07) => "NVIDIA TU104 [GeForce RTX 2080 SUPER]",

        // AMD/ATI
        (0x1002, 0x67df) => "AMD Radeon RX 580",
        (0x1002, 0x7340) => "AMD Radeon RX 5700 XT",
        (0x1002, 0x15d8) => "AMD USB 3.0 Host Controller",

        // Broadcom
        (0x14e4, 0x43a0) => "Broadcom BCM4360 802.11ac Wireless Network Adapter",

        // VMware (common in virtual machines)
        (0x15ad, 0x0740) => "VMware SVGA II Adapter",
        (0x15ad, 0x0790) => "VMware VMXNET3 Ethernet Controller",

        // QEMU (emulated devices)
        (0x1234, 0x1111) => "QEMU Virtual Graphics Adapter",

        // Unknown
        _ => "Unknown device",
    }
}

fn vendor_id(bus: u8, device: u8, function: u8) -> u16 {
    let vendor_id = read_config(bus, device, function, 0);

    if vendor_id != 0xFFFF {
        let device_id = read_config(bus, device, function, 2);
        let name = lookup_device_name(vendor_id, device_id);
        print!("[{:.6}] PCI {:04}:{:02}:{:02} [{:04x}:{:04x}] {}\n", crate::driver::timer::pit::uptime(), bus, device, function, vendor_id, device_id, name);
    }

    vendor_id
}

fn read_config(bus: u8, device: u8, function: u8, offset: u8) -> u16 {
    let addr = ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xfc)  // align to 4 bytes
        | 0x80000000;

    let mut addr_port = Port::new(CONFIG_ADDR);
    let mut data_port = Port::new(CONFIG_DATA);

    let data: u32 = unsafe {
        addr_port.write(addr);
        data_port.read()
    };

    let shift = ((offset & 2) * 8) as u32;
    ((data >> shift) & 0xffff) as u16
}
