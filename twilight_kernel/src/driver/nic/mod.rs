mod pcnet;
mod rtl8139;

use crate::driver::timer::cmos::CMOS;
use crate::sys::pci::DeviceConfig;
use crate::{log, sys};
use alloc::format;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64};
use smoltcp::iface::Interface;
use smoltcp::phy::DeviceCapabilities;
use smoltcp::time::Instant;
use smoltcp::wire::EthernetAddress;
use spin::Mutex;

pub static NET: Mutex<Option<(Interface, EthernetDevice)>> = Mutex::new(None);

#[repr(u8)]
pub enum SocketStatus {
    IsListening = 0,
    IsActive = 1,
    IsOpen = 2,
    CanSend = 3,
    MaySend = 4,
    CanRecv = 5,
    MayRecv = 6,
}

#[derive(Clone)]
pub enum EthernetDevice {
    RTL8139(rtl8139::Device),
    PCNET(pcnet::Device),
}

pub trait EthernetDeviceIO {
    fn config(&self) -> Arc<Config>;
    fn stats(&self) -> Arc<Stats>;
    fn receive_packet(&mut self) -> Option<Vec<u8>>;
    fn transmit_packet(&mut self, len: usize);
    fn next_tx_buffer(&mut self, len: usize) -> &mut [u8];
}

impl EthernetDeviceIO for EthernetDevice {
    fn config(&self) -> Arc<Config> {
        match self {
            EthernetDevice::RTL8139(dev) => dev.config(),
            EthernetDevice::PCNET(dev) => dev.config(),
        }
    }
    fn stats(&self) -> Arc<Stats> {
        match self {
            EthernetDevice::RTL8139(dev) => dev.stats(),
            EthernetDevice::PCNET(dev) => dev.stats(),
        }
    }
    fn receive_packet(&mut self) -> Option<Vec<u8>> {
        match self {
            EthernetDevice::RTL8139(dev) => dev.receive_packet(),
            EthernetDevice::PCNET(dev) => dev.receive_packet(),
        }
    }

    fn transmit_packet(&mut self, len: usize) {
        match self {
            EthernetDevice::RTL8139(dev) => dev.transmit_packet(len),
            EthernetDevice::PCNET(dev) => dev.transmit_packet(len),
        }
    }

    fn next_tx_buffer(&mut self, len: usize) -> &mut [u8] {
        match self {
            EthernetDevice::RTL8139(dev) => dev.next_tx_buffer(len),
            EthernetDevice::PCNET(dev) => dev.next_tx_buffer(len),
        }
    }
}

impl<'a> smoltcp::phy::Device for EthernetDevice {
    type RxToken<'b>
        = RxToken
    where
        Self: 'b;
    type TxToken<'b>
        = TxToken
    where
        Self: 'b;

    fn receive(&mut self, _instant: Instant) -> Option<(Self::RxToken<'a>, Self::TxToken<'a>)> {
        if let Some(buffer) = self.receive_packet() {
            if self.config().is_debug_enabled() {
                // usr::hex::print_hex(&buffer);
            }
            self.stats().rx_add(buffer.len() as u64);
            let rx = RxToken { buffer };
            let tx = TxToken {
                device: self.clone(),
            };
            Some((rx, tx))
        } else {
            None
        }
    }

    fn transmit(&mut self, _instant: Instant) -> Option<Self::TxToken<'a>> {
        let tx = TxToken {
            device: self.clone(),
        };
        Some(tx)
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1500;
        caps.max_burst_size = Some(64);
        caps
    }
}

fn time() -> Instant {
    let mut cmos = CMOS::new();
    Instant::from_micros((cmos.unix_time() * 1000000) as i64)
}

/// Configuration for an Ethernet device.
///
/// - `debug`: enables or disables debug logging at runtime.
/// - `mac`: stores the configured MAC address of the Ethernet device.
pub struct Config {
    /// Whether debug mode is enabled.
    debug: AtomicBool,

    /// The current MAC address of the device (wrapped in a Mutex for safe mutation).
    mac: Mutex<Option<EthernetAddress>>,
}

#[allow(dead_code)]
impl Config {
    fn new() -> Self {
        Self {
            debug: AtomicBool::new(false),
            mac: Mutex::new(None),
        }
    }

    fn is_debug_enabled(&self) -> bool {
        true
        // self.debug.load(core::sync::atomic::Ordering::Relaxed)
    }

    pub fn enable_debug(&self) {
        self.debug
            .store(true, core::sync::atomic::Ordering::Relaxed);
    }

    pub fn disable_debug(&self) {
        self.debug
            .store(false, core::sync::atomic::Ordering::Relaxed);
    }

    pub fn mac(&self) -> Option<EthernetAddress> {
        *self.mac.lock()
    }

    fn update_mac(&self, mac: EthernetAddress) {
        *self.mac.lock() = Some(mac);
    }
}

/// Statistics counters for an Ethernet device.
///
/// Tracks packet and byte counts for both received and transmitted traffic.
pub struct Stats {
    /// Total received bytes count.
    rx_bytes_count: AtomicU64,

    /// Total transmitted bytes count.
    tx_bytes_count: AtomicU64,

    /// Total received packets count.
    rx_packets_count: AtomicU64,

    /// Total transmitted packets count.
    tx_packets_count: AtomicU64,
}

#[allow(dead_code)]
impl Stats {
    fn new() -> Self {
        Self {
            rx_packets_count: AtomicU64::new(0),
            rx_bytes_count: AtomicU64::new(0),
            tx_packets_count: AtomicU64::new(0),
            tx_bytes_count: AtomicU64::new(0),
        }
    }

    pub fn rx_bytes_count(&self) -> u64 {
        self.rx_bytes_count
            .load(core::sync::atomic::Ordering::Relaxed)
    }

    pub fn tx_bytes_count(&self) -> u64 {
        self.tx_bytes_count
            .load(core::sync::atomic::Ordering::Relaxed)
    }

    pub fn rx_packets_count(&self) -> u64 {
        self.rx_packets_count
            .load(core::sync::atomic::Ordering::Relaxed)
    }

    pub fn tx_packets_count(&self) -> u64 {
        self.tx_packets_count
            .load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Increments the receive (RX) packet and byte counters.
    ///
    /// # Arguments
    ///
    /// * `bytes_count` - The number of bytes received in this packet.
    ///
    /// This will increment the total packet count by 1 and add the given
    /// number of bytes to the total received bytes counter.
    pub fn rx_add(&self, bytes_count: u64) {
        self.rx_packets_count
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        self.rx_bytes_count
            .fetch_add(bytes_count, core::sync::atomic::Ordering::Relaxed);
    }

    /// Increments the transmit (TX) packet and byte counters.
    ///
    /// # Arguments
    ///
    /// * `bytes_count` - The number of bytes transmitted in this packet.
    ///
    /// This will increment the total packet count by 1 and add the given
    /// number of bytes to the total transmitted bytes counter.
    pub fn tx_add(&self, bytes_count: u64) {
        self.tx_packets_count
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        self.tx_bytes_count
            .fetch_add(bytes_count, core::sync::atomic::Ordering::Relaxed);
    }
}

#[allow(dead_code)]
fn find_device(device_id: u16, vendor_id: u16) -> Option<DeviceConfig> {
    if let Some(mut dev) = sys::pci::find_device(device_id, vendor_id) {
        dev.enable_bus_mastering();
        return Some(dev);
    }

    None
}

#[doc(hidden)]
pub struct RxToken {
    buffer: Vec<u8>,
}

impl smoltcp::phy::RxToken for RxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.buffer)
    }
}

#[doc(hidden)]
pub struct TxToken {
    device: EthernetDevice,
}
impl smoltcp::phy::TxToken for TxToken {
    fn consume<R, F>(mut self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let config = self.device.config();
        let buf = self.device.next_tx_buffer(len);
        let res = f(buf);
        if config.is_debug_enabled() {
            // usr::hex::print_hex(buf);
        }
        self.device.transmit_packet(len);
        self.device.stats().tx_add(len as u64);
        res
    }
}

pub fn init() {
    let add = |mut device: EthernetDevice, name| {
        log!("NET DRV {}", name);
        if let Some(mac) = device.config().mac() {
            let addr = format!("{}", mac).to_uppercase();
            log!("NET MAC {}", addr);

            let config = smoltcp::iface::Config::new(mac.into());
            let iface = Interface::new(config, &mut device, time());

            *NET.lock() = Some((iface, device));
        }
    };

    if let Some(dev) = find_device(0x10EC, 0x8139) {
        let io = dev.io_base();
        let nic = rtl8139::Device::new(io);
        add(EthernetDevice::RTL8139(nic), "RTL8139");
    }
    if let Some(dev) = find_device(0x1022, 0x2000) {
        let io = dev.io_base();
        let nic = pcnet::Device::new(io);
        add(EthernetDevice::PCNET(nic), "PCNET");
    }
}
