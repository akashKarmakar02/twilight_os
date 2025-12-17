#![no_std]

use alloc::sync::Arc;
use core::ptr::NonNull;

use crate::driver::pci_device_driver::PciDeviceDriver;
use crate::driver::usb::xhci::xhci_regs::{
    XhciCapabilityRegisters,
    XhciOperationalRegisters,
};

/* =========================================================
   xHCI Driver
========================================================= */

pub struct XhciDriver {
    /* MMIO base */
    xhc_base: usize,

    /* Register blocks */
    cap_regs: NonNull<XhciCapabilityRegisters>,
    op_regs:  NonNull<XhciOperationalRegisters>,

    /* CAPLENGTH */
    capability_regs_length: u8,

    /* HCSPARAMS1 */
    max_device_slots: u8,
    max_interrupters: u8,
    max_ports: u8,

    /* HCSPARAMS2 */
    isochronous_scheduling_threshold: u8,
    erst_max: u8,
    max_scratchpad_buffers: u8,

    /* HCCPARAMS1 */
    addr_64bit_capable: bool,
    bandwidth_negotiation_capable: bool,
    context_64byte: bool,
    port_power_control: bool,
    port_indicators: bool,
    light_reset_capable: bool,
    extended_capabilities_offset: u32,
}

/* =========================================================
   PCI Device Driver Implementation
========================================================= */

impl PciDeviceDriver for XhciDriver {
    fn init_device(&mut self) -> bool {
        unsafe {
            self.parse_capability_registers();
        }
        true
    }

    fn start_device(&mut self) -> bool {
        true
    }

    fn shutdown_device(&mut self) -> bool {
        true
    }

    fn attach_device(&mut self, dev: Arc<PciDevice>, enable_bus_mastering: bool) {
        todo!()
    }
}

/* =========================================================
   xHCI Driver Implementation
========================================================= */

impl XhciDriver {
    pub fn new(xhc_base: usize) -> Self {
        let cap_regs = unsafe {
            NonNull::new_unchecked(xhc_base as *mut XhciCapabilityRegisters)
        };

        let caplength = unsafe { cap_regs.as_ref().caplength };

        let op_regs = unsafe {
            NonNull::new_unchecked(
                (xhc_base + caplength as usize) as *mut XhciOperationalRegisters
            )
        };

        Self {
            xhc_base,
            cap_regs,
            op_regs,

            capability_regs_length: caplength,

            max_device_slots: 0,
            max_interrupters: 0,
            max_ports: 0,

            isochronous_scheduling_threshold: 0,
            erst_max: 0,
            max_scratchpad_buffers: 0,

            addr_64bit_capable: false,
            bandwidth_negotiation_capable: false,
            context_64byte: false,
            port_power_control: false,
            port_indicators: false,
            light_reset_capable: false,
            extended_capabilities_offset: 0,
        }
    }

    /* -----------------------------------------------------
       Capability Parsing
    ----------------------------------------------------- */

    unsafe fn parse_capability_registers(&mut self) {
        let cap = self.cap_regs.as_ref();

        /* HCSPARAMS1 */
        let hcs1 = cap.hcsparams1;
        self.max_device_slots  = (hcs1 & 0xFF) as u8;
        self.max_interrupters  = ((hcs1 >> 8) & 0x7FF) as u8;
        self.max_ports         = ((hcs1 >> 24) & 0xFF) as u8;

        /* HCSPARAMS2 */
        let hcs2 = cap.hcsparams2;
        self.isochronous_scheduling_threshold = (hcs2 & 0xF) as u8;
        self.erst_max = ((hcs2 >> 4) & 0xF) as u8;
        self.max_scratchpad_buffers = ((hcs2 >> 27) & 0x1F) as u8;

        /* HCCPARAMS1 */
        let hcc1 = cap.hccparams1;
        self.addr_64bit_capable             = (hcc1 & (1 << 0)) != 0;
        self.bandwidth_negotiation_capable  = (hcc1 & (1 << 1)) != 0;
        self.context_64byte                 = (hcc1 & (1 << 2)) != 0;
        self.port_power_control             = (hcc1 & (1 << 3)) != 0;
        self.port_indicators                = (hcc1 & (1 << 4)) != 0;
        self.light_reset_capable             = (hcc1 & (1 << 5)) != 0;
        self.extended_capabilities_offset   = ((hcc1 >> 16) & 0xFFFF) << 2;
    }

    fn log_capability_registers(&self) {
        /* Kernel log implementation goes here */
    }
}
