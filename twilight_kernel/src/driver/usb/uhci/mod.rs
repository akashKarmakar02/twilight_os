#![allow(dead_code)]
use crate::driver::timer::wait;
use crate::driver::usb::interfaces::{
    HostController, InterruptTransfer, UsbDevice, UsbDeviceKind, UsbDriver, UsbError,
};
use crate::driver::usb::usb_ids;
use crate::log;
use crate::sys::memory::phys::PhysBuf;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use core::mem::size_of;
use core::sync::atomic::{Ordering, fence};
use x86_64::instructions::port::Port;

pub struct UHci {
    io_base: u16,
    usb_cmd: Port<u16>,
    usb_status: Port<u16>,
    usb_interrupt: Port<u16>,
    usb_frame_no: Port<u16>,
    framelist_addr: Port<u32>,
    sof_modifier: Port<u16>,
    ctrl1: Port<u16>,
    ctrl2: Port<u16>,

    frame_list: PhysBuf, // 1024 u32 entries, 4KiB aligned
    async_qh: PhysBuf,   // one QH used for control transfers during enumeration

    // Generic Driver Support
    drivers: alloc::vec::Vec<Box<dyn UsbDriver>>,
    interrupt_root: u32,
}

unsafe impl Send for UHci {}
unsafe impl Sync for UHci {}

#[derive(Debug, Clone, Copy)]
pub struct UhciTD {
    pub link_ptr: u32,
    pub ctrl_status: u32,
    pub token: u32,
    pub buffer_ptr: u32,
}

impl UHci {
    pub fn handle_interrupt(&mut self) -> bool {
        unsafe {
            let status = self.usb_status.read();
            // Linux uhci_hcd treats "no bits" or only HCHalted as not ours.
            if status == 0 || status == USBSTS_HCHALTED {
                return false;
            }
            let w1c = status & USBSTS_W1C_MASK;
            if w1c != 0 {
                self.usb_status.write(w1c);
            }

            if (status & USBSTS_HCHALTED) != 0 {
                log!("UHCI({:#x}): controller halted (USBSTS={:#x})", self.io_base, status);
            } else if (status & (USBSTS_USBERRINT | USBSTS_HSE | USBSTS_HC_PROCESS_ERR)) != 0 {
                log!("UHCI({:#x}): interrupt error status={:#x}", self.io_base, status);
            }

            return (status
                & (USBSTS_USBINT
                    | USBSTS_USBERRINT
                    | USBSTS_RESUME_DETECT
                    | USBSTS_HSE
                    | USBSTS_HC_PROCESS_ERR
                    | USBSTS_HCHALTED))
                != 0;
        }
    }
}

#[allow(dead_code)]
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct UhciQH {
    pub head_link: u32,
    pub element_link: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct SetupPacket {
    bm_request_type: u8,
    b_request: u8,
    w_value: u16,
    w_index: u16,
    w_length: u16,
}

#[derive(Debug, Clone, Copy)]
enum UhciError {
    Timeout,
    Halted,
    Stalled,
    UsbError(u32),
}

const USBCMD_RUN_STOP: u16 = 1 << 0;
const USBCMD_HCRESET: u16 = 1 << 1;
const USBCMD_CONFIGURE_FLAG: u16 = 1 << 6;
const USBCMD_MAX_PACKET_64: u16 = 1 << 7;

const USBSTS_USBINT: u16 = 1 << 0;
const USBSTS_USBERRINT: u16 = 1 << 1;
const USBSTS_RESUME_DETECT: u16 = 1 << 2;
const USBSTS_HSE: u16 = 1 << 3;
const USBSTS_HC_PROCESS_ERR: u16 = 1 << 4;
const USBSTS_HCHALTED: u16 = 1 << 5;
const USBSTS_W1C_MASK: u16 =
    USBSTS_USBINT | USBSTS_USBERRINT | USBSTS_RESUME_DETECT | USBSTS_HSE | USBSTS_HC_PROCESS_ERR;

const PORTSC_CCS: u16 = 1 << 0;
const PORTSC_CSC: u16 = 1 << 1;
const PORTSC_PE: u16 = 1 << 2;
const PORTSC_PEC: u16 = 1 << 3;
const PORTSC_LSDA: u16 = 1 << 8; // low-speed device attached
const PORTSC_PR: u16 = 1 << 9; // port reset

const LINK_TERMINATE: u32 = 1 << 0;
const LINK_QH: u32 = 1 << 1;
const LINK_DEPTH_FIRST: u32 = 1 << 2;

const PID_SETUP: u8 = 0x2D;
const PID_IN: u8 = 0x69;
const PID_OUT: u8 = 0xE1;

const TD_STATUS_ACTLEN_MASK: u32 = 0x7FF;
const TD_STATUS_ACTIVE: u32 = 1 << 23;
const TD_STATUS_IOC: u32 = 1 << 24;
const TD_STATUS_LS: u32 = 1 << 26;
const TD_STATUS_ERRCNT_SHIFT: u32 = 27;
const TD_STATUS_SPD: u32 = 1 << 29;
const TD_STATUS_STALLED: u32 = 1 << 22;
const TD_STATUS_BITSTUFF: u32 = 1 << 17;
const TD_STATUS_CRC_TIMEOUT: u32 = 1 << 18;
const TD_STATUS_BABBLE: u32 = 1 << 20;
const TD_STATUS_DATABUF: u32 = 1 << 21;
const TD_TOKEN_TOGGLE: u32 = 1 << 19;

#[inline(always)]
fn sleep_us(us: u64) {
    wait(us * 1_000);
}

#[inline(always)]
fn sleep_ms(ms: u64) {
    wait(ms * 1_000_000);
}

impl UHci {
    pub fn new(io_base: u16) -> Self {
        Self {
            io_base,
            usb_cmd: Port::new(io_base + 0x00),
            usb_status: Port::new(io_base + 0x02),
            usb_interrupt: Port::new(io_base + 0x04),
            usb_frame_no: Port::new(io_base + 0x06),
            framelist_addr: Port::new(io_base + 0x08),
            sof_modifier: Port::new(io_base + 0x0c),
            ctrl1: Port::new(io_base + 0x10),
            ctrl2: Port::new(io_base + 0x12),

            frame_list: PhysBuf::new(0x1000),
            async_qh: PhysBuf::new(0x1000),

            drivers: alloc::vec::Vec::new(),
            interrupt_root: 0, // Will be init_schedule
        }
    }

    pub fn list(&mut self) {
        if let Err(e) = self.init_controller() {
            log!("UHCI({:#x}): init failed: {:?}", self.io_base, e);
            return;
        }

        for port in 1..=2u8 {
            let portsc = self.read_portsc(port);
            if (portsc & PORTSC_CCS) == 0 {
                continue;
            }

            let is_low_speed = (portsc & PORTSC_LSDA) != 0;
            log!(
                "UHCI({:#x}): device present on port {} (speed={})",
                self.io_base,
                port,
                if is_low_speed { "low" } else { "full" }
            );

            if let Err(e) = self.reset_enable_port(port) {
                log!(
                    "UHCI({:#x}): port {} reset/enable failed: {:?}",
                    self.io_base,
                    port,
                    e
                );
                continue;
            }

            let is_low_speed = (self.read_portsc(port) & PORTSC_LSDA) != 0;
            match self.enumerate_device_on_port(port, is_low_speed) {
                Ok(dev) => {
                    log!(
                        "UHCI({:#x}): port {} addr {} type={} name=\"{}\" vid:pid {:04x}:{:04x} class {:02x}/{:02x}/{:02x} mps0 {}",
                        self.io_base,
                        port,
                        dev.addr,
                        dev.kind.as_str(),
                        dev.name.as_str(),
                        dev.vid,
                        dev.pid,
                        dev.class,
                        dev.subclass,
                        dev.protocol,
                        dev.max_packet0
                    );
                }
                Err(e) => {
                    log!(
                        "UHCI({:#x}): port {} enumeration failed: {:?}",
                        self.io_base,
                        port,
                        e
                    );
                }
            }
        }
    }

    fn init_controller(&mut self) -> Result<(), UhciError> {
        // Stop
        unsafe {
            let cmd = self.usb_cmd.read();
            self.usb_cmd.write(cmd & !USBCMD_RUN_STOP);
            self.usb_interrupt.write(0);
        }

        // Reset
        unsafe {
            self.usb_cmd.write(USBCMD_HCRESET);
        }
        // Spec says HCRESET is self-clearing; poll for it to clear.
        for _ in 0..20_000 {
            let cmd = unsafe { self.usb_cmd.read() };
            if (cmd & USBCMD_HCRESET) == 0 {
                break;
            }
            sleep_us(10);
        }

        // Clear status (R/WC)
        unsafe {
            self.usb_status.write(0xFFFF);
        }

        self.init_schedule();

        // Start
        unsafe {
            self.usb_interrupt.write(0xF); // Enable all interrupts (Timeout, Resume, IOC, Short Packet)
            self.usb_cmd
                .write(USBCMD_RUN_STOP | USBCMD_CONFIGURE_FLAG | USBCMD_MAX_PACKET_64);
        }

        Ok(())
    }

    fn init_schedule(&mut self) {
        let qh_phys = (self.async_qh.addr() as u32) & 0xFFFF_FFF0;
        let frame_ptr = qh_phys | LINK_QH;

        // Interrupt Root initially points to Async QH (the end of the interrupt chain)
        self.interrupt_root = frame_ptr;

        let frame_list_ptr = self.frame_list_mut_u32();

        for entry in frame_list_ptr.iter_mut() {
            *entry = frame_ptr;
        }

        let qh = self.async_qh_mut();
        qh.head_link = LINK_TERMINATE;
        qh.element_link = LINK_TERMINATE;

        unsafe {
            self.usb_frame_no.write(0);
            self.framelist_addr.write(self.frame_list.addr() as u32);
        }
    }

    fn read_portsc(&mut self, port: u8) -> u16 {
        unsafe {
            match port {
                1 => self.ctrl1.read(),
                2 => self.ctrl2.read(),
                _ => 0,
            }
        }
    }

    fn write_portsc(&mut self, port: u8, val: u16) {
        unsafe {
            match port {
                1 => self.ctrl1.write(val),
                2 => self.ctrl2.write(val),
                _ => {}
            }
        }
    }

    fn reset_enable_port(&mut self, port: u8) -> Result<(), UhciError> {
        let mut v = self.read_portsc(port);
        if (v & PORTSC_CCS) == 0 {
            return Err(UhciError::Halted);
        }

        // Clear change bits
        self.write_portsc(port, v | PORTSC_CSC | PORTSC_PEC);
        sleep_us(100);

        // Port reset (10ms+)
        v = self.read_portsc(port);
        self.write_portsc(port, v | PORTSC_PR);
        sleep_ms(50);
        v = self.read_portsc(port);
        self.write_portsc(port, v & !PORTSC_PR);
        sleep_ms(10);

        // Enable port
        v = self.read_portsc(port);
        self.write_portsc(port, v | PORTSC_PE | PORTSC_CSC | PORTSC_PEC);
        sleep_ms(1);

        Ok(())
    }

    fn frame_list_mut_u32(&mut self) -> &mut [u32; 1024] {
        let ptr = self.frame_list.virt_addr().as_u64() as *mut u32;
        unsafe { &mut *(core::ptr::slice_from_raw_parts_mut(ptr, 1024) as *mut [u32; 1024]) }
    }

    fn async_qh_mut(&mut self) -> &mut UhciQH {
        let ptr = self.async_qh.virt_addr().as_u64() as *mut UhciQH;
        unsafe { &mut *ptr }
    }

    fn controller_halted(&mut self) -> bool {
        (unsafe { self.usb_status.read() } & USBSTS_HCHALTED) != 0
    }

    fn td_encode_len(len: usize) -> u32 {
        if len == 0 { 0x7FF } else { (len as u32) - 1 }
    }

    fn make_td(
        next: u32,
        pid: u8,
        addr: u8,
        endp: u8,
        toggle: bool,
        len: usize,
        buf_phys: u32,
        low_speed: bool,
        ioc: bool,
        spd: bool,
    ) -> UhciTD {
        let ctrl_status = TD_STATUS_ACTLEN_MASK
            | TD_STATUS_ACTIVE
            | (3u32 << TD_STATUS_ERRCNT_SHIFT)
            | if low_speed { TD_STATUS_LS } else { 0 }
            | if ioc { TD_STATUS_IOC } else { 0 }
            | if spd { TD_STATUS_SPD } else { 0 };

        let token = (pid as u32)
            | ((addr as u32) << 8)
            | ((endp as u32) << 15)
            | ((toggle as u32) << 19)
            | (Self::td_encode_len(len) << 21);

        UhciTD {
            link_ptr: if (next & LINK_TERMINATE) != 0 {
                LINK_TERMINATE
            } else {
                next | LINK_DEPTH_FIRST
            },
            ctrl_status,
            token,
            buffer_ptr: buf_phys,
        }
    }

    fn run_control_transfer(
        &mut self,
        addr: u8,
        low_speed: bool,
        setup: SetupPacket,
        data_in: bool,
        data_buf: Option<&mut PhysBuf>,
        data_len: usize,
        max_packet: usize,
        timeout_ms: u64,
    ) -> Result<(), UhciError> {
        let setup_buf = {
            let b = PhysBuf::new(size_of::<SetupPacket>());
            let p = b.virt_addr().as_u64() as *mut SetupPacket;
            unsafe { p.write_unaligned(setup) };
            b
        };

        let data_phys = data_buf.as_ref().map(|b| b.addr() as u32).unwrap_or(0);

        let data_packets = if data_len == 0 {
            0
        } else {
            (data_len + max_packet - 1) / max_packet
        };

        let td_count = 1 + data_packets + 1;
        let td_mem = PhysBuf::new(td_count * size_of::<UhciTD>());

        let td_virt = td_mem.virt_addr().as_u64() as *mut UhciTD;
        let td_phys_base = (td_mem.addr() as u32) & 0xFFFF_FFF0;

        let td_phys = |idx: usize| -> u32 { td_phys_base + (idx as u32) * 16 };

        // Setup stage (always DATA0)
        unsafe {
            td_virt.add(0).write_unaligned(Self::make_td(
                td_phys(1),
                PID_SETUP,
                addr,
                0,
                false,
                size_of::<SetupPacket>(),
                setup_buf.addr() as u32,
                low_speed,
                false,
                false,
            ));
        }

        // Data stage (DATA1 first, toggle each TD)
        let mut offset = 0usize;
        for i in 0..data_packets {
            let len = core::cmp::min(max_packet, data_len - offset);
            let is_last_data = i + 1 == data_packets;
            let next = if is_last_data {
                td_phys(1 + data_packets)
            } else {
                td_phys(1 + i + 1)
            };
            let pid = if data_in { PID_IN } else { PID_OUT };
            let toggle = (i % 2) == 0; // DATA1 for first packet
            let buf = if data_len == 0 {
                0
            } else {
                data_phys + offset as u32
            };

            unsafe {
                td_virt.add(1 + i).write_unaligned(Self::make_td(
                    next,
                    pid,
                    addr,
                    0,
                    toggle,
                    len,
                    buf,
                    low_speed,
                    false,
                    data_in && is_last_data,
                ));
            }
            offset += len;
        }

        // Status stage (DATA1, opposite direction of data, or IN if no data)
        let status_pid = if data_len == 0 {
            PID_IN
        } else if data_in {
            PID_OUT
        } else {
            PID_IN
        };
        unsafe {
            td_virt.add(td_count - 1).write_unaligned(Self::make_td(
                LINK_TERMINATE,
                status_pid,
                addr,
                0,
                true,
                0,
                0,
                low_speed,
                true,
                false,
            ));
        }

        fence(Ordering::SeqCst);

        // Link into the async QH and wait for completion.
        {
            let qh = self.async_qh_mut();
            qh.head_link = LINK_TERMINATE;
            qh.element_link = td_phys(0) | LINK_DEPTH_FIRST;
        }

        // Clear status bits and ensure controller is running.
        unsafe {
            self.usb_status.write(0xFFFF);
            let cmd = self.usb_cmd.read();
            if (cmd & USBCMD_RUN_STOP) == 0 {
                self.usb_cmd
                    .write(USBCMD_RUN_STOP | USBCMD_CONFIGURE_FLAG | USBCMD_MAX_PACKET_64);
            }
        }

        let timeout_us = timeout_ms * 1000;
        let mut waited = 0u64;
        loop {
            if self.controller_halted() {
                return Err(UhciError::Halted);
            }

            // If any TD completed with an error, return early. This avoids waiting forever
            // for later TDs that will never execute.
            for i in 0..td_count {
                let td = unsafe { td_virt.add(i).read_unaligned() };
                if (td.ctrl_status & TD_STATUS_ACTIVE) != 0 {
                    continue;
                }
                if (td.ctrl_status & TD_STATUS_STALLED) != 0 {
                    return Err(UhciError::Stalled);
                }
                let err = td.ctrl_status & !TD_STATUS_ACTLEN_MASK;
                let masked = err
                    & !(TD_STATUS_IOC
                        | TD_STATUS_LS
                        | TD_STATUS_SPD
                        | (3 << TD_STATUS_ERRCNT_SHIFT));
                if masked != 0 {
                    return Err(UhciError::UsbError(td.ctrl_status));
                }
            }

            let mut all_done = true;
            for i in 0..td_count {
                let td = unsafe { td_virt.add(i).read_unaligned() };
                if (td.ctrl_status & TD_STATUS_ACTIVE) != 0 {
                    all_done = false;
                    break;
                }
            }
            if all_done {
                break;
            }

            if waited >= timeout_us {
                return Err(UhciError::Timeout);
            }
            sleep_us(50);
            waited += 50;
        }

        // Detach TD list from schedule so it doesn't get re-walked.
        self.async_qh_mut().element_link = LINK_TERMINATE;
        fence(Ordering::SeqCst);

        Ok(())
    }

    fn get_device_descriptor_prefix(
        &mut self,
        low_speed: bool,
    ) -> Result<(u8, [u8; 8]), UhciError> {
        let mut buf = PhysBuf::new(8);
        for b in buf.iter_mut() {
            *b = 0;
        }
        let setup = SetupPacket {
            bm_request_type: 0x80,
            b_request: 6,
            w_value: 0x0100u16.to_le(),
            w_index: 0u16.to_le(),
            w_length: 8u16.to_le(),
        };
        self.run_control_transfer(0, low_speed, setup, true, Some(&mut buf), 8, 8, 200)?;
        let mut out = [0u8; 8];
        out.copy_from_slice(&buf[..8]);
        let mps0 = out[7] as u8;
        Ok((mps0, out))
    }

    fn set_address(&mut self, low_speed: bool, addr: u8) -> Result<(), UhciError> {
        let setup = SetupPacket {
            bm_request_type: 0x00,
            b_request: 5,
            w_value: (addr as u16).to_le(),
            w_index: 0u16.to_le(),
            w_length: 0u16.to_le(),
        };
        self.run_control_transfer(0, low_speed, setup, true, None, 0, 8, 200)?;
        // USB spec: wait at least 2ms after SetAddress.
        sleep_ms(2);
        Ok(())
    }

    fn get_device_descriptor_full(
        &mut self,
        addr: u8,
        low_speed: bool,
        mps0: usize,
    ) -> Result<[u8; 18], UhciError> {
        let mut buf = PhysBuf::new(18);
        for b in buf.iter_mut() {
            *b = 0;
        }
        let setup = SetupPacket {
            bm_request_type: 0x80,
            b_request: 6,
            w_value: 0x0100u16.to_le(),
            w_index: 0u16.to_le(),
            w_length: 18u16.to_le(),
        };
        self.run_control_transfer(addr, low_speed, setup, true, Some(&mut buf), 18, mps0, 300)?;
        let mut out = [0u8; 18];
        out.copy_from_slice(&buf[..18]);
        Ok(out)
    }

    fn get_config_descriptor(
        &mut self,
        addr: u8,
        low_speed: bool,
        mps0: usize,
    ) -> Result<PhysBuf, UhciError> {
        let mut hdr = PhysBuf::new(9);
        hdr.fill(0);
        let setup_hdr = SetupPacket {
            bm_request_type: 0x80,
            b_request: 6,
            w_value: 0x0200u16.to_le(),
            w_index: 0u16.to_le(),
            w_length: 9u16.to_le(),
        };
        self.run_control_transfer(
            addr,
            low_speed,
            setup_hdr,
            true,
            Some(&mut hdr),
            9,
            mps0,
            300,
        )?;
        let total = u16::from_le_bytes([hdr[2], hdr[3]]) as usize;
        if total < 9 || total > 2048 {
            return Err(UhciError::UsbError(total as u32));
        }

        let mut buf = PhysBuf::new(total);
        buf.fill(0);
        let setup_full = SetupPacket {
            bm_request_type: 0x80,
            b_request: 6,
            w_value: 0x0200u16.to_le(),
            w_index: 0u16.to_le(),
            w_length: (total as u16).to_le(),
        };
        self.run_control_transfer(
            addr,
            low_speed,
            setup_full,
            true,
            Some(&mut buf),
            total,
            mps0,
            500,
        )?;
        Ok(buf)
    }

    fn set_configuration(
        &mut self,
        addr: u8,
        low_speed: bool,
        mps0: usize,
        config_value: u8,
    ) -> Result<(), UhciError> {
        let setup = SetupPacket {
            bm_request_type: 0x00,
            b_request: 9, // SET_CONFIGURATION
            w_value: (config_value as u16).to_le(),
            w_index: 0u16.to_le(),
            w_length: 0u16.to_le(),
        };
        self.run_control_transfer(addr, low_speed, setup, true, None, 0, mps0, 500)?;
        sleep_ms(5);
        Ok(())
    }

    fn parse_boot_hid_int_in_endpoint(cfg: &[u8], want_protocol: u8) -> Option<(u8, u8, u8, u8)> {
        // Returns (interface, ep_num, max_packet, interval)
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

    fn detect_device_kind(
        &mut self,
        addr: u8,
        low_speed: bool,
        mps0: usize,
        dev_class: u8,
        dev_subclass: u8,
        dev_protocol: u8,
    ) -> UsbDeviceKind {
        match dev_class {
            0x09 => return UsbDeviceKind::Hub,
            0x08 => return UsbDeviceKind::MassStorage,
            0x02 => return UsbDeviceKind::Communication,
            0x0E => return UsbDeviceKind::Video,
            0x01 => return UsbDeviceKind::Audio,
            0x03 => {
                if dev_subclass == 0x01 && dev_protocol == 0x01 {
                    return UsbDeviceKind::Keyboard;
                }
                if dev_subclass == 0x01 && dev_protocol == 0x02 {
                    return UsbDeviceKind::Mouse;
                }
                return UsbDeviceKind::Hid;
            }
            _ => {}
        }

        if dev_class != 0x00 {
            return UsbDeviceKind::Unknown;
        }

        let cfg = match self.get_config_descriptor(addr, low_speed, mps0) {
            Ok(b) => b,
            Err(_) => return UsbDeviceKind::Unknown,
        };

        let mut off = 0usize;
        while off + 2 <= cfg.len() {
            let len = cfg[off] as usize;
            let dtype = cfg[off + 1];
            if len < 2 || off + len > cfg.len() {
                break;
            }
            if dtype == 0x04 && len >= 9 {
                let if_class = cfg[off + 5];
                let if_subclass = cfg[off + 6];
                let if_protocol = cfg[off + 7];
                if if_class == 0x03 {
                    if if_subclass == 0x01 && if_protocol == 0x01 {
                        return UsbDeviceKind::Keyboard;
                    }
                    if if_subclass == 0x01 && if_protocol == 0x02 {
                        return UsbDeviceKind::Mouse;
                    }
                    return UsbDeviceKind::Hid;
                }
                if if_class == 0x09 {
                    return UsbDeviceKind::Hub;
                }
                if if_class == 0x08 {
                    return UsbDeviceKind::MassStorage;
                }
                if if_class == 0x02 {
                    return UsbDeviceKind::Communication;
                }
                if if_class == 0x0E {
                    return UsbDeviceKind::Video;
                }
                if if_class == 0x01 {
                    return UsbDeviceKind::Audio;
                }
            }
            off += len;
        }

        UsbDeviceKind::Unknown
    }

    fn get_langid(&mut self, addr: u8, low_speed: bool, mps0: usize) -> Result<u16, UhciError> {
        let mut buf = PhysBuf::new(4);
        buf.fill(0);
        let setup = SetupPacket {
            bm_request_type: 0x80,
            b_request: 6,
            w_value: 0x0300u16.to_le(),
            w_index: 0u16.to_le(),
            w_length: 4u16.to_le(),
        };
        self.run_control_transfer(addr, low_speed, setup, true, Some(&mut buf), 4, mps0, 500)?;
        let len = buf[0] as usize;
        if len < 4 {
            return Err(UhciError::UsbError(len as u32));
        }
        Ok(u16::from_le_bytes([buf[2], buf[3]]))
    }

    fn get_string_descriptor(
        &mut self,
        addr: u8,
        low_speed: bool,
        mps0: usize,
        index: u8,
        langid: u16,
    ) -> Result<String, UhciError> {
        if index == 0 {
            return Ok(String::new());
        }

        // First fetch a small header to learn the real length.
        // Some devices are picky if you request only 2 bytes, so use 4.
        let mut hdr = PhysBuf::new(4);
        hdr.fill(0);
        let w_value = ((0x03u16) << 8) | (index as u16);
        let setup_hdr = SetupPacket {
            bm_request_type: 0x80,
            b_request: 6,
            w_value: w_value.to_le(),
            w_index: langid.to_le(),
            w_length: 4u16.to_le(),
        };
        self.run_control_transfer(
            addr,
            low_speed,
            setup_hdr,
            true,
            Some(&mut hdr),
            4,
            mps0,
            500,
        )?;
        let len = hdr[0] as usize;
        if len < 2 || len > 255 {
            return Err(UhciError::UsbError(len as u32));
        }

        let mut buf = PhysBuf::new(len);
        buf.fill(0);
        let setup_full = SetupPacket {
            bm_request_type: 0x80,
            b_request: 6,
            w_value: w_value.to_le(),
            w_index: langid.to_le(),
            w_length: (len as u16).to_le(),
        };
        self.run_control_transfer(
            addr,
            low_speed,
            setup_full,
            true,
            Some(&mut buf),
            len,
            mps0,
            700,
        )?;

        let mut s = String::new();
        let mut i = 2usize;
        while i + 1 < len {
            let ch = u16::from_le_bytes([buf[i], buf[i + 1]]) as u32;
            if let Some(c) = core::char::from_u32(ch) {
                if c != '\u{0}' {
                    s.push(c);
                }
            }
            i += 2;
        }
        Ok(s)
    }

    // ... keep existing get_string_descriptor ...

    fn enumerate_device_on_port(
        &mut self,
        port: u8,
        low_speed: bool,
    ) -> Result<UhciDevInfo, UhciError> {
        // ... (Keep existing enumeration preamble: reset, address, descriptors) ...
        sleep_ms(100);

        let mut prefix = None;
        for _ in 0..3 {
            match self.get_device_descriptor_prefix(low_speed) {
                Ok(v) => {
                    prefix = Some(v);
                    break;
                }
                Err(UhciError::Timeout) => {
                    sleep_ms(50);
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        let (mps0, _prefix) = prefix.ok_or(UhciError::Timeout)?;
        let mps0 = core::cmp::max(8usize, mps0 as usize);

        let addr = core::cmp::min(127, 8 + port) as u8;
        let mut set_addr_ok = false;
        for _ in 0..3 {
            match self.set_address(low_speed, addr) {
                Ok(()) => {
                    set_addr_ok = true;
                    break;
                }
                Err(UhciError::Timeout) => {
                    sleep_ms(50);
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        if !set_addr_ok {
            return Err(UhciError::Timeout);
        }

        let mut desc = None;
        for _ in 0..3 {
            match self.get_device_descriptor_full(addr, low_speed, mps0) {
                Ok(v) => {
                    desc = Some(v);
                    break;
                }
                Err(UhciError::Timeout) => {
                    sleep_ms(50);
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        let desc = desc.ok_or(UhciError::Timeout)?;
        let vid = u16::from_le_bytes([desc[8], desc[9]]);
        let pid = u16::from_le_bytes([desc[10], desc[11]]);
        let class = desc[4];
        let subclass = desc[5];
        let protocol = desc[6];
        let max_packet0 = desc[7];

        let kind = self.detect_device_kind(addr, low_speed, mps0, class, subclass, protocol);

        let name = {
            let i_mfg = desc[14];
            let i_prod = desc[15];
            let mut resolved_name = None;
            if i_mfg != 0 || i_prod != 0 {
                let langid_res = self.get_langid(addr, low_speed, mps0);
                if let Ok(langid) = langid_res {
                    let mfg = self
                        .get_string_descriptor(addr, low_speed, mps0, i_mfg, langid)
                        .ok();
                    let prod = self
                        .get_string_descriptor(addr, low_speed, mps0, i_prod, langid)
                        .ok();
                    match (mfg, prod) {
                        (Some(m), Some(p)) => resolved_name = Some(format!("{} {}", m, p)),
                        (Some(m), None) => resolved_name = Some(m),
                        (None, Some(p)) => resolved_name = Some(p),
                        (None, None) => {}
                    }
                }
            }
            resolved_name.unwrap_or_else(|| {
                if let Some((vendor, product)) = usb_ids::lookup(vid, pid) {
                    format!("{} {}", vendor, product)
                } else {
                    format!("Unknown {:04x}:{:04x}", vid, pid)
                }
            })
        };

        // DRIVER INITIALIZATION
        let mut usb_dev = UsbDevice {
            port,
            addr,
            vid,
            pid,
            class,
            subclass,
            protocol,
            max_packet0,
            kind,
            name: name.clone(),
            low_speed,
            // Optional fields removed or set to dummy values since driver handles them
            interface: 0,
            int_in_ep: 0,
            int_in_mps: 0,
            int_in_interval: 0,
        };

        if let Some(mut driver) = crate::driver::usb::manager::get_driver(&usb_dev) {
            log!("UHCI: Initializing driver for {}", name);
            if let Err(e) = driver.init(&mut usb_dev, self) {
                log!("UHCI: Driver init failed: {:?}", e);
            } else {
                log!("UHCI: Driver active!");
                self.drivers.push(driver);
            }
        } else {
            log!("UHCI: No driver found for {}", name);
        }

        Ok(UhciDevInfo {
            port,
            addr,
            vid,
            pid,
            class,
            subclass,
            protocol,
            max_packet0,
            kind,
            name,
        })
    }
    pub fn poll_drivers(&mut self) {
        for driver in self.drivers.iter_mut() {
            driver.poll();
        }
    }
}

// Implement HostController trait for UHCI
impl HostController for UHci {
    fn control_transfer(
        &mut self,
        addr: u8,
        _endp: u8, // UHCI run_control_transfer assumes endpoint 0 for control
        setup: [u8; 8],
        data: Option<&mut [u8]>,
        low_speed: bool, // Passed from driver
    ) -> Result<usize, UsbError> {
        let setup_pkt = unsafe { core::ptr::read(setup.as_ptr() as *const SetupPacket) };
        let len = data.as_ref().map(|d| d.len()).unwrap_or(0);

        // We need a PhysBuf for data if present.
        let mut bounce_buf = if len > 0 {
            Some(PhysBuf::new(len))
        } else {
            None
        };

        if let Some(ref mut b) = bounce_buf {
            if let Some(src) = data.as_ref() {
                b.copy_from_slice(src);
            }
        }

        let res = self.run_control_transfer(
            addr,
            low_speed,
            setup_pkt,
            (setup[0] & 0x80) != 0,
            bounce_buf.as_mut(),
            len,
            8,
            500,
        );

        match res {
            Ok(_) => {
                if (setup[0] & 0x80) != 0 {
                    if let Some(src) = bounce_buf {
                        if let Some(dst) = data {
                            dst.copy_from_slice(&src[..len]);
                        }
                    }
                }
                Ok(len)
            }
            Err(e) => Err(match e {
                UhciError::Timeout => UsbError::Timeout,
                UhciError::Stalled => UsbError::Stalled,
                UhciError::Halted => UsbError::Halted,
                UhciError::UsbError(u) => UsbError::UsbError(u),
            }),
        }
    }

    fn schedule_interrupt(
        &mut self,
        addr: u8,
        endp: u8,
        _max_packet_size: u8,
        _interval: u8,
        buf_phys: u64,
        len: usize,
        low_speed: bool,
    ) -> Result<Box<dyn InterruptTransfer>, UsbError> {
        // Create QH + TD
        // QH: Head -> Next QH (Async or Next Int)
        //     Elem -> TD
        // TD: -> Terminate (or next TD if multi-stage, but interrupt is single)

        let mem = PhysBuf::new(32); // 16 bytes QH + 16 bytes TD (aligned 16)
        let base_phys = mem.addr() as u32;
        let qh_phys = base_phys;
        let td_phys = base_phys + 16;

        let ptr = mem.virt_addr().as_u64() as *mut u32;
        let qh_ptr = ptr as *mut UhciQH;
        let td_ptr = unsafe { ptr.add(4) } as *mut UhciTD;

        // Link into schedule: Insert at HEAD of interrupt chain
        // Existing chain head: self.interrupt_root (phys)
        // New QH Head Link -> self.interrupt_root
        // self.interrupt_root -> New QH

        let entry_link = qh_phys | LINK_QH | LINK_DEPTH_FIRST;

        // Setup TD
        let td = UHci::make_td(
            LINK_TERMINATE,
            PID_IN,
            addr,
            endp,
            false, // Data0/1? usually toggle maintained? Interrupt keeps toggle?
            // UHCI spec: Driver maintains toggle or controller?
            // Controller uses QH overlay for toggle bit 1 in dword 2?
            // For now: data0
            len,
            buf_phys as u32,
            low_speed,
            true, // IOC
            true, // SPD (short packets are normal for HID reports)
        );

        unsafe {
            qh_ptr.write(UhciQH {
                head_link: self.interrupt_root | LINK_DEPTH_FIRST,
                element_link: td_phys | LINK_DEPTH_FIRST,
            });
            td_ptr.write(td);
        }

        // Update Root
        // We need to update ALL frame list entries to point to new head?
        // Yes, if we want 1ms polling (interval 1).
        // Since interval argument is unused in this simple impl, we set interval 1.
        self.interrupt_root = entry_link;
        // Optimization: only update frame list once at init?
        // Currently `init_schedule` sets all frames to `frame_ptr` (async).
        // We should update `frame_list` to point to `interrupt_root`?
        // BUT `interrupt_root` inside `UHci` struct is a software concept.
        // We need to write to the actual frame list in memory.
        let frame_list_ptr = self.frame_list_mut_u32();
        for entry in frame_list_ptr.iter_mut() {
            *entry = entry_link;
        }

        Ok(Box::new(UhciInterruptTransfer {
            mem, // Keeps QH/TD alive
            qh_ptr,
            td_ptr,
            td_phys,
            data_toggle: false,
            last_poll_had_error: false,
        }))
    }
}

struct UhciInterruptTransfer {
    mem: PhysBuf,
    qh_ptr: *mut UhciQH,
    td_ptr: *mut UhciTD,
    td_phys: u32,
    data_toggle: bool,
    last_poll_had_error: bool,
}

impl InterruptTransfer for UhciInterruptTransfer {
    fn poll(&mut self) -> bool {
        let td = unsafe { self.td_ptr.read_volatile() };
        // If Active bit (Bit 23) is 0, transfer complete.
        if (td.ctrl_status & TD_STATUS_ACTIVE) != 0 {
            return false;
        }

        let hard_errors = td.ctrl_status
            & (TD_STATUS_STALLED
                | TD_STATUS_BITSTUFF
                | TD_STATUS_CRC_TIMEOUT
                | TD_STATUS_BABBLE
                | TD_STATUS_DATABUF);
        self.last_poll_had_error = hard_errors != 0;
        if self.last_poll_had_error {
            log!("UHCI: interrupt TD error status={:#x}", td.ctrl_status);
        }
        true
    }

    fn ack(&mut self) {
        // UHCI advances the QH element link ("overlay") as TDs complete.
        // If we reuse the same TD, we must repoint the QH back to it.
        unsafe {
            let mut qh = self.qh_ptr.read_volatile();
            qh.element_link = self.td_phys | LINK_DEPTH_FIRST;
            self.qh_ptr.write_volatile(qh);
        }

        // Re-arm: Set Active bit, reset length?
        // Max Length is in bits 21-31 of token? No, token is separate.
        // Status has bits 0-10 ActLen.
        // We need to reset Status to Initial Status (Active | 3 Errors)
        // Token remains comparable.
        let mut td = unsafe { self.td_ptr.read_volatile() };
        let keep = td.ctrl_status & (TD_STATUS_LS | TD_STATUS_SPD);
        td.ctrl_status = TD_STATUS_ACTLEN_MASK
            | TD_STATUS_ACTIVE
            | (3u32 << TD_STATUS_ERRCNT_SHIFT)
            | TD_STATUS_IOC
            | keep;
        td.token = (td.token & !TD_TOKEN_TOGGLE)
            | if self.data_toggle { TD_TOKEN_TOGGLE } else { 0 };
        // Keep buffer pointer intact and reprogram token toggle from software state.
        if !self.last_poll_had_error {
            self.data_toggle = !self.data_toggle;
        }
        self.last_poll_had_error = false;
        unsafe {
            self.td_ptr.write_volatile(td);
        }
    }
}

#[derive(Debug, Clone)]
struct UhciDevInfo {
    port: u8,
    addr: u8,
    vid: u16,
    pid: u16,
    class: u8,
    subclass: u8,
    protocol: u8,
    max_packet0: u8,
    kind: UsbDeviceKind,
    name: String,
}
