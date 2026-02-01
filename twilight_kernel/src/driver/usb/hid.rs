use crate::driver::mouse::enqueue_packet;
use crate::driver::usb::interfaces::{
    HostController, InterruptTransfer, UsbDevice, UsbDriver, UsbError,
};
use crate::sys::memory::phys::PhysBuf;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicUsize, Ordering};

static MOUSE_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);

pub struct MouseDriver {
    // We need to keep the buffer alive so the HC can write to it via DMA
    data_buf: Option<PhysBuf>,
    interrupt_handle: Option<Box<dyn InterruptTransfer>>,
}

impl MouseDriver {
    pub const fn new() -> Self {
        Self {
            data_buf: None,
            interrupt_handle: None,
        }
    }

    fn parse_boot_hid_int_in_endpoint(
        cfg: &[u8],
        want_protocol: u8,
    ) -> Option<(u8, u8, u8, u8, u8)> {
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
                    // Check logic: We want HID (3), Boot (1), Protocol (want_protocol)
                    // Wait, XHCI code checked class/subclass/protocol.
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
}

impl UsbDriver for MouseDriver {
    fn init(
        &mut self,
        device: &mut UsbDevice,
        hc: &mut dyn HostController,
    ) -> Result<(), UsbError> {
        // 1. Get Configuration Descriptor to find endpoints
        // We assume index 0 for configuration (standard for boot devices)
        // First get header to find length
        // Note: For simplicity, we just fetch a reasonable size (e.g. 256 bytes) or do two fetches.
        // Let's try fetching 9 bytes first.
        let mut cfg_hdr = [0u8; 9];
        // GET_DESCRIPTOR(Configuration, 0, 0, 9)
        let mut setup = [0u8; 8];
        setup[0] = 0x80; // Dir=In
        setup[1] = 0x06; // GET_DESCRIPTOR
        setup[2] = 0x00; // Desc Index
        setup[3] = 0x02; // Desc Type (CONFIGURATION)
        setup[4] = 0x00;
        setup[5] = 0x00;
        setup[6] = 0x09;
        setup[7] = 0x00;

        match hc.control_transfer(device.addr, 0, setup, Some(&mut cfg_hdr), device.low_speed) {
            Ok(9) => {}
            _ => return Err(UsbError::UsbError(0)), // Failed to get header
        }

        let total_len = u16::from_le_bytes([cfg_hdr[2], cfg_hdr[3]]) as usize;
        let mut cfg_buf = alloc::vec![0u8; total_len];
        setup[6] = total_len as u8; // Lower byte
        setup[7] = (total_len >> 8) as u8; // Upper byte

        match hc.control_transfer(device.addr, 0, setup, Some(&mut cfg_buf), device.low_speed) {
            Ok(n) if n == total_len => {}
            _ => return Err(UsbError::UsbError(0)), // Failed to get full config
        }

        // Parse for Boot Mouse Interface (Class 3, Subclass 1, Protocol 2)
        // We move the parsing logic here.
        let (config_value, interface, ep_num, ep_mps, ep_interval) =
            Self::parse_boot_hid_int_in_endpoint(&cfg_buf, 0x02).ok_or(UsbError::InvalidDevice)?;

        // 2. Set Configuration
        let cfg_val_to_set = if config_value == 0 { 1 } else { config_value };
        // SET_CONFIGURATION
        setup = [0u8; 8];
        setup[0] = 0x00; // Dir=Out
        setup[1] = 0x09; // SET_CONFIGURATION
        setup[2] = cfg_val_to_set;
        setup[3] = 0x00;
        setup[4] = 0x00;
        setup[5] = 0x00;
        setup[6] = 0x00;
        setup[7] = 0x00;
        hc.control_transfer(device.addr, 0, setup, None, device.low_speed)?;

        // 3. Set Protocol 0 (Boot Protocol)
        setup[0] = 0x21; // bmRequestType: Class, Interface
        setup[1] = 0x0B; // SET_PROTOCOL
        setup[2] = 0x00; // Boot
        setup[3] = 0x00;
        setup[4] = interface;
        setup[5] = 0x00;
        setup[6] = 0x00;
        setup[7] = 0x00;
        hc.control_transfer(device.addr, 0, setup, None, device.low_speed)?;

        // 4. Set Idle 0
        setup[0] = 0x21;
        setup[1] = 0x0A; // SET_IDLE
        setup[2] = 0x00;
        setup[3] = 0x00;
        setup[4] = interface;
        setup[5] = 0x00;
        setup[6] = 0x00;
        setup[7] = 0x00;
        hc.control_transfer(device.addr, 0, setup, None, device.low_speed)?;

        // 5. Schedule Interrupt
        let buf_len = core::cmp::min(64usize, core::cmp::max(1usize, ep_mps as usize));
        let buf = PhysBuf::new(buf_len);
        let phys_addr = buf.addr();
        let interval = core::cmp::max(1u8, ep_interval);

        let handle = hc.schedule_interrupt(
            device.addr,
            ep_num,
            ep_mps,
            interval,
            phys_addr,
            buf_len,
            device.low_speed,
        )?;

        self.data_buf = Some(buf);
        self.interrupt_handle = Some(handle);

        Ok(())
    }

    fn poll(&mut self) {
        if let Some(handle) = self.interrupt_handle.as_mut() {
            if handle.poll() {
                // We have new data!
                if let Some(buf) = self.data_buf.as_ref() {
                    let n = MOUSE_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
                    // Log the first few, and then only when there's activity.
                    let b0 = buf.get(0).copied().unwrap_or(0);
                    let b1 = buf.get(1).copied().unwrap_or(0);
                    let b2 = buf.get(2).copied().unwrap_or(0);
                    let active = b0 != 0 || b1 != 0 || b2 != 0;
                    if n < 16 || active {
                        // log!(
                        //     "USB Mouse report: [{:02x} {:02x} {:02x} ...] (len={})",
                        //     b0,
                        //     b1,
                        //     b2,
                        //     buf.len()
                        // );
                    }

                    // Boot Protocol Report:
                    // Byte 0: Buttons (bit 0=L, 1=R, 2=M)
                    // Byte 1: X displacement (signed i8)
                    // Byte 2: Y displacement (signed i8)

                    let buttons = buf[0];
                    let dx = buf[1] as i8;
                    // USB HID Boot Mouse: Y+ is typically "up", while our PS/2 consumer expects
                    // Y+ to be "down" (screen coordinates). Invert Y to match existing PS/2 path.
                    let dy = -(buf[2] as i8);

                    // Convert to correct coordinate system?
                    // PS/2 Packet:
                    // Byte 0: [Yovfl, Xovfl, Ysign, Xsign, 1, Mid, Right, Left]
                    // Byte 1: X
                    // Byte 2: Y

                    // Our generic enqueue_packet expects generic flags or raw PS/2?
                    // enqueue_packet in driver/mouse/mod.rs takes [u8; 3] and pushes it.
                    // The Userspace TWC expects PS/2 format.
                    // We must synthesize a PS/2 packet from USB data.

                    let mut ps2_flags = 0x08; // Bit 3 is always 1
                    if (buttons & 0x01) != 0 {
                        ps2_flags |= 0x01;
                    } // Left
                    if (buttons & 0x02) != 0 {
                        ps2_flags |= 0x02;
                    } // Right
                    if (buttons & 0x04) != 0 {
                        ps2_flags |= 0x04;
                    } // Middle

                    // Careful: USB X/Y are standard 2's complement i8.
                    // PS/2 X/Y are 9-bit values with sign bit in flags?
                    // Actually, for standard 3-byte PS/2, they are 8-bit,
                    // but if overflow happens, overflow bits are set.
                    // Since USB i8 fits in PS/2 byte, we just copy.
                    // Sign bits in flags are for metadata, but often ignored by simple parsers
                    // OR required.

                    // Let's set sign bits correctly
                    if dx < 0 {
                        ps2_flags |= 0x10;
                    } // X Sign
                    if dy < 0 {
                        ps2_flags |= 0x20;
                    } // Y Sign

                    let ps2_packet = [ps2_flags, dx as u8, dy as u8];
                    enqueue_packet(ps2_packet);
                }
                handle.ack();
            }
        }
    }
}
