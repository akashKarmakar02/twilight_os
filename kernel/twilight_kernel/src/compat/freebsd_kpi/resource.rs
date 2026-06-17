use super::device::Device;

pub const SYS_RES_IRQ: u32 = 1;
pub const SYS_RES_MEMORY: u32 = 3;
pub const SYS_RES_IOPORT: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resource {
    pub kind: u32,
    pub rid: usize,
    pub start: u64,
    pub size: usize,
}

pub fn bus_alloc_resource_any(
    device: &Device,
    kind: u32,
    rid: usize,
    _flags: u32,
) -> Option<Resource> {
    match kind {
        SYS_RES_IOPORT => ioport_resource(device, rid),
        SYS_RES_IRQ => irq_resource(device, rid),
        SYS_RES_MEMORY => None,
        _ => None,
    }
}

fn ioport_resource(device: &Device, rid: usize) -> Option<Resource> {
    let bar = *device.pci_config().base_addresses.get(rid)?;
    if bar == 0 || (bar & 0x1) == 0 {
        return None;
    }

    Some(Resource {
        kind: SYS_RES_IOPORT,
        rid,
        start: (bar & 0xFFFC) as u64,
        size: 0,
    })
}

fn irq_resource(device: &Device, rid: usize) -> Option<Resource> {
    let irq = device.pci_config().interrupt_line;
    if irq == 0xFF {
        return None;
    }

    Some(Resource {
        kind: SYS_RES_IRQ,
        rid,
        start: irq as u64,
        size: 1,
    })
}
