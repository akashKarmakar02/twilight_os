use crate::driver::keyboard::keyboard_interrupt;
use crate::driver::usb::interfaces::{
    HostController, InterruptTransfer, UsbDevice, UsbDriver, UsbError,
};
use crate::sys::memory::phys::PhysBuf;
use alloc::boxed::Box;
// use alloc::vec::Vec;

pub struct KeyboardDriver {
    data_buf: Option<PhysBuf>,
    interrupt_handle: Option<Box<dyn InterruptTransfer>>,
    last_report: [u8; 8],
}

impl KeyboardDriver {
    pub const fn new() -> Self {
        Self {
            data_buf: None,
            interrupt_handle: None,
            last_report: [0u8; 8],
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
        let mut current_if: Option<(u8, u8, u8, u8)> = None;

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
                    // HID (3), Boot (1), Protocol (1 for Keyboard)
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

impl UsbDriver for KeyboardDriver {
    fn init(
        &mut self,
        device: &mut UsbDevice,
        hc: &mut dyn HostController,
    ) -> Result<(), UsbError> {
        // 1. Get Config Descriptor
        let mut cfg_hdr = [0u8; 9];
        // GET_DESCRIPTOR(Configuration, 0, 0, 9)
        let mut setup = [0x80, 0x06, 0x00, 0x02, 0x00, 0x00, 0x09, 0x00];

        match hc.control_transfer(device.addr, 0, setup, Some(&mut cfg_hdr), device.low_speed) {
            Ok(9) => {}
            _ => return Err(UsbError::UsbError(0)),
        }

        let total_len = u16::from_le_bytes([cfg_hdr[2], cfg_hdr[3]]) as usize;
        let mut cfg_buf = alloc::vec![0u8; total_len];
        setup[6] = total_len as u8;
        setup[7] = (total_len >> 8) as u8;

        match hc.control_transfer(device.addr, 0, setup, Some(&mut cfg_buf), device.low_speed) {
            Ok(n) if n == total_len => {}
            _ => return Err(UsbError::UsbError(0)),
        }

        // Parse for Boot Keyboard (Protocol 1)
        let (config_value, interface, ep_num, ep_mps, ep_interval) =
            Self::parse_boot_hid_int_in_endpoint(&cfg_buf, 0x01).ok_or(UsbError::InvalidDevice)?;

        // 2. Set Configuration
        let cfg_val_to_set = if config_value == 0 { 1 } else { config_value };
        setup = [0x00, 0x09, cfg_val_to_set, 0x00, 0x00, 0x00, 0x00, 0x00];
        hc.control_transfer(device.addr, 0, setup, None, device.low_speed)?;

        // 3. Set Protocol 0 (Boot Protocol)
        setup = [0x21, 0x0B, 0x00, 0x00, interface, 0x00, 0x00, 0x00];
        hc.control_transfer(device.addr, 0, setup, None, device.low_speed)?;

        // 4. Set Idle 0 (Duration=0 means infinite until change)
        setup = [0x21, 0x0A, 0x00, 0x00, interface, 0x00, 0x00, 0x00];
        hc.control_transfer(device.addr, 0, setup, None, device.low_speed)?;

        // 5. Schedule Interrupt
        let buf_len = core::cmp::min(64usize, core::cmp::max(8usize, ep_mps as usize));
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
                if let Some(buf) = self.data_buf.as_ref() {
                    // USB Boot Keyboard Report:
                    // Byte 0: Modifiers
                    // Byte 1: Reserved
                    // Byte 2..7: Keycodes (Usage ID)

                    let modifiers = buf[0];
                    let keys = &buf[2..8];

                    // Handle Modifiers
                    let old_modifiers = self.last_report[0];
                    let mod_diff = modifiers ^ old_modifiers;

                    if mod_diff != 0 {
                        // Bit 0: LCtrl (0xE0, 0x14 - Set 1?? No, just mapping)
                        // PS/2 Set 1 Scancodes for modifiers:
                        // LCtrl: 0x1D
                        // LShift: 0x2A
                        // LAlt: 0x38
                        // LGUI: 0xE0 0x5B (Scancode Set 1 Extended) -> Complex, let's stick to basics
                        // RCtrl: 0xE0 0x1D
                        // RShift: 0x36
                        // RAlt: 0xE0 0x38
                        // RGUI: 0xE0 0x5C

                        // Helper to send make/break
                        let send_mod = |bit: u8, scancode: u8, is_ext: bool| {
                            if (mod_diff & (1 << bit)) != 0 {
                                let pressed = (modifiers & (1 << bit)) != 0;
                                if is_ext {
                                    keyboard_interrupt(0xE0);
                                }
                                if pressed {
                                    keyboard_interrupt(scancode);
                                } else {
                                    keyboard_interrupt(scancode | 0x80);
                                }
                            }
                        };

                        send_mod(0, 0x1D, false); // LCtrl
                        send_mod(1, 0x2A, false); // LShift
                        send_mod(2, 0x38, false); // LAlt
                        send_mod(3, 0x5B, true); // LGUI
                        send_mod(4, 0x1D, true); // RCtrl
                        send_mod(5, 0x36, false); // RShift
                        send_mod(6, 0x38, true); // RAlt
                        send_mod(7, 0x5C, true); // RGUI
                    }

                    // Handle Keys
                    // Naive O(N^2) diff is fine for N=6
                    let old_keys = &self.last_report[2..8];

                    // Check for Released Keys (in Old but not New)
                    for &k in old_keys.iter() {
                        if k != 0 && !keys.contains(&k) {
                            if let Some(sc) = hid_usage_to_scancode(k) {
                                // Extended keys check
                                if (0x49..=0x52).contains(&k) {
                                    keyboard_interrupt(0xE0);
                                }
                                // Break code: scancode + 0x80
                                keyboard_interrupt(sc | 0x80);
                            }
                        }
                    }

                    // Check for Pressed Keys (in New but not Old)
                    for &k in keys.iter() {
                        if k != 0 && (k == 42 || !old_keys.contains(&k)) {
                            if let Some(sc) = hid_usage_to_scancode(k) {
                                // Extended keys check
                                if (0x49..=0x52).contains(&k) {
                                    keyboard_interrupt(0xE0);
                                }
                                keyboard_interrupt(sc);
                            }
                        }
                    }

                    // Store report
                    // Only if it's not an error status (USB sends all 1s for error sometimes)
                    // Usage ID 01 is ErrorRollOver.
                    let is_err = keys.iter().any(|&k| k == 0x01);
                    if !is_err {
                        self.last_report.copy_from_slice(&buf[0..8]);
                    }
                }
                handle.ack();
            }
        }
    }
}

// Simple lookup table for common HID Usage IDs to PS/2 Scancode Set 1
fn hid_usage_to_scancode(usage: u8) -> Option<u8> {
    match usage {
        0x04 => Some(0x1E), // A
        0x05 => Some(0x30), // B
        0x06 => Some(0x2E), // C
        0x07 => Some(0x20), // D
        0x08 => Some(0x12), // E
        0x09 => Some(0x21), // F
        0x0A => Some(0x22), // G
        0x0B => Some(0x23), // H
        0x0C => Some(0x17), // I
        0x0D => Some(0x24), // J
        0x0E => Some(0x25), // K
        0x0F => Some(0x26), // L
        0x10 => Some(0x32), // M
        0x11 => Some(0x31), // N
        0x12 => Some(0x18), // O
        0x13 => Some(0x19), // P
        0x14 => Some(0x10), // Q
        0x15 => Some(0x13), // R
        0x16 => Some(0x1F), // S
        0x17 => Some(0x14), // T
        0x18 => Some(0x16), // U
        0x19 => Some(0x2F), // V
        0x1A => Some(0x11), // W
        0x1B => Some(0x2D), // X
        0x1C => Some(0x15), // Y
        0x1D => Some(0x2C), // Z
        0x1E => Some(0x02), // 1
        0x1F => Some(0x03), // 2
        0x20 => Some(0x04), // 3
        0x21 => Some(0x05), // 4
        0x22 => Some(0x06), // 5
        0x23 => Some(0x07), // 6
        0x24 => Some(0x08), // 7
        0x25 => Some(0x09), // 8
        0x26 => Some(0x0A), // 9
        0x27 => Some(0x0B), // 0
        0x28 => Some(0x1C), // Enter
        0x29 => Some(0x01), // Esc
        0x2A => Some(0x0E), // Backspace
        0x2B => Some(0x0F), // Tab
        0x2C => Some(0x39), // Space
        0x2D => Some(0x0C), // -
        0x2E => Some(0x0D), // =
        0x2F => Some(0x1A), // [
        0x30 => Some(0x1B), // ]
        0x31 => Some(0x2B), // \
        0x33 => Some(0x27), // ;
        0x34 => Some(0x28), // '
        0x35 => Some(0x29), // `
        0x36 => Some(0x33), // ,
        0x37 => Some(0x34), // .
        0x38 => Some(0x35), // /
        0x39 => Some(0x3A), // CapsLock
        0x3A => Some(0x3B), // F1
        0x3B => Some(0x3C), // F2
        0x3C => Some(0x3D), // F3
        0x3D => Some(0x3E), // F4
        0x3E => Some(0x3F), // F5
        0x3F => Some(0x40), // F6
        0x40 => Some(0x41), // F7
        0x41 => Some(0x42), // F8
        0x42 => Some(0x43), // F9
        0x43 => Some(0x44), // F10
        0x44 => Some(0x57), // F11
        0x45 => Some(0x58), // F12
        0x49 => Some(0x52), // Insert
        0x4A => Some(0x47), // Home
        0x4B => Some(0x49), // PageUp
        0x4C => Some(0x53), // Delete
        0x4D => Some(0x4F), // End
        0x4E => Some(0x51), // PageDown
        0x4F => Some(0x4D), // Right
        0x50 => Some(0x4B), // Left
        0x51 => Some(0x50), // Down
        0x52 => Some(0x48), // Up
        _ => None,
    }
}
