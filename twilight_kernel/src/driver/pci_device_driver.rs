use alloc::string::String;
use alloc::sync::Arc;

use crate::sys::pci::DeviceConfig;

pub type IrqVector = u8;

/* =========================================================
   PCI Device Driver Trait
========================================================= */

pub trait PciDeviceDriver {
    /* -----------------------------------------------------
       Lifecycle hooks
    ----------------------------------------------------- */

    fn init_device(&mut self) -> bool;
    fn start_device(&mut self) -> bool;
    fn shutdown_device(&mut self) -> bool;

    /* -----------------------------------------------------
       Device attachment
    ----------------------------------------------------- */

    fn attach_device(&mut self, dev: Arc<DeviceConfig>, enable_bus_mastering: bool);
}

/* =========================================================
   Common PCI Driver State
========================================================= */

pub struct PciDeviceDriverState {
    name: String,
    pci_dev: Option<Arc<DeviceConfig>>,
    irq_vector: Option<u8>,
}

impl PciDeviceDriverState {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            pci_dev: None,
            irq_vector: None,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn pci_device(&self) -> Option<&Arc<DeviceConfig>> {
        self.pci_dev.as_ref()
    }

    pub fn irq_vector(&self) -> Option<IrqVector> {
        self.irq_vector
    }

    pub fn attach_device(&mut self, dev: Arc<DeviceConfig>, enable_bus_mastering: bool) {
        if enable_bus_mastering {
            dev.enable_bus_mastering();
        }

        self.irq_vector = Some(dev.interrupt_line);
        self.pci_dev = Some(dev);
    }
}
