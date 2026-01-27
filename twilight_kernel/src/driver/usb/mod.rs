use crate::driver::usb::uhci::UHci;
use crate::driver::usb::xhci::XhciDriver;
use crate::log;
use crate::sys::pci::{find_device, PCI_DEVICES};
use crate::driver::pci_device_driver::PciDeviceDriver;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use spin::Mutex;

mod uhci;
mod xhci;

lazy_static! {
    static ref UCHI_DEVICES: Mutex<Vec<UHci>> = Mutex::new(Vec::new());
    static ref XHCI_DEVICES: Mutex<Vec<XhciDriver>> = Mutex::new(Vec::new());
}

pub fn init() {
    // UHCI Initialization (existing)
    if let Some(mut dev) = find_device(0x8086, 0x7020) {
        dev.enable_bus_mastering();
        let bar0 = dev.base_addresses[0];
        let _ = (bar0 & 0xFFFC) as u16;
        let mut io_base = 0;

        for addr in dev.base_addresses {
            if addr & 0xFFF0 != 0 {
                io_base = (addr as u16) & 0xFFF0;
            }
        }

        let mut uhci = UHci::new(io_base);
        uhci.list();
        {
            UCHI_DEVICES.lock().push(uhci);
        }

        log!("UHCI Dev IO Base: {:#x}", io_base);
    }

    // XHCI Initialization
    let devices = PCI_DEVICES.lock();
    for dev in devices.iter() {
        if dev.class == 0x0C && dev.subclass == 0x03 && dev.prog == 0x30 {
            log!("XHCI Controller found: {:04x}:{:04x}", dev.vendor_id, dev.device_id);
            
            // Get MMIO Base Address (BAR0)
            let base_addr = dev.mem_base().as_u64() as usize;
            log!("XHCI MMIO Base: {:#x}", base_addr);

            let mut xhci = XhciDriver::new(base_addr);
            // We need to clone the device config because attach_device takes ownership of an Arc
            // But we are iterating a locked Vec. Ideally we'd clone the Arc.
             // For now, let's just initialize it. attach_device in this codebase seems to be a mix of logic.
            
             // Note: In a real implementation we would call xhci.init_device(), start_device(), etc.
             // For now, let's store it.
             XHCI_DEVICES.lock().push(xhci);
        }
    }
}
