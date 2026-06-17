use core::hint::spin_loop;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::log;

/// Default FreeBSD mutex compatibility flag.
///
/// In this first KPI layer, `MTX_DEF` and `MTX_SPIN` both use the same small
/// spin-based implementation. Sleeping locks, witness, priority propagation,
/// and recursive locking are intentionally not implemented yet.
pub const MTX_DEF: u32 = 0x0000;
pub const MTX_SPIN: u32 = 0x0001;
pub const MTX_RECURSE: u32 = 0x0004;
pub const MTX_NOWITNESS: u32 = 0x0008;

pub struct Mtx {
    name: Option<&'static str>,
    type_name: Option<&'static str>,
    opts: u32,
    initialized: AtomicBool,
    locked: AtomicBool,
}

impl Mtx {
    pub const fn new() -> Self {
        Self {
            name: None,
            type_name: None,
            opts: MTX_DEF,
            initialized: AtomicBool::new(false),
            locked: AtomicBool::new(false),
        }
    }
}

pub fn mtx_init(mtx: &mut Mtx, name: &'static str, type_name: Option<&'static str>, opts: u32) {
    mtx.name = Some(name);
    mtx.type_name = type_name;
    mtx.opts = opts;
    mtx.locked.store(false, Ordering::Release);
    mtx.initialized.store(true, Ordering::Release);
}

pub fn mtx_lock(mtx: &Mtx) {
    if !mtx_initialized(mtx) {
        log!("freebsd_kpi: attempted to lock uninitialized mutex");
        return;
    }

    while mtx
        .locked
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        spin_loop();
    }
}

pub fn mtx_unlock(mtx: &Mtx) {
    if !mtx_initialized(mtx) {
        log!("freebsd_kpi: attempted to unlock uninitialized mutex");
        return;
    }

    if !mtx.locked.swap(false, Ordering::Release) {
        log!("freebsd_kpi: attempted to unlock unlocked mutex");
    }
}

pub fn mtx_destroy(mtx: &mut Mtx) {
    mtx.locked.store(false, Ordering::Release);
    mtx.initialized.store(false, Ordering::Release);
    mtx.name = None;
    mtx.type_name = None;
    mtx.opts = MTX_DEF;
}

pub fn mtx_initialized(mtx: &Mtx) -> bool {
    mtx.initialized.load(Ordering::Acquire)
}
