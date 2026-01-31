use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::mem::size_of;
use core::ptr::{read_volatile, write_volatile, NonNull};
use crate::driver::pci_device_driver::PciDeviceDriver;
use crate::sys::pci::DeviceConfig;
use crate::sys::memory::{map_mmio, phys_mem_offset, PAGE_SIZE};
use crate::sys::memory::phys::PhysBuf;
use crate::driver::usb::xhci::xhci_regs::{
    XhciCapabilityRegisters,
    XhciOperationalRegisters,
};

/* =========================================================
   xHCI Driver
========================================================= */

const XHCI_MMIO_MAP_BYTES: usize = 0x1000;
const XHCI_PORT_REG_BASE_OFFSET: usize = 0x400;
const XHCI_PORT_REG_STRIDE: usize = 0x10;

pub struct XhciDriver {
    /* MMIO base */
    xhc_base: usize,
    mmio_phys: u64,
    mmio_valid: bool,

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

    /* DMA-backed structures */
    dcbaa: Option<PhysBuf>,
    scratchpad_array: Option<PhysBuf>,
    scratchpad_buffers: Vec<PhysBuf>,

    /* Port status tracking */
    port_status_cache: Vec<u32>,
}

/* =========================================================
   PCI Device Driver Implementation
========================================================= */

unsafe impl Send for XhciDriver {}
unsafe impl Sync for XhciDriver {}

impl PciDeviceDriver for XhciDriver {
    fn init_device(&mut self) -> bool {
        unsafe {
            if !self.init_mmio() {
                return false;
            }
            self.parse_capability_registers();
        }
        self.port_status_cache = vec![0; self.max_ports as usize];
        self.init_dma_structs();
        self.enable_port_power();
        self.poll_ports();
        true
    }

    fn start_device(&mut self) -> bool {
        true
    }

    fn shutdown_device(&mut self) -> bool {
        true
    }

    fn attach_device(&mut self, dev: Arc<DeviceConfig>, enable_bus_mastering: bool) {
        crate::log!("XHCI: Attaching device to driver");
        if enable_bus_mastering {
            dev.enable_bus_mastering();
        }
        // In a real implementation we would keep the reference to dev
        // self.pci_dev = Some(dev);
    }
}

/* =========================================================
   xHCI Driver Implementation
========================================================= */

impl XhciDriver {
    pub fn new(mmio_phys: u64) -> Self {
        let mut mmio_valid = true;
        if mmio_phys == 0 {
            crate::log!("XHCI: MMIO base is 0, refusing to touch registers");
            mmio_valid = false;
        } else if map_mmio(mmio_phys, XHCI_MMIO_MAP_BYTES).is_err() {
            crate::log!("XHCI: Failed to map MMIO window");
            mmio_valid = false;
        }

        let xhc_base = if mmio_valid {
            (mmio_phys + phys_mem_offset()) as usize
        } else {
            0
        };

        let cap_regs = NonNull::new(xhc_base as *mut XhciCapabilityRegisters)
            .unwrap_or_else(NonNull::dangling);
        let op_regs = NonNull::dangling();

        Self {
            xhc_base,
            mmio_phys,
            mmio_valid,
            cap_regs,
            op_regs,

            capability_regs_length: 0,

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

            dcbaa: None,
            scratchpad_array: None,
            scratchpad_buffers: Vec::new(),

            port_status_cache: Vec::new(),
        }
    }

    /* -----------------------------------------------------
       Capability Parsing
    ----------------------------------------------------- */

    unsafe fn parse_capability_registers(&mut self) {
        let cap = self.cap_regs.as_ptr();

        /* HCSPARAMS1 */
        let hcs1 = unsafe { read_volatile(&(*cap).hcsparams1) };
        self.max_device_slots  = (hcs1 & 0xFF) as u8;
        self.max_interrupters  = ((hcs1 >> 8) & 0x7FF) as u8;
        self.max_ports         = ((hcs1 >> 24) & 0xFF) as u8;

        /* HCSPARAMS2 */
        let hcs2 = unsafe { read_volatile(&(*cap).hcsparams2) };
        self.isochronous_scheduling_threshold = (hcs2 & 0xF) as u8;
        self.erst_max = ((hcs2 >> 4) & 0xF) as u8;
        self.max_scratchpad_buffers = ((hcs2 >> 27) & 0x1F) as u8;

        /* HCCPARAMS1 */
        let hcc1 = unsafe { read_volatile(&(*cap).hccparams1) };
        self.addr_64bit_capable             = (hcc1 & (1 << 0)) != 0;
        self.bandwidth_negotiation_capable  = (hcc1 & (1 << 1)) != 0;
        self.context_64byte                 = (hcc1 & (1 << 2)) != 0;
        self.port_power_control             = (hcc1 & (1 << 3)) != 0;
        self.port_indicators                = (hcc1 & (1 << 4)) != 0;
        self.light_reset_capable             = (hcc1 & (1 << 5)) != 0;
        self.extended_capabilities_offset   = ((hcc1 >> 16) & 0xFFFF) << 2;
    }

    unsafe fn init_mmio(&mut self) -> bool {
        if !self.mmio_valid {
            return false;
        }

        let caplength = unsafe { read_volatile(&(*self.cap_regs.as_ptr()).caplength) };
        if caplength == 0 {
            crate::log!("XHCI: CAPLENGTH read as 0, aborting init");
            return false;
        }
        self.capability_regs_length = caplength;

        let op_ptr = (self.xhc_base + caplength as usize) as *mut XhciOperationalRegisters;
        self.op_regs = match NonNull::new(op_ptr) {
            Some(ptr) => ptr,
            None => {
                crate::log!("XHCI: Failed to build operational regs pointer");
                return false;
            }
        };
        true
    }

    fn init_dma_structs(&mut self) {
        let dcbaa_entries = (self.max_device_slots as usize).saturating_add(1).max(1);
        let mut dcbaa = PhysBuf::new(dcbaa_entries * size_of::<u64>());
        dcbaa.fill(0);

        let scratchpad_count = self.max_scratchpad_buffers as usize;
        let mut scratchpad_array: Option<PhysBuf> = None;
        let mut scratchpad_buffers: Vec<PhysBuf> = Vec::new();

        if scratchpad_count > 0 {
            let mut array = PhysBuf::new(scratchpad_count * size_of::<u64>());
            array.fill(0);

            for _ in 0..scratchpad_count {
                scratchpad_buffers.push(PhysBuf::new(PAGE_SIZE));
            }

            unsafe {
                let arr_ptr = array.virt_addr().as_mut_ptr::<u64>();
                let arr_slice = core::slice::from_raw_parts_mut(arr_ptr, scratchpad_count);
                for (idx, entry) in arr_slice.iter_mut().enumerate() {
                    *entry = scratchpad_buffers[idx].addr();
                }

                let dcbaa_ptr = dcbaa.virt_addr().as_mut_ptr::<u64>();
                let dcbaa_slice = core::slice::from_raw_parts_mut(dcbaa_ptr, dcbaa_entries);
                dcbaa_slice[0] = array.addr();
            }

            scratchpad_array = Some(array);
        }

        self.dcbaa = Some(dcbaa);
        self.scratchpad_array = scratchpad_array;
        self.scratchpad_buffers = scratchpad_buffers;

        if let Some(ref dcbaa) = self.dcbaa {
            unsafe {
                write_volatile(&mut (*self.op_regs.as_ptr()).dcbaap, dcbaa.addr());
                write_volatile(
                    &mut (*self.op_regs.as_ptr()).config,
                    self.max_device_slots as u32,
                );
            }
        }
    }

    pub fn poll_ports(&mut self) {
        if !self.mmio_valid || self.max_ports == 0 {
            return;
        }

        if self.port_status_cache.len() != self.max_ports as usize {
            self.port_status_cache = vec![0; self.max_ports as usize];
        }

        for port_index in 0..self.max_ports as usize {
            let portsc = unsafe { read_volatile(self.portsc_ptr(port_index)) };
            let prev = self.port_status_cache[port_index];

            let prev_ccs = prev & 0x1;
            let ccs = portsc & 0x1;

            if prev_ccs != ccs {
                if ccs != 0 {
                    let speed = Self::port_speed_name(portsc);
                    crate::log!(
                        "XHCI: USB device connected on port {} ({})",
                        port_index + 1,
                        speed
                    );
                } else {
                    crate::log!("XHCI: USB device disconnected on port {}", port_index + 1);
                }
            }

            self.port_status_cache[port_index] = portsc;
        }
    }

    fn enable_port_power(&mut self) {
        if !self.mmio_valid || !self.port_power_control || self.max_ports == 0 {
            return;
        }

        for port_index in 0..self.max_ports as usize {
            let ptr = self.portsc_ptr(port_index);
            let mut portsc = unsafe { read_volatile(ptr) };
            portsc |= 1 << 9; // PP: Port Power
            unsafe { write_volatile(ptr, portsc) };
        }
    }

    fn portsc_ptr(&self, port_index: usize) -> *mut u32 {
        let base = self.op_regs.as_ptr() as usize;
        let offset = XHCI_PORT_REG_BASE_OFFSET + (port_index * XHCI_PORT_REG_STRIDE);
        (base + offset) as *mut u32
    }

    fn port_speed_name(portsc: u32) -> &'static str {
        match (portsc >> 10) & 0xF {
            1 => "full-speed",
            2 => "low-speed",
            3 => "high-speed",
            4 => "super-speed",
            5 => "super-speed-plus",
            _ => "unknown-speed",
        }
    }

    fn log_capability_registers(&self) {
        /* Kernel log implementation goes here */
    }
}
