use crate::driver::usb::uhci::UHci;
use crate::driver::usb::xhci::XhciDriver;
use crate::log;
use crate::sys::memory::phys_mem_offset;
use crate::sys::pci::PCI_DEVICES;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use spin::Mutex;

mod uhci;
pub mod usb_ids;
mod xhci;

lazy_static! {
    static ref UCHI_DEVICES: Mutex<Vec<UHci>> = Mutex::new(Vec::new());
    static ref XHCI_DEVICES: Mutex<Vec<XhciDriver>> = Mutex::new(Vec::new());
}

pub fn init() {
    // UHCI Initialization
    {
        let devices = PCI_DEVICES.lock();
        for dev in devices.iter() {
            if dev.class == 0x0C && dev.subclass == 0x03 && dev.prog == 0x00 {
                let io_base = dev
                    .base_addresses
                    .iter()
                    .copied()
                    .find(|bar| (bar & 0x1) == 0x1 && (bar & 0xFFFC) != 0)
                    .map(|bar| (bar & 0xFFFC) as u16)
                    .unwrap_or(0);

                if io_base == 0 {
                    log!(
                        "UHCI Controller found {:04x}:{:04x} but no IO BAR",
                        dev.vendor_id,
                        dev.device_id
                    );
                    continue;
                }

                dev.enable_bus_mastering();
                log!(
                    "UHCI Controller found: {:04x}:{:04x} IO={:#x}",
                    dev.vendor_id,
                    dev.device_id,
                    io_base
                );

                let mut uhci = UHci::new(io_base);
                uhci.list();
                UCHI_DEVICES.lock().push(uhci);
            }
        }
    }

    // XHCI Initialization
    let devices = PCI_DEVICES.lock();
    for dev in devices.iter() {
        if dev.class == 0x0C && dev.subclass == 0x03 && dev.prog == 0x30 {
            log!(
                "XHCI Controller found: {:04x}:{:04x}",
                dev.vendor_id,
                dev.device_id
            );

            // Get MMIO Base Address (BAR0)
            let base_addr = dev.mem_base().as_u64() as usize;
            log!("XHCI MMIO Base: {:#x}", base_addr);

            let mut xhci = XhciDriver::new(base_addr + phys_mem_offset() as usize);
            // We need to clone the device config because attach_device takes ownership of an Arc
            // But we are iterating a locked Vec. Ideally we'd clone the Arc.
            // For now, let's just initialize it. attach_device in this codebase seems to be a mix of logic.

            // Note: In a real implementation we would call xhci.init_device(), start_device(), etc.
            // For now, let's store it.
            // XHCI_DEVICES.lock().push(xhci);
        }
    }
}
