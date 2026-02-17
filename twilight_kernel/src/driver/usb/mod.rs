use crate::driver::pci_device_driver::PciDeviceDriver;
use crate::driver::usb::uhci::UHci;
use crate::driver::usb::xhci::XhciDriver;
use crate::log;
use crate::sys::pci::{PCI_DEVICES, lookup_device_name};
use alloc::vec::Vec;
use spin::Once;

pub mod hid;
pub mod interfaces;
pub mod keyboard;
pub mod manager;
pub mod msc;
mod uhci;
pub mod usb_ids;
mod xhci;

static mut UCHI_DEVICES: Once<Vec<UHci>> = Once::new();
static mut XHCI_DEVICES: Once<Vec<XhciDriver>> = Once::new();

pub fn init() {
    unsafe {
        #[allow(static_mut_refs)]
        if UCHI_DEVICES.get().is_none() {
            UCHI_DEVICES.call_once(|| Vec::new());
        }
        #[allow(static_mut_refs)]
        if XHCI_DEVICES.get().is_none() {
            XHCI_DEVICES.call_once(|| Vec::new());
        }
    }
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
                // One-shot recovery pass: a boot keyboard may complete its first IN TD
                // before IRQ registration. Re-arm once so runtime keypresses can continue.
                if uhci.handle_interrupt() {
                    uhci.poll_drivers();
                }
                #[allow(static_mut_refs)]
                unsafe { UCHI_DEVICES.get_mut_unchecked() }.push(uhci);

                let irq = dev.interrupt_line;
                log!("UHCI: Registering IRQ {} handler", irq);
                let _ = crate::arch::x86_64::idt::register_irq_handler(irq, usb_irq_handler);
            }
        }
    }

    // XHCI Initialization
    let devices = PCI_DEVICES.lock();
    for dev in devices.iter() {
        if dev.class == 0x0C && dev.subclass == 0x03 && dev.prog == 0x30 {
            log!(
                "XHCI Controller found: {:04x}:{:04x} ({}) at {:02x}:{:02x}.{:01x}",
                dev.vendor_id,
                dev.device_id,
                lookup_device_name(dev.vendor_id, dev.device_id),
                dev.bus,
                dev.device,
                dev.function
            );

            let bus_master_before = (dev.command & (1 << 2)) != 0;
            dev.enable_bus_mastering();

            // Get MMIO Base Address (BAR0)
            let base_addr = dev.mem_base().as_u64();
            log!(
                "XHCI MMIO Base: {:#x}; PCI bus master before init={}",
                base_addr,
                bus_master_before
            );

            #[allow(static_mut_refs)]
            let controller_id = unsafe { XHCI_DEVICES.get().unwrap_unchecked() }.len();
            let mut xhci = XhciDriver::new(base_addr);
            xhci.set_controller_id(controller_id);
            if !xhci.init_device() {
                log!("XHCI: Failed to initialize device");
                continue;
            }
            // We need to clone the device config because attach_device takes ownership of an Arc
            // But we are iterating a locked Vec. Ideally we'd clone the Arc.
            // For now, let's just initialize it. attach_device in this codebase seems to be a mix of logic.

            // Note: In a real implementation we would call xhci.init_device(), start_device(), etc.
            // For now, let's store it.
            #[allow(static_mut_refs)]
            unsafe { XHCI_DEVICES.get_mut_unchecked() }.push(xhci);

            // Register IRQ
            let irq = dev.interrupt_line;
            log!("XHCI: Registering IRQ {} handler", irq);
            let _ = crate::arch::x86_64::idt::register_irq_handler(irq, usb_irq_handler);
        }
    }
}

pub fn poll_all_drivers() {
    // unsafe {
    //     #[allow(static_mut_refs)]
    //     if let Some(uhci) = UCHI_DEVICES.get_mut() {
    //         for hc in uhci.iter_mut() {
    //             if hc.handle_interrupt() {
    //                 hc.poll_drivers();
    //             }
    //         }
    //     }
    //     #[allow(static_mut_refs)]
    //     if let Some(xhci) = XHCI_DEVICES.get_mut() {
    //         for hc in xhci.iter_mut() {
    //             if hc.handle_interrupt() {
    //                 hc.poll_drivers();
    //             }
    //         }
    //     }
    // }
}

pub fn usb_irq_handler() {
    unsafe {
        #[allow(static_mut_refs)]
        if let Some(uhci) = UCHI_DEVICES.get_mut() {
            for hc in uhci.iter_mut() {
                if hc.handle_interrupt() {
                    hc.poll_drivers();
                }
            }
        }
        // #[allow(static_mut_refs)]
        // if let Some(xhci) = XHCI_DEVICES.get_mut() {
        //     for hc in xhci.iter_mut() {
        //         if hc.handle_interrupt() {
        //             hc.poll_drivers();
        //         }
        //     }
        // }
    }
}
