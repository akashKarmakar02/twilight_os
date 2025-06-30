use alloc::sync::Arc;
use alloc::vec::Vec;
use core::hint::spin_loop;
use core::sync::atomic::{fence, AtomicUsize, Ordering};
use smoltcp::wire::EthernetAddress;
use x86_64::instructions::port::Port;
use crate::driver::nic::{Config, EthernetDeviceIO, Stats};
use crate::sys::memory::phys::PhysBuf;

const RX_BUFFER_SIZE: usize = 0;
const MTU: usize = 1536;
const RX_BUFFER_PAD: usize = 16;
const RX_BUFFER_LEN: usize = 8192 << RX_BUFFER_SIZE;

const TX_BUFFER_LEN: usize = 2048;
const TX_BUFFER_COUNT: usize = 4;
const ROK: u16 = 0x01;

const CR_RST: u8 = 1 << 4;
const CR_RE: u8 = 1 << 3;
const CR_TE: u8 = 1 << 2;
const CR_BUFE: u8 = 1 << 0;

const RCR_WRAP: u32 = 1 << 7;
const RCR_RBLEN: u32 = (RX_BUFFER_LEN << 11) as u32;

const RCR_WARP: u32 = 1 << 7;

const RCR_AB: u32 = 1 << 3;
const RCR_AM: u32 = 1 << 2;
const RCR_APM: u32 = 1 << 1;
const RCR_AAP: u32 = 1 << 0;

const TCR_IFG: u32 = 3 << 24;

const IMR_TOK: u16 = 1 << 2; // Transmit OK Interrupt
const IMR_ROK: u16 = 1 << 0; // Receive OK Interrupt

const TCR_MXDMA1: u32 = 1 << 9;
const TCR_MXDMA2: u32 = 1 << 10;

const TOK: u32 = 1 << 15;
const OWN: u32 = 1 << 13;

#[derive(Clone)]
pub struct Ports {
    pub mac: [Port<u8>; 6],
    pub tx_cmds: [Port<u32>; TX_BUFFER_COUNT],
    pub tx_addrs: [Port<u32>; TX_BUFFER_COUNT],
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


#[derive(Clone)]
pub struct Device {
    config: Arc<Config>,
    stats: Arc<Stats>,
    ports: Ports,

    rx_buffer: PhysBuf,
    rx_offset: usize,
    tx_buffers: [PhysBuf; TX_BUFFER_COUNT],
    tx_id: Arc<AtomicUsize>,
}

impl Device {
    pub fn new(io_base: u16) -> Self {
        let mut device = Self {
            config: Arc::new(Config::new()),
            stats: Arc::new(Stats::new()),
            ports: Ports::new(io_base),

            // Add MTU to RX_BUFFER_LEN if RCR_WRAP is set
            rx_buffer: PhysBuf::new(RX_BUFFER_LEN + RX_BUFFER_PAD + MTU),

            rx_offset: 0,
            tx_buffers: [(); TX_BUFFER_COUNT].map(|_|
                PhysBuf::new(TX_BUFFER_LEN)
            ),

            // Before a transmission begin the id is incremented,
            // so the first transimission will start at 0.
            tx_id: Arc::new(AtomicUsize::new(TX_BUFFER_COUNT - 1)),
        };
        device.init();
        device
    }

    pub fn init(&mut self) {
        unsafe { self.ports.config1.write(0) }; // power on the device

        // Software reset
        unsafe {
            self.ports.cmd.write(CR_RST);
            fence(Ordering::SeqCst);
            while self.ports.cmd.read() & CR_RST != 0 {
                spin_loop();
            }
        }

        // Set interrupts
        unsafe { self.ports.imr.write(IMR_TOK | IMR_ROK) }

        unsafe { self.ports.cmd.write(CR_RE | CR_TE) }


        // Read MAC addr
        self.config.update_mac(EthernetAddress::from_bytes(&self.ports.mac()));


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

        let cba = unsafe { self.ports.cba.read() };

        // CAPR starts at 65520 and with the pad it overflows to 0
        let capr = unsafe { self.ports.capr.read() };
        let offset = ((capr as usize) + RX_BUFFER_PAD) % (1 << 16);

        let header = u16::from_le_bytes(
            self.rx_buffer[(offset + 0)..(offset + 2)].try_into().unwrap(),
        );

        if header & ROK != ROK {
            let capr = ((cba as usize) % RX_BUFFER_LEN) - RX_BUFFER_PAD;
            unsafe { self.ports.capr.write(capr as u16) }
            return None;
        }

        let n = u16::from_le_bytes(
            self.rx_buffer[(offset + 2)..(offset + 4)].try_into().unwrap()
        ) as usize;

        // Update buffer read pointer
        self.rx_offset = (offset + n + 4 + 3) & !3;
        let capr = (self.rx_offset % RX_BUFFER_LEN) - RX_BUFFER_PAD;
        unsafe { self.ports.capr.write(capr as u16) }

        Some(self.rx_buffer[(offset + 4)..(offset + n)].to_vec())
    }

    fn transmit_packet(&self, len: usize) {
        let tx_id = self.tx_id.load(Ordering::SeqCst);
        let mut cmd_port = self.ports.tx_cmds[tx_id].clone();
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
            cmd_port.write(0x1FFF & len as u32);
            fence(Ordering::SeqCst);

            while cmd_port.read() & OWN != OWN {
                spin_loop();
            }
            while cmd_port.read() & TOK != TOK {
                spin_loop();
            }
        }
    }

    fn next_tx_buffer(&mut self, len: usize) -> &mut [u8] {
        let tx_id = (self.tx_id.load(Ordering::SeqCst) + 1) % TX_BUFFER_COUNT;
        self.tx_id.store(tx_id, Ordering::SeqCst);
        &mut self.tx_buffers[tx_id][0..len]
    }
}