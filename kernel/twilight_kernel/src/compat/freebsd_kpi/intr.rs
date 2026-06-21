use alloc::vec::Vec;

use lazy_static::lazy_static;
use crate::utils::sync::Mutex;

use super::device::{Device, DeviceId, device_get_nameunit};
use super::driver::EINVAL;
use super::resource::{Resource, SYS_RES_IRQ, rman_get_start};
use crate::log;

/// Compatibility interrupt type flags for future FreeBSD driver ports.
///
/// These are registry metadata only for now. They are not wired into Twilight's
/// real interrupt dispatcher yet.
pub const INTR_TYPE_MISC: u32 = 0x0000;
pub const INTR_TYPE_NET: u32 = 0x0004;
pub const INTR_TYPE_BIO: u32 = 0x0008;
pub const INTR_MPSAFE: u32 = 0x0100;
pub const INTR_EXCL: u32 = 0x0200;

pub type IntrHandler = fn(usize);
pub type IntrFilter = fn(usize) -> i32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntrCookie {
    id: usize,
    irq_line: u8,
    device_id: DeviceId,
    device_name: &'static str,
    flags: u32,
}

impl IntrCookie {
    pub fn id(self) -> usize {
        self.id
    }

    pub fn irq_line(self) -> u8 {
        self.irq_line
    }

    pub fn device_id(self) -> DeviceId {
        self.device_id
    }

    pub fn device_name(self) -> &'static str {
        self.device_name
    }

    pub fn flags(self) -> u32 {
        self.flags
    }
}

#[derive(Clone, Copy)]
struct IntrEntry {
    cookie: IntrCookie,
    filter: Option<IntrFilter>,
    handler: Option<IntrHandler>,
    arg: usize,
}

struct IntrRegistry {
    next_cookie_id: usize,
    entries: Vec<IntrEntry>,
}

impl IntrRegistry {
    fn new() -> Self {
        Self {
            next_cookie_id: 1,
            entries: Vec::new(),
        }
    }

    fn next_cookie_id(&mut self) -> usize {
        let id = self.next_cookie_id;
        self.next_cookie_id = self.next_cookie_id.saturating_add(1).max(1);
        id
    }
}

lazy_static! {
    static ref INTR_REGISTRY: Mutex<IntrRegistry> = Mutex::new(IntrRegistry::new());
}

pub fn bus_setup_intr(
    device: &Device,
    irq_resource: Resource,
    flags: u32,
    filter: Option<IntrFilter>,
    handler: Option<IntrHandler>,
    arg: usize,
    cookie_out: &mut Option<IntrCookie>,
) -> i32 {
    if irq_resource.kind != SYS_RES_IRQ || (filter.is_none() && handler.is_none()) {
        return EINVAL;
    }

    let irq = rman_get_start(irq_resource) as u8;
    let mut registry = INTR_REGISTRY.lock();
    let cookie = IntrCookie {
        id: registry.next_cookie_id(),
        irq_line: irq,
        device_id: device.id(),
        device_name: device_get_nameunit(device),
        flags,
    };

    registry.entries.push(IntrEntry {
        cookie,
        filter,
        handler,
        arg,
    });
    *cookie_out = Some(cookie);

    log!(
        "freebsd_kpi: registered irq handler dev={} irq={} cookie={} flags={:#x}",
        cookie.device_name,
        cookie.irq_line,
        cookie.id,
        cookie.flags
    );

    0
}

pub fn bus_teardown_intr(device: &Device, irq_resource: Resource, cookie: IntrCookie) -> i32 {
    if irq_resource.kind != SYS_RES_IRQ {
        return EINVAL;
    }

    let irq = rman_get_start(irq_resource) as u8;
    let device_id = device.id();
    let mut registry = INTR_REGISTRY.lock();
    let Some(index) = registry.entries.iter().position(|entry| {
        entry.cookie.id == cookie.id
            && entry.cookie.irq_line == irq
            && entry.cookie.device_id == device_id
    }) else {
        return EINVAL;
    };

    let entry = registry.entries.remove(index);
    let _registered_callback = (entry.filter, entry.handler, entry.arg);
    log!(
        "freebsd_kpi: removed irq handler dev={} irq={} cookie={}",
        entry.cookie.device_name,
        entry.cookie.irq_line,
        entry.cookie.id
    );

    0
}

pub fn debug_registered_intr_count() -> usize {
    INTR_REGISTRY.lock().entries.len()
}
