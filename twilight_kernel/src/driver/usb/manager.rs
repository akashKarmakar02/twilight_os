use crate::driver::usb::hid::MouseDriver;
use crate::driver::usb::interfaces::{UsbDevice, UsbDeviceKind, UsbDriver};
use crate::driver::usb::keyboard::KeyboardDriver;
use alloc::boxed::Box;

pub fn get_driver(device: &UsbDevice) -> Option<Box<dyn UsbDriver>> {
    match device.kind {
        UsbDeviceKind::Mouse => Some(Box::new(MouseDriver::new())),
        UsbDeviceKind::Keyboard => Some(Box::new(KeyboardDriver::new())),
        _ => None,
    }
}
