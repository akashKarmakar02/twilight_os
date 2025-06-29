use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64};
use smoltcp::iface::Interface;
use smoltcp::wire::EthernetAddress;
use spin::Mutex;
use crate::{println, sys};
use crate::driver::timer::pit::uptime;
use crate::sys::pci::DeviceConfig;

pub static NET: Mutex<Option<(Interface, EthernetDevice)>> = Mutex::new(None);

pub enum EthernetDevice {
    
} 

pub trait EthernetDeviceIO {
    fn config(&self) -> Arc<Config>;
    fn stats(&self) -> Arc<Stats>;
    fn receive_packet(&self) -> Option<Vec<u8>>;
    fn transmit_packet(&self, len: usize);
    fn next_tx_buffer(&mut self, len: usize) -> &mut [u8];
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
        self.debug.load(core::sync::atomic::Ordering::Relaxed)
    }
    
    pub fn enable_debug(&self) {
        self.debug.store(true, core::sync::atomic::Ordering::Relaxed);
    }
    
    pub fn disable_debug(&self) {
        self.debug.store(false, core::sync::atomic::Ordering::Relaxed);
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
        self.rx_bytes_count.load(core::sync::atomic::Ordering::Relaxed)
    }

    pub fn tx_bytes_count(&self) -> u64 {
        self.tx_bytes_count.load(core::sync::atomic::Ordering::Relaxed)
    }

    pub fn rx_packets_count(&self) -> u64 {
        self.rx_packets_count.load(core::sync::atomic::Ordering::Relaxed)
    }

    pub fn tx_packets_count(&self) -> u64 {
        self.tx_packets_count.load(core::sync::atomic::Ordering::Relaxed)
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
        self.rx_packets_count.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        self.rx_bytes_count.fetch_add(bytes_count, core::sync::atomic::Ordering::Relaxed);
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
        self.tx_packets_count.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        self.tx_bytes_count.fetch_add(bytes_count, core::sync::atomic::Ordering::Relaxed);
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

pub fn init() {
    let uptime = uptime();
    println!("\x1b[93m[{:.6}]\x1b[0m NET DEV INIT (unimplemented)", uptime);
}