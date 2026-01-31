use crate::driver::pci_device_driver::PciDeviceDriver;
use crate::driver::timer::wait;
use crate::driver::usb::interfaces::{
    HostController, InterruptTransfer, UsbDevice, UsbDeviceKind, UsbDriver, UsbError,
};
use crate::driver::usb::usb_ids;
use crate::driver::usb::xhci::xhci_regs::{
    XhciCapabilityRegisters, XhciDoorbellRegisters, XhciOperationalRegisters, XhciRuntimeRegisters,
};
use crate::log;
use crate::sys::memory::phys::PhysBuf;
use crate::sys::memory::{PAGE_SIZE, map_mmio, phys_mem_offset};
use crate::sys::pci::DeviceConfig;
use alloc::boxed::Box;
use alloc::format;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::mem::size_of;
use core::ptr::{NonNull, read_volatile, write_volatile};

/* =========================================================
   xHCI Driver
========================================================= */

// Needs to cover op regs, port regs, runtime regs, and doorbells (QEMU commonly places these well past 4KiB).
const XHCI_MMIO_MAP_BYTES: usize = 0x10000;
const XHCI_PORT_REG_BASE_OFFSET: usize = 0x400;
const XHCI_PORT_REG_STRIDE: usize = 0x10;

const USBCMD_RUN_STOP: u32 = 1 << 0;
const USBCMD_HCRST: u32 = 1 << 1;
const USBCMD_INTE: u32 = 1 << 2;

const USBSTS_HCH: u32 = 1 << 0;
const USBSTS_CNR: u32 = 1 << 11;

const PORTSC_CCS: u32 = 1 << 0;
const PORTSC_PED: u32 = 1 << 1;
const PORTSC_PR: u32 = 1 << 4;
const PORTSC_PP: u32 = 1 << 9;
const PORTSC_SPEED_SHIFT: u32 = 10;
const PORTSC_SPEED_MASK: u32 = 0xF << PORTSC_SPEED_SHIFT;

// Write-1-to-clear bits (subset)
const PORTSC_W1C_MASK: u32 =
    (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23);

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Trb {
    d0: u32,
    d1: u32,
    d2: u32,
    d3: u32,
}

const TRB_TYPE_SHIFT: u32 = 10;
const TRB_TYPE_MASK: u32 = 0x3F << TRB_TYPE_SHIFT;

const TRB_TYPE_LINK: u32 = 6;
const TRB_TYPE_NORMAL: u32 = 1;
const TRB_TYPE_SETUP_STAGE: u32 = 2;
const TRB_TYPE_DATA_STAGE: u32 = 3;
const TRB_TYPE_STATUS_STAGE: u32 = 4;
const TRB_TYPE_ENABLE_SLOT_CMD: u32 = 9;
const TRB_TYPE_ADDRESS_DEVICE_CMD: u32 = 11;
const TRB_TYPE_CONFIGURE_ENDPOINT_CMD: u32 = 12;
const TRB_TYPE_TRANSFER_EVENT: u32 = 32;
const TRB_TYPE_CMD_COMPLETION_EVENT: u32 = 33;

const TRB_CYCLE: u32 = 1 << 0;
const TRB_TC: u32 = 1 << 1; // Toggle Cycle (Link TRB)
const TRB_IOC: u32 = 1 << 5;
const TRB_IDT: u32 = 1 << 6; // Immediate Data (Setup Stage)
const TRB_DIR_IN: u32 = 1 << 16; // Data/Status Stage

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct ErstEntry {
    base: u64,
    size: u32,
    rsvd: u32,
}

use core::sync::atomic::{AtomicU8, Ordering};

struct XhciSlotState {
    port_index: usize,
    speed_psiv: u8,
    input_ctx: PhysBuf,
    dev_ctx: PhysBuf,
    ep0_ring: PhysBuf,
    ep0_cycle: bool,
    ep0_index: usize,
    ep_transfers: Vec<Option<Arc<AtomicU8>>>,
}

// ... (existing code)

fn sleep_us(us: u64) {
    wait(us * 1_000);
}

fn sleep_ms(ms: u64) {
    wait(ms * 1_000_000);
}

fn endpoint_id(ep_num: u8, dir_in: bool) -> u8 {
    if ep_num == 0 {
        1
    } else {
        (ep_num * 2) + if dir_in { 1 } else { 0 }
    }
}

fn parse_boot_hid_int_in_endpoint(cfg: &[u8], want_protocol: u8) -> Option<(u8, u8, u8, u8, u8)> {
    // Returns (config_value, interface, ep_num, max_packet, interval)
    if cfg.len() < 9 {
        return None;
    }
    let config_value = cfg[5];

    let mut off = 0usize;
    let mut current_if: Option<(u8, u8, u8, u8)> = None; // (ifnum, class, subclass, protocol)

    while off + 2 <= cfg.len() {
        let len = cfg[off] as usize;
        let dtype = cfg[off + 1];
        if len < 2 || off + len > cfg.len() {
            break;
        }

        if dtype == 0x04 && len >= 9 {
            let ifnum = cfg[off + 2];
            let class = cfg[off + 5];
            let subclass = cfg[off + 6];
            let protocol = cfg[off + 7];
            current_if = Some((ifnum, class, subclass, protocol));
        } else if dtype == 0x05 && len >= 7 {
            if let Some((ifnum, class, subclass, protocol)) = current_if {
                if class == 0x03 && subclass == 0x01 && protocol == want_protocol {
                    let ep_addr = cfg[off + 2];
                    let attrs = cfg[off + 3] & 0x03;
                    let max_packet = u16::from_le_bytes([cfg[off + 4], cfg[off + 5]]) as usize;
                    let interval = cfg[off + 6];

                    let is_in = (ep_addr & 0x80) != 0;
                    let ep_num = ep_addr & 0x0F;
                    let is_interrupt = attrs == 0x03;

                    if is_in && is_interrupt && ep_num != 0 && max_packet > 0 {
                        return Some((
                            config_value,
                            ifnum,
                            ep_num,
                            core::cmp::min(255, max_packet) as u8,
                            interval,
                        ));
                    }
                }
            }
        }

        off += len;
    }

    None
}

#[allow(dead_code)]
pub struct XhciDriver {
    /* MMIO base */
    xhc_base: usize,
    mmio_phys: u64,
    mmio_valid: bool,

    /* Register blocks */
    cap_regs: NonNull<XhciCapabilityRegisters>,
    op_regs: NonNull<XhciOperationalRegisters>,
    rt_regs: NonNull<XhciRuntimeRegisters>,
    db_regs: NonNull<XhciDoorbellRegisters>,

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

    /* Rings */
    cmd_ring: Option<PhysBuf>,
    cmd_cycle: bool,
    cmd_index: usize,

    event_ring: Option<PhysBuf>,
    event_cycle: bool,
    event_index: usize,
    erst: Option<PhysBuf>,

    /* Command completion (polled) */
    last_cmd_seen: bool,
    last_cmd_cc: u8,
    last_cmd_slot: u8,

    /* Transfer completion (polled, EP0 only during init) */
    last_xfer_seen: bool,
    last_xfer_cc: u8,
    last_xfer_slot: u8,
    last_xfer_epid: u8,

    /* Slots (best-effort; USB2 only for now) */
    slots: Vec<Option<XhciSlotState>>,

    /* Generic Driver Support */
    drivers: Vec<Box<dyn UsbDriver>>,

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
        if !self.reset_and_start() {
            crate::log!("XHCI: controller reset/start failed");
            return false;
        }
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
        let rt_regs = NonNull::dangling();
        let db_regs = NonNull::dangling();

        Self {
            xhc_base,
            mmio_phys,
            mmio_valid,
            cap_regs,
            op_regs,
            rt_regs,
            db_regs,

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

            cmd_ring: None,
            cmd_cycle: true,
            cmd_index: 0,

            event_ring: None,
            event_cycle: true,
            event_index: 0,
            erst: None,

            last_cmd_seen: false,
            last_cmd_cc: 0,
            last_cmd_slot: 0,

            last_xfer_seen: false,
            last_xfer_cc: 0,
            last_xfer_slot: 0,
            last_xfer_epid: 0,

            slots: Vec::new(),
            drivers: Vec::new(),

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
        self.max_device_slots = (hcs1 & 0xFF) as u8;
        self.max_interrupters = ((hcs1 >> 8) & 0x7FF) as u8;
        self.max_ports = ((hcs1 >> 24) & 0xFF) as u8;

        /* HCSPARAMS2 */
        let hcs2 = unsafe { read_volatile(&(*cap).hcsparams2) };
        self.isochronous_scheduling_threshold = (hcs2 & 0xF) as u8;
        self.erst_max = ((hcs2 >> 4) & 0xF) as u8;
        self.max_scratchpad_buffers = ((hcs2 >> 27) & 0x1F) as u8;

        /* HCCPARAMS1 */
        let hcc1 = unsafe { read_volatile(&(*cap).hccparams1) };
        self.addr_64bit_capable = (hcc1 & (1 << 0)) != 0;
        self.bandwidth_negotiation_capable = (hcc1 & (1 << 1)) != 0;
        self.context_64byte = (hcc1 & (1 << 2)) != 0;
        self.port_power_control = (hcc1 & (1 << 3)) != 0;
        self.port_indicators = (hcc1 & (1 << 4)) != 0;
        self.light_reset_capable = (hcc1 & (1 << 5)) != 0;
        self.extended_capabilities_offset = ((hcc1 >> 16) & 0xFFFF) << 2;
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

        // Runtime + Doorbell base pointers
        let cap = self.cap_regs.as_ptr();
        let rtsoff = unsafe { read_volatile(&(*cap).rtsoff) } as usize;
        let dboff = unsafe { read_volatile(&(*cap).dboff) } as usize;

        let rt_ptr = (self.xhc_base + rtsoff) as *mut XhciRuntimeRegisters;
        self.rt_regs = NonNull::new(rt_ptr).unwrap_or_else(NonNull::dangling);

        let db_ptr = (self.xhc_base + dboff) as *mut XhciDoorbellRegisters;
        self.db_regs = NonNull::new(db_ptr).unwrap_or_else(NonNull::dangling);

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

        // Slot table (index 0 unused). Keep in sync with max_device_slots.
        if self.slots.len() != (self.max_device_slots as usize + 1) {
            let n = self.max_device_slots as usize + 1;
            self.slots = (0..n).map(|_| None).collect();
        }
    }

    fn reset_and_start(&mut self) -> bool {
        if !self.mmio_valid {
            return false;
        }

        unsafe {
            // Stop
            let mut cmd = read_volatile(&(*self.op_regs.as_ptr()).usbcmd);
            cmd &= !USBCMD_RUN_STOP;
            write_volatile(&mut (*self.op_regs.as_ptr()).usbcmd, cmd);
        }

        // Wait for halted
        for _ in 0..2000 {
            let st = unsafe { read_volatile(&(*self.op_regs.as_ptr()).usbsts) };
            if (st & USBSTS_HCH) != 0 {
                break;
            }
            sleep_us(50);
        }

        // HC reset
        unsafe {
            let mut cmd = read_volatile(&(*self.op_regs.as_ptr()).usbcmd);
            cmd |= USBCMD_HCRST;
            write_volatile(&mut (*self.op_regs.as_ptr()).usbcmd, cmd);
        }

        for _ in 0..20_000 {
            let cmd = unsafe { read_volatile(&(*self.op_regs.as_ptr()).usbcmd) };
            if (cmd & USBCMD_HCRST) == 0 {
                break;
            }
            sleep_us(50);
        }

        // Wait for controller not ready to clear
        for _ in 0..20_000 {
            let st = unsafe { read_volatile(&(*self.op_regs.as_ptr()).usbsts) };
            if (st & USBSTS_CNR) == 0 {
                break;
            }
            sleep_us(50);
        }

        if !self.init_rings() {
            return false;
        }

        unsafe {
            // Start + enable interrupts (we still poll event ring)
            let cmd = USBCMD_RUN_STOP | USBCMD_INTE;
            write_volatile(&mut (*self.op_regs.as_ptr()).usbcmd, cmd);
        }

        // Wait for running (HCH cleared)
        for _ in 0..2000 {
            let st = unsafe { read_volatile(&(*self.op_regs.as_ptr()).usbsts) };
            if (st & USBSTS_HCH) == 0 {
                return true;
            }
            sleep_us(50);
        }
        false
    }

    fn init_rings(&mut self) -> bool {
        // Command ring (TRB array + link TRB at end)
        let trb_count = 256usize;
        let mut cmd_ring = PhysBuf::new(trb_count * size_of::<Trb>());
        cmd_ring.fill(0);
        let trbs = unsafe {
            core::slice::from_raw_parts_mut(cmd_ring.virt_addr().as_mut_ptr::<Trb>(), trb_count)
        };
        let ring_phys = cmd_ring.addr();
        trbs[trb_count - 1] = Trb {
            d0: (ring_phys as u32) & !0xF,
            d1: (ring_phys >> 32) as u32,
            d2: 0,
            d3: TRB_CYCLE | TRB_TC | (TRB_TYPE_LINK << TRB_TYPE_SHIFT),
        };

        self.cmd_cycle = true;
        self.cmd_index = 0;
        self.cmd_ring = Some(cmd_ring);

        // Program CRCR
        unsafe {
            let crcr = (ring_phys & !0x3F) | 1; // RCS=1
            write_volatile(&mut (*self.op_regs.as_ptr()).crcr, crcr);
        }

        // Event ring + ERST
        let event_count = 256usize;
        let mut event_ring = PhysBuf::new(event_count * size_of::<Trb>());
        event_ring.fill(0);
        self.event_cycle = true;
        self.event_index = 0;
        let event_phys = event_ring.addr();
        self.event_ring = Some(event_ring);

        let mut erst = PhysBuf::new(size_of::<ErstEntry>());
        erst.fill(0);
        unsafe {
            let e = erst.virt_addr().as_mut_ptr::<ErstEntry>();
            (*e).base = event_phys;
            (*e).size = event_count as u32;
            (*e).rsvd = 0;
        }
        let erst_phys = erst.addr();
        self.erst = Some(erst);

        unsafe {
            let ir0 = &mut (*self.rt_regs.as_ptr()).ir[0];
            write_volatile(&mut ir0.imod, 0);
            write_volatile(&mut ir0.erstsz, 1);
            write_volatile(&mut ir0.erstba, erst_phys & !0x3F);
            write_volatile(&mut ir0.erdp, event_phys & !0xF);
            // IMAN.IE = bit1
            write_volatile(&mut ir0.iman, 1 << 1);
        }
        true
    }

    pub fn poll_drivers(&mut self) {
        self.poll_event_ring();
        for d in self.drivers.iter_mut() {
            d.poll();
        }
    }

    fn ring_doorbell_slot_ep(&mut self, slot_id: u8, ep_id: u8) {
        unsafe {
            write_volatile(
                &mut (*self.db_regs.as_ptr()).db[slot_id as usize],
                ep_id as u32,
            );
        }
    }

    fn ring_doorbell0(&mut self) {
        unsafe {
            write_volatile(&mut (*self.db_regs.as_ptr()).db[0], 0);
        }
    }

    fn push_cmd(&mut self, mut trb: Trb) {
        let ring = self.cmd_ring.as_mut().expect("cmd_ring not init");
        let trb_count = ring.len() / size_of::<Trb>();
        let last = trb_count - 1; // last TRB is the Link TRB

        let mut idx = self.cmd_index;
        let mut cycle = self.cmd_cycle;
        if idx >= last {
            idx = 0;
            cycle = !cycle;
        }

        trb.d3 = (trb.d3 & !TRB_CYCLE) | if cycle { TRB_CYCLE } else { 0 };

        let trbs = unsafe {
            core::slice::from_raw_parts_mut(ring.virt_addr().as_mut_ptr::<Trb>(), trb_count)
        };
        trbs[idx] = trb;

        self.cmd_index = idx + 1;
        self.cmd_cycle = cycle;
        // Ensure TRB visible before doorbell
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.ring_doorbell0();
    }

    fn poll_event_ring(&mut self) {
        let Some(ref mut ring) = self.event_ring else {
            return;
        };
        let trb_count = ring.len() / size_of::<Trb>();
        let trbs = unsafe {
            core::slice::from_raw_parts_mut(ring.virt_addr().as_mut_ptr::<Trb>(), trb_count)
        };

        loop {
            let trb = trbs[self.event_index];
            let cycle = (trb.d3 & TRB_CYCLE) != 0;
            if cycle != self.event_cycle {
                break;
            }
            let ty = (trb.d3 & TRB_TYPE_MASK) >> TRB_TYPE_SHIFT;
            if ty == TRB_TYPE_CMD_COMPLETION_EVENT {
                let cc = (trb.d2 >> 24) & 0xFF;
                let slot_id = (trb.d3 >> 24) as u8;
                self.last_cmd_seen = true;
                self.last_cmd_cc = cc as u8;
                self.last_cmd_slot = slot_id;
                crate::log!("XHCI: Command Completion Event cc={} slot={}", cc, slot_id);
            } else if ty == TRB_TYPE_TRANSFER_EVENT {
                let cc = (trb.d2 >> 24) & 0xFF;
                let slot_id = (trb.d3 >> 24) as u8;
                let ep_id = ((trb.d3 >> 16) & 0x1F) as u8;

                if let Some(Some(st)) = self.slots.get(slot_id as usize) {
                    if let Some(Some(status)) = st.ep_transfers.get(ep_id as usize) {
                        status.store(cc as u8, Ordering::SeqCst);
                    }
                }

                self.last_xfer_seen = true;
                self.last_xfer_cc = cc as u8;
                self.last_xfer_slot = slot_id;
                self.last_xfer_epid = ep_id;
                // crate::log!(
                //     "XHCI: Transfer Event cc={} slot={} epid={}",
                //     cc,
                //     slot_id,
                //     ep_id
                // );
            }
            // advance
            self.event_index += 1;
            if self.event_index >= trb_count {
                self.event_index = 0;
                self.event_cycle = !self.event_cycle;
            }
            // update ERDP: write dequeue pointer and clear EHB (Event Handler Busy) by setting bit3
            let event_phys = ring.addr() + (self.event_index as u64) * (size_of::<Trb>() as u64);
            unsafe {
                let ir0 = &mut (*self.rt_regs.as_ptr()).ir[0];
                write_volatile(&mut ir0.erdp, (event_phys & !0xF) | (1 << 3));
            }
        }
    }

    fn enable_slot(&mut self) -> Option<u8> {
        // Slot type 0 = default
        self.last_cmd_seen = false;
        self.push_cmd(Trb {
            d0: 0,
            d1: 0,
            d2: 0,
            d3: (TRB_TYPE_ENABLE_SLOT_CMD << TRB_TYPE_SHIFT),
        });
        // Poll for completion (polled mode)
        for _ in 0..4000 {
            self.poll_event_ring();
            if self.last_cmd_seen {
                if self.last_cmd_cc == 1 && self.last_cmd_slot != 0 {
                    return Some(self.last_cmd_slot);
                }
                return None;
            }
            sleep_us(50);
        }
        None
    }

    fn context_size(&self) -> usize {
        if self.context_64byte { 64 } else { 32 }
    }

    fn push_ring_trb(ring: &mut PhysBuf, index: &mut usize, cycle: &mut bool, mut trb: Trb) {
        let trb_count = ring.len() / size_of::<Trb>();
        let last = trb_count - 1; // link trb
        if *index >= last {
            *index = 0;
            *cycle = !*cycle;

            // Update Link TRB to match the new cycle bit so the HC accepts it at the end of this pass
            let trbs = unsafe {
                core::slice::from_raw_parts_mut(ring.virt_addr().as_mut_ptr::<Trb>(), trb_count)
            };
            trbs[last].d3 = (trbs[last].d3 & !TRB_CYCLE) | if *cycle { TRB_CYCLE } else { 0 };
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        }

        trb.d3 = (trb.d3 & !TRB_CYCLE) | if *cycle { TRB_CYCLE } else { 0 };
        let trbs = unsafe {
            core::slice::from_raw_parts_mut(ring.virt_addr().as_mut_ptr::<Trb>(), trb_count)
        };
        trbs[*index] = trb;
        *index += 1;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    }

    fn wait_for_xfer(&mut self, slot_id: u8, ep_id: u8, timeout_ms: u64) -> Result<(), UsbError> {
        self.last_xfer_seen = false;
        let mut waited_us = 0u64;
        let timeout_us = timeout_ms * 1000;
        loop {
            self.poll_event_ring();
            if self.last_xfer_seen && self.last_xfer_slot == slot_id && self.last_xfer_epid == ep_id
            {
                if self.last_xfer_cc == 1 {
                    return Ok(());
                }
                return Err(UsbError::UsbError(self.last_xfer_cc as u32));
            }
            if waited_us >= timeout_us {
                crate::log!(
                    "XHCI: wait_for_xfer timeout (slot {}, ep {})",
                    slot_id,
                    ep_id
                );
                return Err(UsbError::Timeout);
            }
            sleep_us(50);
            waited_us += 50;
        }
    }

    fn control_transfer_ep0(
        &mut self,
        slot_id: u8,
        setup: [u8; 8],
        data: Option<&mut [u8]>,
        data_in: bool,
    ) -> Result<usize, UsbError> {
        let Some(Some(st)) = self.slots.get_mut(slot_id as usize) else {
            return Err(UsbError::InvalidDevice);
        };

        let ep_id = 1u8; // EP0

        let setup_d0 = u32::from_le_bytes([setup[0], setup[1], setup[2], setup[3]]);
        let setup_d1 = u32::from_le_bytes([setup[4], setup[5], setup[6], setup[7]]);

        let data_len = data.as_ref().map(|d| d.len()).unwrap_or(0);

        let trt = if data_len == 0 {
            0u32
        } else if data_in {
            3u32
        } else {
            2u32
        };

        // Setup Stage TRB (immediate data)
        Self::push_ring_trb(
            &mut st.ep0_ring,
            &mut st.ep0_index,
            &mut st.ep0_cycle,
            Trb {
                d0: setup_d0,
                d1: setup_d1,
                d2: 8,
                d3: TRB_IDT | (TRB_TYPE_SETUP_STAGE << TRB_TYPE_SHIFT) | (trt << 16),
            },
        );

        // Data stage
        let mut data_buf = if data_len > 0 {
            Some(PhysBuf::new(data_len))
        } else {
            None
        };
        if let Some(ref mut b) = data_buf {
            if !data_in {
                if let Some(src) = data.as_ref() {
                    b.copy_from_slice(src);
                }
            } else {
                b.fill(0);
            }
        }

        if data_len > 0 {
            let p = data_buf.as_ref().unwrap().addr();
            Self::push_ring_trb(
                &mut st.ep0_ring,
                &mut st.ep0_index,
                &mut st.ep0_cycle,
                Trb {
                    d0: (p as u32) & !0xF,
                    d1: (p >> 32) as u32,
                    d2: (data_len as u32),
                    d3: (TRB_TYPE_DATA_STAGE << TRB_TYPE_SHIFT)
                        | if data_in { TRB_DIR_IN } else { 0 },
                },
            );
        }

        // Status stage (direction opposite of data stage, or IN for no-data)
        let status_in = if data_len == 0 { true } else { !data_in };
        Self::push_ring_trb(
            &mut st.ep0_ring,
            &mut st.ep0_index,
            &mut st.ep0_cycle,
            Trb {
                d0: 0,
                d1: 0,
                d2: 0,
                d3: TRB_IOC
                    | (TRB_TYPE_STATUS_STAGE << TRB_TYPE_SHIFT)
                    | if status_in { TRB_DIR_IN } else { 0 },
            },
        );

        self.ring_doorbell_slot_ep(slot_id, ep_id);
        // Increase timeout to 3000ms
        self.wait_for_xfer(slot_id, ep_id, 3000)?;

        if data_in {
            if let (Some(src), Some(dst)) = (data_buf, data) {
                dst.copy_from_slice(&src[..data_len]);
            }
        }

        Ok(data_len)
    }

    fn get_device_descriptor(&mut self, slot_id: u8) -> Result<[u8; 18], UsbError> {
        let mut buf = [0u8; 18];
        let setup = [0x80, 6, 0x00, 0x01, 0x00, 0x00, 18, 0x00];
        self.control_transfer_ep0(slot_id, setup, Some(&mut buf), true)?;
        Ok(buf)
    }

    fn get_config_descriptor(&mut self, slot_id: u8) -> Result<Vec<u8>, UsbError> {
        let mut hdr = [0u8; 9];
        let setup_hdr = [0x80, 6, 0x00, 0x02, 0x00, 0x00, 9, 0x00];
        self.control_transfer_ep0(slot_id, setup_hdr, Some(&mut hdr), true)?;
        let total = u16::from_le_bytes([hdr[2], hdr[3]]) as usize;
        if total < 9 || total > 2048 {
            return Err(UsbError::InvalidDevice);
        }
        let mut buf = vec![0u8; total];
        let setup_full = [
            0x80,
            6,
            0x00,
            0x02,
            0x00,
            0x00,
            (total as u8),
            ((total >> 8) as u8),
        ];
        self.control_transfer_ep0(slot_id, setup_full, Some(&mut buf), true)?;
        Ok(buf)
    }

    fn set_configuration(&mut self, slot_id: u8, config_value: u8) -> Result<(), UsbError> {
        let setup = [0x00, 9, config_value, 0, 0, 0, 0, 0];
        let _ = self.control_transfer_ep0(slot_id, setup, None, true)?;
        sleep_ms(5);
        Ok(())
    }

    fn configure_interrupt_in_endpoint(
        &mut self,
        slot_id: u8,
        port_index: usize,
        speed_psiv: u8,
        ep_num: u8,
        max_packet: u8,
        interval: u8,
        ring_phys: u64,
    ) -> Result<(), UsbError> {
        let ep_id = endpoint_id(ep_num, true);
        let ctx_sz = self.context_size();
        let bytes = ctx_sz * (ep_id as usize + 2); // ICC + contexts up to ep_id
        let mut input_ctx = PhysBuf::new(bytes);
        input_ctx.fill(0);
        let ic_ptr = input_ctx.virt_addr().as_mut_ptr::<u8>();

        unsafe {
            *(ic_ptr.add(4) as *mut u32) = (1u32 << 0) | (1u32 << (ep_id as u32));
        }

        // Slot context
        let slot_off = ctx_sz;
        unsafe {
            let sc = ic_ptr.add(slot_off) as *mut u32;
            let speed_field = (speed_psiv as u32 & 0xF) << 20;
            let entries = (ep_id as u32) << 27;
            sc.add(0).write_volatile(speed_field | entries);
            sc.add(1)
                .write_volatile(((port_index as u32 + 1) & 0xFF) << 16);
        }

        // Endpoint context
        let ep_off = ctx_sz * (ep_id as usize + 1);
        let interval_field = (interval.saturating_sub(1) as u32) << 16;
        unsafe {
            let ec = ic_ptr.add(ep_off) as *mut u32;
            ec.add(0).write_volatile(interval_field);
            let ep_type = 7u32 << 3; // interrupt IN
            ec.add(1)
                .write_volatile(ep_type | ((max_packet as u32) << 16));
            // dword2/3: TR Dequeue Pointer (16-byte aligned), DCS is bit0 of the low dword
            ec.add(2).write_volatile(((ring_phys as u32) & !0xF) | 1);
            ec.add(3).write_volatile((ring_phys >> 32) as u32);
            ec.add(4).write_volatile(8);
        }

        self.last_cmd_seen = false;
        let ic_phys = input_ctx.addr();
        self.push_cmd(Trb {
            d0: (ic_phys as u32) & !0xF,
            d1: (ic_phys >> 32) as u32,
            d2: 0,
            d3: (TRB_TYPE_CONFIGURE_ENDPOINT_CMD << TRB_TYPE_SHIFT) | ((slot_id as u32) << 24),
        });

        for _ in 0..8000 {
            self.poll_event_ring();
            if self.last_cmd_seen {
                if self.last_cmd_cc == 1 {
                    return Ok(());
                }
                return Err(UsbError::UsbError(self.last_cmd_cc as u32));
            }
            sleep_us(50);
        }
        Err(UsbError::Timeout)
    }

    fn try_attach_boot_mouse(&mut self, slot_id: u8, port_index: usize) {
        let Some(Some(st)) = self.slots.get(slot_id as usize) else {
            return;
        };
        let low_speed = st.speed_psiv == 2;

        let dev_desc = match self.get_device_descriptor(slot_id) {
            Ok(d) => d,
            Err(e) => {
                log!(
                    "XHCI: slot {} get_device_descriptor failed: {:?}",
                    slot_id,
                    e
                );
                return;
            }
        };
        let vid = u16::from_le_bytes([dev_desc[8], dev_desc[9]]);
        let pid = u16::from_le_bytes([dev_desc[10], dev_desc[11]]);

        // Give the device a moment to breathe before next control transfer
        sleep_ms(10);

        let cfg = match self.get_config_descriptor(slot_id) {
            Ok(c) => c,
            Err(e) => {
                log!(
                    "XHCI: slot {} get_config_descriptor failed: {:?}",
                    slot_id,
                    e
                );
                return;
            }
        };

        let Some((cfg_value, interface, ep_num, ep_mps, ep_interval)) =
            parse_boot_hid_int_in_endpoint(&cfg, 0x02)
        else {
            log!(
                "XHCI: slot {} not a boot mouse (no int-in endpoint)",
                slot_id
            );
            return;
        };

        if let Err(e) = self.set_configuration(slot_id, if cfg_value == 0 { 1 } else { cfg_value })
        {
            log!("XHCI: slot {} set_configuration failed: {:?}", slot_id, e);
            return;
        }

        let name = usb_ids::lookup(vid, pid)
            .map(|(v, p)| format!("{} {}", v, p))
            .unwrap_or_else(|| format!("Unknown {:04x}:{:04x}", vid, pid));

        let mut dev = UsbDevice {
            port: (port_index + 1) as u8,
            addr: slot_id, // xHCI slot id
            vid,
            pid,
            class: dev_desc[4],
            subclass: dev_desc[5],
            protocol: dev_desc[6],
            max_packet0: dev_desc[7],
            kind: UsbDeviceKind::Mouse,
            name: name.clone(),
            low_speed,
            interface,
            int_in_ep: ep_num,
            int_in_mps: ep_mps,
            int_in_interval: ep_interval,
        };

        let mut driver = crate::driver::usb::hid::MouseDriver::new();
        log!("XHCI: Initializing MouseDriver for {}", name);
        if let Err(e) = driver.init(&mut dev, self) {
            log!("XHCI: MouseDriver init failed: {:?}", e);
            return;
        }
        log!("XHCI: MouseDriver active!");
        self.drivers.push(Box::new(driver));
    }

    fn address_device(&mut self, slot_id: u8, port_index: usize) -> bool {
        let Some(ref dcbaa) = self.dcbaa else {
            return false;
        };
        let ctx_sz = self.context_size();
        let speed = {
            let portsc = unsafe { read_volatile(self.portsc_ptr(port_index)) };
            ((portsc & PORTSC_SPEED_MASK) >> PORTSC_SPEED_SHIFT) as u8
        };

        // Device context: slot ctx + ep0 ctx (2 contexts)
        let dev_ctx_bytes = ctx_sz * 2;
        let mut dev_ctx = PhysBuf::new(dev_ctx_bytes);
        dev_ctx.fill(0);

        // EP0 transfer ring
        let trb_count = 256usize;
        let mut ep0_ring = PhysBuf::new(trb_count * size_of::<Trb>());
        ep0_ring.fill(0);
        let ep0_phys = ep0_ring.addr();
        let ep0_trbs = unsafe {
            core::slice::from_raw_parts_mut(ep0_ring.virt_addr().as_mut_ptr::<Trb>(), trb_count)
        };
        ep0_trbs[trb_count - 1] = Trb {
            d0: (ep0_phys as u32) & !0xF,
            d1: (ep0_phys >> 32) as u32,
            d2: 0,
            d3: TRB_CYCLE | TRB_TC | (TRB_TYPE_LINK << TRB_TYPE_SHIFT),
        };
        let ep0_cycle = true;
        let ep0_index = 0;

        // Input context: ICC + slot ctx + ep0 ctx (3 contexts)
        let input_ctx_bytes = ctx_sz * 3;
        let mut input_ctx = PhysBuf::new(input_ctx_bytes);
        input_ctx.fill(0);
        let ic_ptr = input_ctx.virt_addr().as_mut_ptr::<u8>();

        // Input Control Context
        unsafe {
            // drop flags @ 0, add flags @ 4
            *(ic_ptr.add(4) as *mut u32) = 0b11;
        }

        // Slot context (in input ctx) at offset ctx_sz
        let slot_off = ctx_sz;
        unsafe {
            let sc = ic_ptr.add(slot_off) as *mut u32;
            // dword0: speed in bits 20..23, context entries in bits 27..31
            let speed_field = (speed as u32 & 0xF) << 20;
            let entries = 1u32 << 27;
            sc.add(0).write_volatile(speed_field | entries);
            // dword1: root hub port number in bits 16..23
            sc.add(1)
                .write_volatile(((port_index as u32 + 1) & 0xFF) << 16);
        }

        // EP0 context (in input ctx) at offset ctx_sz*2
        let ep0_off = ctx_sz * 2;
        let max_packet = match speed {
            2 => 8u32,  // low-speed
            _ => 64u32, // default
        };
        unsafe {
            let ec = ic_ptr.add(ep0_off) as *mut u32;
            // dword1: EP type in bits 3..5, max packet in bits 16..31
            let ep_type = 4u32 << 3; // control
            ec.add(1).write_volatile(ep_type | (max_packet << 16));
            // dword2/3: TR Dequeue Pointer (16-byte aligned), DCS is bit0 of the low dword
            ec.add(2).write_volatile(((ep0_phys as u32) & !0xF) | 1);
            ec.add(3).write_volatile((ep0_phys >> 32) as u32);
            // dword4: average TRB length
            ec.add(4).write_volatile(8);
        }

        // Write DCBAA[slot] = dev_ctx phys
        unsafe {
            let dcbaa_ptr = dcbaa.virt_addr().as_mut_ptr::<u64>();
            let dcbaa_slice = core::slice::from_raw_parts_mut(
                dcbaa_ptr,
                (self.max_device_slots as usize + 1).max(1),
            );
            dcbaa_slice[slot_id as usize] = dev_ctx.addr();
        }

        // Address Device command
        self.last_cmd_seen = false;
        let ic_phys = input_ctx.addr();
        self.push_cmd(Trb {
            d0: (ic_phys as u32) & !0xF,
            d1: (ic_phys >> 32) as u32,
            d2: 0,
            d3: (TRB_TYPE_ADDRESS_DEVICE_CMD << TRB_TYPE_SHIFT) | ((slot_id as u32) << 24),
        });

        for _ in 0..8000 {
            self.poll_event_ring();
            if self.last_cmd_seen {
                let ok = self.last_cmd_cc == 1;
                if ok {
                    self.slots[slot_id as usize] = Some(XhciSlotState {
                        port_index,
                        speed_psiv: speed,
                        input_ctx,
                        dev_ctx,
                        ep0_ring,
                        ep0_cycle,
                        ep0_index,
                        ep_transfers: vec![None; 32],
                    });
                }
                return ok;
            }
            sleep_us(50);
        }
        false
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
                // Once we hand the event ring to an interrupt transfer handle (mouse),
                // issuing more xHCI commands from here would race on the event ring.
                if !self.drivers.is_empty() {
                    self.port_status_cache[port_index] = portsc;
                    continue;
                }
                if ccs != 0 {
                    let speed = Self::port_speed_name(portsc);
                    crate::log!(
                        "XHCI: USB device connected on port {} ({})",
                        port_index + 1,
                        speed
                    );
                    self.reset_port(port_index);
                    let slot = self.enable_slot();
                    crate::log!(
                        "XHCI: enable_slot result: {:?} (cc={}, slot={})",
                        slot,
                        self.last_cmd_cc,
                        self.last_cmd_slot
                    );
                    if let Some(slot_id) = slot {
                        let ok = self.address_device(slot_id, port_index);
                        crate::log!(
                            "XHCI: address_device slot {} port {} -> {} (cc={})",
                            slot_id,
                            port_index + 1,
                            ok,
                            self.last_cmd_cc
                        );
                        if ok {
                            self.try_attach_boot_mouse(slot_id, port_index);
                        }
                    }
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

    fn reset_port(&mut self, port_index: usize) {
        if !self.mmio_valid {
            return;
        }
        let ptr = self.portsc_ptr(port_index);
        unsafe {
            let mut portsc = read_volatile(ptr);
            // Ensure power
            portsc |= PORTSC_PP;
            // Clear change bits
            portsc |= PORTSC_W1C_MASK;
            // Port reset
            portsc |= PORTSC_PR;
            write_volatile(ptr, portsc);
        }
        // wait for PR to clear and PED set
        for _ in 0..2000 {
            let portsc = unsafe { read_volatile(ptr) };
            if (portsc & PORTSC_PR) == 0 && (portsc & PORTSC_PED) != 0 {
                break;
            }
            sleep_us(100);
        }
        let portsc = unsafe { read_volatile(ptr) };
        let speed = (portsc & PORTSC_SPEED_MASK) >> PORTSC_SPEED_SHIFT;
        crate::log!(
            "XHCI: port {} reset done (ccs={} ped={} speed={})",
            port_index + 1,
            (portsc & PORTSC_CCS) != 0,
            (portsc & PORTSC_PED) != 0,
            speed
        );
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

    #[allow(dead_code)]
    fn log_capability_registers(&self) {
        /* Kernel log implementation goes here */
    }
}

/* =========================================================
   HostController impl (so hid.rs can run on xHCI)
   Note: `addr` is treated as xHCI Slot ID.
========================================================= */

impl HostController for XhciDriver {
    fn control_transfer(
        &mut self,
        addr: u8,
        _endp: u8,
        setup: [u8; 8],
        data: Option<&mut [u8]>,
        _low_speed: bool,
    ) -> Result<usize, UsbError> {
        let data_in = (setup[0] & 0x80) != 0;
        self.control_transfer_ep0(addr, setup, data, data_in)
    }

    fn schedule_interrupt(
        &mut self,
        addr: u8,
        endp: u8,
        max_packet_size: u8,
        interval: u8,
        buf_phys: u64,
        len: usize,
        _low_speed: bool,
    ) -> Result<Box<dyn InterruptTransfer>, UsbError> {
        let slot_id = addr;
        let (port_index, speed_psiv) = {
            let Some(Some(st)) = self.slots.get(slot_id as usize) else {
                return Err(UsbError::InvalidDevice);
            };
            (st.port_index, st.speed_psiv)
        };

        let ep_id = endpoint_id(endp, true);

        // Transfer ring
        let trb_count = 256usize;
        let mut ring = PhysBuf::new(trb_count * size_of::<Trb>());
        ring.fill(0);
        let ring_phys = ring.addr();
        unsafe {
            let trbs =
                core::slice::from_raw_parts_mut(ring.virt_addr().as_mut_ptr::<Trb>(), trb_count);
            trbs[trb_count - 1] = Trb {
                d0: (ring_phys as u32) & !0xF,
                d1: (ring_phys >> 32) as u32,
                d2: 0,
                d3: TRB_CYCLE | TRB_TC | (TRB_TYPE_LINK << TRB_TYPE_SHIFT),
            };
        }

        // Configure endpoint in controller
        self.configure_interrupt_in_endpoint(
            slot_id,
            port_index,
            speed_psiv,
            endp,
            max_packet_size,
            interval,
            ring_phys,
        )?;

        let Some(Some(st)) = self.slots.get_mut(slot_id as usize) else {
            return Err(UsbError::InvalidDevice);
        };

        let status = Arc::new(AtomicU8::new(0));
        st.ep_transfers[ep_id as usize] = Some(status.clone());

        // Prime first transfer
        let mut it = XhciInterruptInTransfer {
            ring,
            ring_index: 0,
            ring_cycle: true,
            buf_phys,
            len,
            slot_id,
            ep_id,
            db_regs: self.db_regs.as_ptr(),
            status,
            completed: false,
        };

        it.submit_one();
        Ok(Box::new(it))
    }
}

struct XhciInterruptInTransfer {
    ring: PhysBuf,
    ring_index: usize,
    ring_cycle: bool,

    buf_phys: u64,
    len: usize,
    slot_id: u8,
    ep_id: u8,

    db_regs: *mut XhciDoorbellRegisters,
    status: Arc<AtomicU8>,

    completed: bool,
}

impl XhciInterruptInTransfer {
    fn submit_one(&mut self) {
        // Normal TRB
        let p = self.buf_phys;
        let mut trb = Trb {
            d0: (p as u32) & !0xF,
            d1: (p >> 32) as u32,
            d2: self.len as u32,
            d3: TRB_IOC | (TRB_TYPE_NORMAL << TRB_TYPE_SHIFT),
        };

        let trb_count = self.ring.len() / size_of::<Trb>();
        let last = trb_count - 1;
        if self.ring_index >= last {
            self.ring_index = 0;
            self.ring_cycle = !self.ring_cycle;
        }
        trb.d3 = (trb.d3 & !TRB_CYCLE) | if self.ring_cycle { TRB_CYCLE } else { 0 };

        unsafe {
            let trbs = core::slice::from_raw_parts_mut(
                self.ring.virt_addr().as_mut_ptr::<Trb>(),
                trb_count,
            );
            trbs[self.ring_index] = trb;
        }
        self.ring_index += 1;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        unsafe {
            write_volatile(
                &mut (*self.db_regs).db[self.slot_id as usize],
                self.ep_id as u32,
            );
        }
        self.completed = false;
    }
}

impl InterruptTransfer for XhciInterruptInTransfer {
    fn poll(&mut self) -> bool {
        let s = self.status.swap(0, Ordering::SeqCst);
        if s != 0 {
            self.completed = true;
        }
        self.completed
    }

    fn ack(&mut self) {
        if self.completed {
            self.submit_one();
        }
    }
}
