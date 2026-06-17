use crate::compat::freebsd_kpi::device::{
    Device, device_get_device, device_get_nameunit, device_get_vendor, device_set_desc,
};
use crate::compat::freebsd_kpi::driver::{BUS_PROBE_DEFAULT, ENXIO, FreeBsdPciDriver};
use crate::compat::freebsd_kpi::pci::{pci_get_device, pci_get_vendor};
use crate::sys::pci::{PciClaimError, PciOwner, PciOwnerKind};
use crate::{log, sys};

const RTL8139_VENDOR_ID: u16 = 0x10EC;
const RTL8139_DEVICE_ID: u16 = 0x8139;
const RTL8139_DESC: &str = "FreeBSD KPI demo RTL8139";
const FREEBSD_DEMO_CLAIM_RTL8139: bool = false;

struct Rtl8139DemoDriver;

impl FreeBsdPciDriver for Rtl8139DemoDriver {
    fn probe(&mut self, device: &mut Device) -> i32 {
        if pci_get_vendor(device) == RTL8139_VENDOR_ID
            && pci_get_device(device) == RTL8139_DEVICE_ID
        {
            device_set_desc(device, RTL8139_DESC);
            log!(
                "freebsd_kpi_demo: probe {} {:04x}:{:04x}",
                device_get_nameunit(device),
                device_get_vendor(device),
                device_get_device(device)
            );
            return BUS_PROBE_DEFAULT;
        }

        ENXIO
    }

    fn attach(&mut self, device: &mut Device) -> i32 {
        let id = device.id();
        log!(
            "freebsd_kpi_demo: attach {} {:02x}:{:02x}.{} {:04x}:{:04x} {}",
            device_get_nameunit(device),
            id.bus,
            id.slot,
            id.function,
            device_get_vendor(device),
            device_get_device(device),
            device.desc().unwrap_or("unknown device")
        );
        0
    }

    fn detach(&mut self, _device: &mut Device) -> i32 {
        0
    }
}

pub fn init() {
    let mut driver = Rtl8139DemoDriver;
    let mut matched = false;

    for pci_config in sys::pci::list() {
        let mut device = Device::from_pci_config(pci_config);
        if driver.probe(&mut device) == BUS_PROBE_DEFAULT {
            matched = true;
            if !FREEBSD_DEMO_CLAIM_RTL8139 {
                log!(
                    "freebsd_kpi_demo: probe-only, not claiming {} {:04x}:{:04x}",
                    device_get_nameunit(&device),
                    device_get_vendor(&device),
                    device_get_device(&device)
                );
                continue;
            }

            let id = device.id();
            let owner = PciOwner {
                kind: PciOwnerKind::FreeBsdKpiDriver,
                name: "freebsd_kpi_demo",
            };

            match sys::pci::claim_device(id.bus, id.slot, id.function, owner) {
                Ok(()) => {
                    log!(
                        "freebsd_kpi_demo: claimed {} {:04x}:{:04x}",
                        device_get_nameunit(&device),
                        device_get_vendor(&device),
                        device_get_device(&device)
                    );

                    let result = driver.attach(&mut device);
                    if result != 0 {
                        log!("freebsd_kpi_demo: attach failed with {}", result);
                    }
                }
                Err(PciClaimError::AlreadyClaimed(owner)) => {
                    log!(
                        "freebsd_kpi_demo: attach skipped: {} {:04x}:{:04x} already claimed by {}/{}",
                        device_get_nameunit(&device),
                        device_get_vendor(&device),
                        device_get_device(&device),
                        owner.kind.as_str(),
                        owner.name
                    );
                }
                Err(PciClaimError::NotFound) => {
                    log!(
                        "freebsd_kpi_demo: attach skipped: {} {:04x}:{:04x} disappeared before claim",
                        device_get_nameunit(&device),
                        device_get_vendor(&device),
                        device_get_device(&device)
                    );
                }
            }
        }
    }

    if !matched {
        log!("freebsd_kpi_demo: no matching RTL8139 device found");
    }
}
