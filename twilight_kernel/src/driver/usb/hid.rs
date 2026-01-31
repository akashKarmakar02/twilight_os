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
}

impl UsbDriver for MouseDriver {
    fn init(
        &mut self,
        device: &mut UsbDevice,
        hc: &mut dyn HostController,
    ) -> Result<(), UsbError> {
        if device.int_in_ep == 0 || device.int_in_mps == 0 {
            return Err(UsbError::InvalidDevice);
        }

        // 1. Set Protocol 0 (Boot Protocol)
        // Request: 0x21 (0b00100001) - Set Protocol
        // Value: 0 (Boot)
        // Index: 0 (Interface)
        let mut setup = [0u8; 8];
        setup[0] = 0x21; // bmRequestType: Host to Device, Class, Interface
        setup[1] = 0x0B; // bRequest: SET_PROTOCOL
        setup[2] = 0x00; // wValue: 0 (Boot)
        setup[3] = 0x00;
        setup[4] = device.interface; // wIndex: Interface
        setup[5] = 0x00;
        setup[6] = 0x00; // wLength: 0
        setup[7] = 0x00;

        hc.control_transfer(device.addr, 0, setup, None, device.low_speed)?;

        // 2. Set Idle 0 (Duration Indefinite / Report only on change)
        setup[0] = 0x21;
        setup[1] = 0x0A; // SET_IDLE
        setup[2] = 0x00; // Duration: 0 (upper byte), ReportID: 0 (lower byte)
        setup[3] = 0x00;
        setup[4] = device.interface;
        setup[5] = 0x00;
        setup[6] = 0x00;
        setup[7] = 0x00;
        hc.control_transfer(device.addr, 0, setup, None, device.low_speed)?;

        // 3. Allocate buffer for Interrupt Transfers
        // Use endpoint max packet size (cap to a small buffer).
        let buf_len = core::cmp::min(64usize, core::cmp::max(1usize, device.int_in_mps as usize));
        let buf = PhysBuf::new(buf_len);
        let phys_addr = buf.addr();

        let interval = core::cmp::max(1u8, device.int_in_interval);
        let handle = hc.schedule_interrupt(
            device.addr,
            device.int_in_ep,
            device.int_in_mps,
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
