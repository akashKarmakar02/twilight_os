use alloc::boxed::Box;
use alloc::string::String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbError {
    Timeout,
    Halted,
    Stalled,
    InvalidDevice,
    UsbError(u32),
    NoMemory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbDeviceKind {
    Keyboard,
    Mouse,
    Hid,
    Hub,
    MassStorage,
    Communication,
    Video,
    Audio,
    Unknown,
}

impl UsbDeviceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            UsbDeviceKind::Keyboard => "keyboard",
            UsbDeviceKind::Mouse => "mouse",
            UsbDeviceKind::Hid => "hid",
            UsbDeviceKind::Hub => "hub",
            UsbDeviceKind::MassStorage => "mass-storage",
            UsbDeviceKind::Communication => "communication",
            UsbDeviceKind::Video => "video",
            UsbDeviceKind::Audio => "audio",
            UsbDeviceKind::Unknown => "unknown",
        }
    }
}

pub struct UsbDevice {
    pub port: u8,
    pub addr: u8,
    pub vid: u16,
    pub pid: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub max_packet0: u8,
    pub kind: UsbDeviceKind,
    pub name: String,
    pub low_speed: bool, // Added

    // Selected interface/endpoint info for simple drivers (best-effort).
    // For Boot HID mouse/keyboard we fill these from the config descriptor.
    pub interface: u8,
    pub int_in_ep: u8,
    pub int_in_mps: u8,
    pub int_in_interval: u8,
}

/// Abstract interface for a Host Controller (UHCI/XHCI) to allow drivers to perform transfers.
pub trait HostController {
    // Basic control transfer (setup + data + status)
    fn control_transfer(
        &mut self,
        addr: u8,
        endp: u8,
        setup: [u8; 8],
        data: Option<&mut [u8]>,
        low_speed: bool, // Added
    ) -> Result<usize, UsbError>; // Returns bytes transferred or error

    fn schedule_interrupt(
        &mut self,
        addr: u8,
        endp: u8,
        max_packet_size: u8,
        interval: u8,
        buf_phys: u64, // Physical address of the buffer
        len: usize,
        low_speed: bool, // Added
    ) -> Result<Box<dyn InterruptTransfer>, UsbError>;
}

pub trait InterruptTransfer {
    // Check if new data is available since last check
    fn poll(&mut self) -> bool;
    // Acknowledge data (if needed) and re-arm
    fn ack(&mut self);
}

/// The trait that ALL USB device drivers must implement.
pub trait UsbDriver {
    /// Initialize the driver.
    /// The HC (Host Controller) is passed to allow control transfers etc during setup.
    /// Returns true if the driver successfully claimed the device.
    // We might need to store the HC reference? No, usually HC owns Driver, so we can't store mutable ref to parent.
    // "init" does configuration. "poll" is called by HC, so "poll" can take HC as arg?
    // Or "poll" assumes the driver setup interrupts and just checks its own buffer?
    // YES: If we schedule an interrupt transfer, the HC hardware updates the buffer. The driver just checks the buffer.
    fn init(&mut self, device: &mut UsbDevice, hc: &mut dyn HostController)
    -> Result<(), UsbError>;

    /// Poll the device for updates (logic level).
    /// Typically checks the interrupt buffer managed by the handle returned from schedule_interrupt.
    fn poll(&mut self);
}
