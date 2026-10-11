//! RTL8139 DMA rings. Interrupts acknowledge status and wake task-context
//! service only; no packet allocation or protocol processing is IRQ-safe.

use crate::driver::nic::{Config, EthernetDeviceIO, Stats};
use crate::sys::memory::phys::PhysBuf;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::hint::spin_loop;
use core::sync::atomic::{AtomicU16, Ordering, fence};
use smoltcp::wire::EthernetAddress;
use x86_64::instructions::port::Port;

// 00 = 8K + 16 bytes
// 01 = 16K + 16 bytes
// 10 = 32K + 16 bytes
// 11 = 64K + 16 bytes
const RX_BUFFER_IDX: usize = 0;

const MTU: usize = 1536;

const RX_BUFFER_PAD: usize = 16;
const RX_BUFFER_LEN: usize = 8192 << RX_BUFFER_IDX;

const TX_BUFFER_LEN: usize = 2048;
const TX_BUFFERS_COUNT: usize = 4;
const ROK: u16 = 0x01;

const CR_RST: u8 = 1 << 4; // Reset
const CR_RE: u8 = 1 << 3; // Receiver Enable
const CR_TE: u8 = 1 << 2; // Transmitter Enable
const CR_BUFE: u8 = 1 << 0; // Buffer Empty

// Rx Buffer Length
const RCR_RBLEN: u32 = (RX_BUFFER_IDX << 11) as u32;

// When the WRAP bit is set, the nic will keep moving the rest
// of the packet data into the memory immediately after the
// end of the Rx buffer instead of going back to the begining
// of the buffer. So the buffer must have an additionnal 1500 bytes.
const RCR_WRAP: u32 = 1 << 7;

const RCR_AB: u32 = 1 << 3; // Accept Broadcast packets
const RCR_AM: u32 = 1 << 2; // Accept Multicast packets
const RCR_APM: u32 = 1 << 1; // Accept Physical Match packets
const RCR_AAP: u32 = 1 << 0; // Accept All Packets

// Interframe Gap Time
const TCR_IFG: u32 = 3 << 24;

// Max DMA Burst Size per Tx DMA Burst
// 000 = 16 bytes
// 001 = 32 bytes
// 010 = 64 bytes
// 011 = 128 bytes
// 100 = 256 bytes
// 101 = 512 bytes
// 110 = 1024 bytes
// 111 = 2048 bytes
//const TCR_MXDMA0: u32 = 1 << 8;
const TCR_MXDMA1: u32 = 1 << 9;
const TCR_MXDMA2: u32 = 1 << 10;

// Receive/transmit completion and error causes, plus RX ring overflow.
const INTERRUPTS: u16 = 0x005f;
static IRQ_IO_BASE: AtomicU16 = AtomicU16::new(0);

fn interrupt() {
    let base = IRQ_IO_BASE.load(Ordering::Acquire);
    if base == 0 {
        return;
    }
    // SAFETY: the PCI I/O BAR is published after the device has been initialized.
    // These dedicated status/mask ports do not share indexed-register state.
    unsafe {
        let mut status = Port::<u16>::new(base + 0x3e);
        let pending = status.read() & INTERRUPTS;
        if pending == 0 {
            return; // another device owns this shared INTx event
        }
        Port::<u16>::new(base + 0x3c).write(0);
        status.write(pending);
    }
    crate::sys::net::service::request();
}

//const CRS: u32 = 1 << 31; // Carrier Sense Lost
//const TAB: u32 = 1 << 30; // Transmit Abort
//const OWC: u32 = 1 << 29; // Out of Window Collision
//const CDH: u32 = 1 << 28; // CD Heart Beat
//const TUN: u32 = 1 << 14; // Transmit FIFO Underrun
const OWN: u32 = 1 << 13; // DMA operation completed

#[allow(dead_code)]
#[derive(Clone)]
pub struct Ports {
    pub mac: [Port<u8>; 6],
    pub tx_cmds: [Port<u32>; TX_BUFFERS_COUNT],
    pub tx_addrs: [Port<u32>; TX_BUFFERS_COUNT],
    pub config1: Port<u8>,
    pub rx_addr: Port<u32>,
    pub capr: Port<u16>,
    pub cba: Port<u16>,
    pub imr: Port<u16>,
    pub cmd: Port<u8>,
    pub tx_config: Port<u32>,
    pub rx_config: Port<u32>,
}

impl Ports {
    pub fn new(io_base: u16) -> Self {
        Self {
            mac: [
                Port::new(io_base + 0x00),
                Port::new(io_base + 0x01),
                Port::new(io_base + 0x02),
                Port::new(io_base + 0x03),
                Port::new(io_base + 0x04),
                Port::new(io_base + 0x05),
            ],
            tx_cmds: [
                Port::new(io_base + 0x10),
                Port::new(io_base + 0x14),
                Port::new(io_base + 0x18),
                Port::new(io_base + 0x1C),
            ],
            tx_addrs: [
                Port::new(io_base + 0x20),
                Port::new(io_base + 0x24),
                Port::new(io_base + 0x28),
                Port::new(io_base + 0x2C),
            ],
            config1: Port::new(io_base + 0x52),
            rx_addr: Port::new(io_base + 0x30),
            capr: Port::new(io_base + 0x38),
            cba: Port::new(io_base + 0x3A),
            cmd: Port::new(io_base + 0x37),
            imr: Port::new(io_base + 0x3C),
            //isr: Port::new(io_base + 0x3E),
            tx_config: Port::new(io_base + 0x40),
            rx_config: Port::new(io_base + 0x44),
        }
    }

    fn mac(&mut self) -> [u8; 6] {
        unsafe {
            [
                self.mac[0].read(),
                self.mac[1].read(),
                self.mac[2].read(),
                self.mac[3].read(),
                self.mac[4].read(),
                self.mac[5].read(),
            ]
        }
    }
}

pub struct Device {
    config: Arc<Config>,
    stats: Arc<Stats>,
    ports: Ports,

    rx_buffer: PhysBuf,
    rx_offset: usize,
    tx_buffers: [PhysBuf; TX_BUFFERS_COUNT],
    tx_id: usize,
    tx_inflight: [bool; TX_BUFFERS_COUNT],
    irq: u8,
    io_base: u16,
}

impl Device {
    pub fn new(io_base: u16, irq: u8) -> Self {
        let mut device = Self {
            config: Arc::new(Config::new()),
            stats: Arc::new(Stats::new()),
            ports: Ports::new(io_base),

            // Add MTU to RX_BUFFER_LEN if RCR_WRAP is set
            rx_buffer: {
                let buf = PhysBuf::new(RX_BUFFER_LEN + RX_BUFFER_PAD + MTU);
                buf
            },

            rx_offset: 0,
            tx_buffers: [(); TX_BUFFERS_COUNT].map(|_| PhysBuf::new(TX_BUFFER_LEN)),

            // Index of the next TX buffer to use.
            tx_id: 0,
            tx_inflight: [false; TX_BUFFERS_COUNT],
            irq,
            io_base,
        };
        device.init();
        device
    }

    pub fn init(&mut self) {
        // Power on
        unsafe { self.ports.config1.write(0) }

        // Software reset
        unsafe {
            self.ports.cmd.write(CR_RST);
            fence(Ordering::SeqCst);
            while self.ports.cmd.read() & CR_RST != 0 {
                spin_loop();
            }
        }

        // Boot DHCP uses task-context polling before the scheduler is ready.
        // The network service enables INTx only after publishing its waiter.
        unsafe { self.ports.imr.write(0) }

        // Enable Receive and Transmitter
        unsafe { self.ports.cmd.write(CR_RE | CR_TE) }

        // Read MAC addr
        self.config
            .update_mac(EthernetAddress::from_bytes(&self.ports.mac()));

        // Get physical address of rx_buffer
        let rx_addr = self.rx_buffer.addr();

        // Init Receive buffer
        unsafe { self.ports.rx_addr.write(rx_addr as u32) }

        for i in 0..4 {
            // Get physical address of each tx_buffer
            let tx_addr = self.tx_buffers[i].addr();

            // Init Transmit buffer
            unsafe { self.ports.tx_addrs[i].write(tx_addr as u32) }
        }

        // Configure receive buffer (RCR)
        let flags = RCR_RBLEN | RCR_WRAP | RCR_AB | RCR_AM | RCR_APM | RCR_AAP;
        unsafe { self.ports.rx_config.write(flags) }

        // Configure transmit buffer (TCR)
        let flags = TCR_IFG | TCR_MXDMA1 | TCR_MXDMA2;
        unsafe { self.ports.tx_config.write(flags) }
    }
    /// Register shared INTx once, then unmask completion/error notifications.
    pub fn enable_interrupts(&mut self) -> bool {
        if IRQ_IO_BASE.load(Ordering::Acquire) == 0 {
            if crate::arch::x86_64::idt::register_irq_handler(self.irq, interrupt).is_err() {
                return false;
            }
            IRQ_IO_BASE.store(self.io_base, Ordering::Release);
        }
        // SAFETY: device initialization owns and validated these PCI I/O ports.
        unsafe { self.ports.imr.write(INTERRUPTS) };
        true
    }

    /// Select a free descriptor without waiting for hardware under NET (#83).
    pub fn tx_available(&mut self) -> bool {
        for _ in 0..TX_BUFFERS_COUNT {
            if self.tx_inflight[self.tx_id] {
                // SAFETY: each TSD belongs to one of our four DMA buffers.
                let status = unsafe { self.ports.tx_cmds[self.tx_id].read() };
                self.tx_inflight[self.tx_id] = status & OWN != OWN;
            }
            if !self.tx_inflight[self.tx_id] {
                return true;
            }
            self.tx_id = (self.tx_id + 1) % TX_BUFFERS_COUNT;
        }
        false
    }
}

impl EthernetDeviceIO for Device {
    fn config(&self) -> Arc<Config> {
        self.config.clone()
    }

    fn stats(&self) -> Arc<Stats> {
        self.stats.clone()
    }

    fn receive_packet(&mut self) -> Option<Vec<u8>> {
        let cmd = unsafe { self.ports.cmd.read() };
        if (cmd & CR_BUFE) == CR_BUFE {
            return None;
        }
        // DMA completion must be observed before reading the receive buffer.
        fence(Ordering::Acquire);
        let offset = self.rx_offset;

        let header = u16::from_le_bytes([self.rx_buffer[offset], self.rx_buffer[offset + 1]]);

        if header & ROK != ROK {
            let cba = unsafe { self.ports.cba.read() };
            let next = (cba as usize) % RX_BUFFER_LEN;
            let capr = next.wrapping_sub(RX_BUFFER_PAD);
            unsafe { self.ports.capr.write(capr as u16) }
            self.rx_offset = next;
            return None;
        }

        let n =
            u16::from_le_bytes([self.rx_buffer[offset + 2], self.rx_buffer[offset + 3]]) as usize;

        // Copy before releasing this part of the DMA ring to the NIC. Updating
        // CAPR first permits a busy receiver to overwrite the frame mid-copy.
        let packet = self.rx_buffer[(offset + 4)..(offset + n)].to_vec();
        fence(Ordering::Release);
        let next = ((offset + n + 4 + 3) & !3) % RX_BUFFER_LEN;
        self.rx_offset = next;
        let capr = next.wrapping_sub(RX_BUFFER_PAD);
        unsafe { self.ports.capr.write(capr as u16) }

        Some(packet)
    }

    fn transmit_packet(&mut self, len: usize) {
        let tx_id = self.tx_id;
        unsafe {
            // RTL8139 will not transmit packets smaller than 64 bits
            let len = len.max(60); // 60 + 4 bits of CRC

            // Fill in Transmit Status: the size of this packet, the early
            // transmit threshold, and clear OWN bit in TSD (this starts the
            // PCI operation).
            // NOTE: The length of the packet use the first 13 bits (but should
            // not exceed 1792 bytes), and a value of 0x000000 for the early
            // transmit threshold means 8 bytes. So we just write the size of
            // the packet.
            // Publish the filled DMA buffer before ringing the transmit port.
            fence(Ordering::Release);
            self.ports.tx_cmds[tx_id].write(0x1FFF & len as u32);
        }

        self.tx_inflight[tx_id] = true;
        self.tx_id = (tx_id + 1) % TX_BUFFERS_COUNT;
    }

    fn next_tx_buffer(&mut self, len: usize) -> &mut [u8] {
        // Device::transmit/receive reserves this descriptor before creating the
        // token; the single CPU cannot change it before consume runs.
        assert!(
            !self.tx_inflight[self.tx_id],
            "TX token without free descriptor"
        );
        &mut self.tx_buffers[self.tx_id][0..len]
    }
}
