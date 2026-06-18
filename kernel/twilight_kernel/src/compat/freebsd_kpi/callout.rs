use super::driver::EINVAL;
use super::mutex::{Mtx, mtx_initialized};
use crate::log;

pub type CalloutFn = fn(usize);

pub const CALLOUT_MPSAFE: u32 = 0x0001;
pub const CALLOUT_RETURNUNLOCKED: u32 = 0x0002;

pub struct Callout {
    initialized: bool,
    mpsafe: bool,
    uses_mtx: bool,
    flags: u32,
    ticks: usize,
    func: Option<CalloutFn>,
    arg: usize,
    pending: bool,
    active: bool,
}

impl Callout {
    pub const fn new() -> Self {
        Self {
            initialized: false,
            mpsafe: false,
            uses_mtx: false,
            flags: 0,
            ticks: 0,
            func: None,
            arg: 0,
            pending: false,
            active: false,
        }
    }
}

pub fn callout_init(callout: &mut Callout, mpsafe: bool) {
    reset_state(callout);
    callout.initialized = true;
    callout.mpsafe = mpsafe;
    callout.flags = if mpsafe { CALLOUT_MPSAFE } else { 0 };
    log!("freebsd_kpi: callout initialized mpsafe={}", mpsafe);
}

pub fn callout_init_mtx(callout: &mut Callout, mtx: &Mtx, flags: u32) {
    reset_state(callout);
    callout.initialized = true;
    callout.uses_mtx = mtx_initialized(mtx);
    callout.mpsafe = (flags & CALLOUT_MPSAFE) != 0;
    callout.flags = flags;
    log!(
        "freebsd_kpi: callout initialized with mutex initialized={} flags={:#x}",
        callout.uses_mtx,
        flags
    );
}

pub fn callout_reset(callout: &mut Callout, ticks: usize, func: CalloutFn, arg: usize) -> i32 {
    if !callout.initialized {
        return EINVAL;
    }

    callout.ticks = ticks;
    callout.func = Some(func);
    callout.arg = arg;
    callout.pending = true;
    callout.active = true;
    log!("freebsd_kpi: callout armed ticks={} arg={}", ticks, arg);
    0
}

pub fn callout_stop(callout: &mut Callout) -> i32 {
    let was_queued = callout.pending || callout.active;
    callout.pending = false;
    callout.active = false;
    log!("freebsd_kpi: callout stopped was_queued={}", was_queued);
    i32::from(was_queued)
}

pub fn callout_drain(callout: &mut Callout) -> i32 {
    let was_queued = callout_stop(callout);
    log!("freebsd_kpi: callout drained");
    was_queued
}

pub fn callout_pending(callout: &Callout) -> bool {
    callout.pending
}

pub fn callout_active(callout: &Callout) -> bool {
    callout.active
}

pub fn callout_deactivate(callout: &mut Callout) {
    callout.active = false;
}

pub fn callout_initialized(callout: &Callout) -> bool {
    callout.initialized
}

fn reset_state(callout: &mut Callout) {
    callout.initialized = false;
    callout.mpsafe = false;
    callout.uses_mtx = false;
    callout.flags = 0;
    callout.ticks = 0;
    callout.func = None;
    callout.arg = 0;
    callout.pending = false;
    callout.active = false;
}
