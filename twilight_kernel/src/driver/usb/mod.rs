use crate::driver::usb::uhci::UHci;
use crate::log;
use crate::sys::pci::find_device;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use spin::Mutex;

mod uhci;

lazy_static! {
    static ref UCHI_DEVICES: Mutex<Vec<UHci>> = Mutex::new(Vec::new());
}

pub fn init() {
    if let Some(mut dev) = find_device(0x8086, 0x7020) {
        dev.enable_bus_mastering();
        let bar0 = dev.base_addresses[0];
        let _ = (bar0 & 0xFFFC) as u16;
        let mut io_base = 0;

        for addr in dev.base_addresses {
            if addr & &0xFFF0 != 0 {
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
}
