use crate::log;
use x86_64::instructions::port::Port;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusSpaceTag {
    IoPort,
    MmioUnsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusSpaceHandle {
    pub base: u64,
}

impl BusSpaceHandle {
    pub const fn new(base: u64) -> Self {
        Self { base }
    }
}

pub fn bus_space_read_1(tag: BusSpaceTag, handle: BusSpaceHandle, offset: usize) -> u8 {
    let Some(port) = io_port(tag, handle, offset, 1) else {
        return 0;
    };

    // SAFETY: The caller supplied an I/O port resource. This wrapper only
    // narrows after range checking and performs a single programmed I/O read.
    unsafe { Port::<u8>::new(port).read() }
}

pub fn bus_space_read_2(tag: BusSpaceTag, handle: BusSpaceHandle, offset: usize) -> u16 {
    let Some(port) = io_port(tag, handle, offset, 2) else {
        return 0;
    };

    // SAFETY: The caller supplied an I/O port resource. This wrapper only
    // narrows after range checking and performs a single programmed I/O read.
    unsafe { Port::<u16>::new(port).read() }
}

pub fn bus_space_read_4(tag: BusSpaceTag, handle: BusSpaceHandle, offset: usize) -> u32 {
    let Some(port) = io_port(tag, handle, offset, 4) else {
        return 0;
    };

    // SAFETY: The caller supplied an I/O port resource. This wrapper only
    // narrows after range checking and performs a single programmed I/O read.
    unsafe { Port::<u32>::new(port).read() }
}

pub fn bus_space_write_1(tag: BusSpaceTag, handle: BusSpaceHandle, offset: usize, value: u8) {
    let Some(port) = io_port(tag, handle, offset, 1) else {
        return;
    };

    // SAFETY: The caller supplied an I/O port resource. This wrapper only
    // narrows after range checking and performs a single programmed I/O write.
    unsafe { Port::<u8>::new(port).write(value) };
}

pub fn bus_space_write_2(tag: BusSpaceTag, handle: BusSpaceHandle, offset: usize, value: u16) {
    let Some(port) = io_port(tag, handle, offset, 2) else {
        return;
    };

    // SAFETY: The caller supplied an I/O port resource. This wrapper only
    // narrows after range checking and performs a single programmed I/O write.
    unsafe { Port::<u16>::new(port).write(value) };
}

pub fn bus_space_write_4(tag: BusSpaceTag, handle: BusSpaceHandle, offset: usize, value: u32) {
    let Some(port) = io_port(tag, handle, offset, 4) else {
        return;
    };

    // SAFETY: The caller supplied an I/O port resource. This wrapper only
    // narrows after range checking and performs a single programmed I/O write.
    unsafe { Port::<u32>::new(port).write(value) };
}

fn io_port(tag: BusSpaceTag, handle: BusSpaceHandle, offset: usize, width: u64) -> Option<u16> {
    match tag {
        BusSpaceTag::IoPort => {
            let start = handle.base.checked_add(offset as u64)?;
            let end = start.checked_add(width.saturating_sub(1))?;
            if end > u16::MAX as u64 {
                log!(
                    "freebsd_kpi: I/O port access out of range base={:#x} offset={:#x} width={}",
                    handle.base,
                    offset,
                    width
                );
                return None;
            }

            Some(start as u16)
        }
        BusSpaceTag::MmioUnsupported => {
            log!(
                "freebsd_kpi: MMIO bus-space access unsupported base={:#x} offset={:#x} width={}",
                handle.base,
                offset,
                width
            );
            None
        }
    }
}
