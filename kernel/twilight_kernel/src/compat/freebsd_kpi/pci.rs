use super::device::Device;

pub fn pci_get_vendor(device: &Device) -> u16 {
    device.pci_config().vendor_id
}

pub fn pci_get_device(device: &Device) -> u16 {
    device.pci_config().device_id
}

pub fn pci_get_class(device: &Device) -> u8 {
    device.pci_config().class
}

pub fn pci_get_subclass(device: &Device) -> u8 {
    device.pci_config().subclass
}

pub fn pci_get_revid(device: &Device) -> u8 {
    device.pci_config().rev
}
