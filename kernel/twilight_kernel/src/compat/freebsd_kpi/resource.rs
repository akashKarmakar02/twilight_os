use super::bus_space::{BusSpaceHandle, BusSpaceTag};
use super::device::Device;
use super::driver::EINVAL;

pub const SYS_RES_IRQ: u32 = 1;
pub const SYS_RES_MEMORY: u32 = 3;
pub const SYS_RES_IOPORT: u32 = 4;

pub const RF_ACTIVE: u32 = 0x0001;
pub const RF_SHAREABLE: u32 = 0x0002;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resource {
    pub kind: u32,
    pub rid: usize,
    pub start: u64,
    pub size: usize,
    pub flags: u32,
    pub bus_tag: BusSpaceTag,
    pub bus_handle: BusSpaceHandle,
}

pub fn bus_alloc_resource_any(
    device: &Device,
    kind: u32,
    rid: usize,
    flags: u32,
) -> Option<Resource> {
    match kind {
        SYS_RES_IOPORT => ioport_resource(device, rid, flags),
        SYS_RES_IRQ => irq_resource(device, rid, flags),
        SYS_RES_MEMORY => memory_resource(device, rid, flags),
        _ => None,
    }
}

pub fn bus_release_resource(_device: &Device, kind: u32, rid: usize, resource: Resource) -> i32 {
    if resource.kind == kind && resource.rid == rid {
        0
    } else {
        EINVAL
    }
}

pub fn rman_get_start(resource: Resource) -> u64 {
    resource.start
}

pub fn rman_get_end(resource: Resource) -> u64 {
    if resource.size == 0 {
        resource.start
    } else {
        resource.start + resource.size as u64 - 1
    }
}

pub fn rman_get_size(resource: Resource) -> usize {
    resource.size
}

pub fn rman_get_rid(resource: Resource) -> usize {
    resource.rid
}

pub fn rman_get_bustag(resource: Resource) -> BusSpaceTag {
    resource.bus_tag
}

pub fn rman_get_bushandle(resource: Resource) -> BusSpaceHandle {
    resource.bus_handle
}

fn ioport_resource(device: &Device, rid: usize, flags: u32) -> Option<Resource> {
    let bar = *device.pci_config().base_addresses.get(rid)?;
    if bar == 0 || (bar & 0x1) == 0 {
        return None;
    }

    let start = (bar & 0xFFFF_FFFC) as u64;
    Some(Resource {
        kind: SYS_RES_IOPORT,
        rid,
        start,
        size: 0,
        flags,
        bus_tag: BusSpaceTag::IoPort,
        bus_handle: BusSpaceHandle::new(start),
    })
}

fn memory_resource(device: &Device, rid: usize, flags: u32) -> Option<Resource> {
    let bars = &device.pci_config().base_addresses;
    let bar = *bars.get(rid)?;
    if bar == 0 || (bar & 0x1) != 0 {
        return None;
    }

    let start = match (bar >> 1) & 0x3 {
        0x0 => (bar & 0xFFFF_FFF0) as u64,
        0x2 => {
            let high = *bars.get(rid + 1)? as u64;
            ((bar & 0xFFFF_FFF0) as u64) | (high << 32)
        }
        _ => return None,
    };

    Some(Resource {
        kind: SYS_RES_MEMORY,
        rid,
        start,
        size: 0,
        flags,
        bus_tag: BusSpaceTag::MmioUnsupported,
        bus_handle: BusSpaceHandle::new(start),
    })
}

fn irq_resource(device: &Device, rid: usize, flags: u32) -> Option<Resource> {
    let irq = device.pci_config().interrupt_line;
    if irq == 0xFF {
        return None;
    }

    Some(Resource {
        kind: SYS_RES_IRQ,
        rid,
        start: irq as u64,
        size: 1,
        flags,
        bus_tag: BusSpaceTag::IoPort,
        bus_handle: BusSpaceHandle::new(irq as u64),
    })
}
