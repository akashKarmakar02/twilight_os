use crate::sys::pci::DeviceConfig;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceId {
    pub bus: u8,
    pub slot: u8,
    pub function: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceClass {
    Pci,
}

#[derive(Clone, Copy, Debug)]
pub struct DeviceSoftc(*mut ());

impl DeviceSoftc {
    pub const fn null() -> Self {
        Self(core::ptr::null_mut())
    }

    pub const fn from_ptr(ptr: *mut ()) -> Self {
        Self(ptr)
    }

    pub fn as_ptr<T>(self) -> *mut T {
        self.0.cast()
    }
}

pub struct Device {
    id: DeviceId,
    class: DeviceClass,
    nameunit: &'static str,
    desc: Option<&'static str>,
    softc: DeviceSoftc,
    pci_config: DeviceConfig,
}

impl Device {
    pub fn from_pci_config(pci_config: DeviceConfig) -> Self {
        Self {
            id: DeviceId {
                bus: pci_config.bus,
                slot: pci_config.device,
                function: pci_config.function,
            },
            class: DeviceClass::Pci,
            nameunit: "pci0",
            desc: None,
            softc: DeviceSoftc::null(),
            pci_config,
        }
    }

    pub fn id(&self) -> DeviceId {
        self.id
    }

    pub fn class(&self) -> DeviceClass {
        self.class
    }

    pub fn desc(&self) -> Option<&'static str> {
        self.desc
    }

    pub fn pci_config(&self) -> &DeviceConfig {
        &self.pci_config
    }

    pub fn set_softc(&mut self, softc: DeviceSoftc) {
        self.softc = softc;
    }
}

pub fn device_get_nameunit(device: &Device) -> &'static str {
    device.nameunit
}

pub fn device_get_vendor(device: &Device) -> u16 {
    device.pci_config.vendor_id
}

pub fn device_get_device(device: &Device) -> u16 {
    device.pci_config.device_id
}

pub fn device_get_softc<T>(device: &Device) -> *mut T {
    device.softc.as_ptr()
}

pub fn device_set_desc(device: &mut Device, desc: &'static str) {
    device.desc = Some(desc);
}
