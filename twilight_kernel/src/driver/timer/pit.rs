use crate::driver::timer::cmos::CMOS;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use core::time::Duration;
use crate::task::executor::halt;
use twilight_common::syscall::types::{EFAULT, EINVAL, Timespec};

const PIT_BASE_HZ: u64 = 1_193_182;
const PIT_DIVISOR: u64 = 65_536;
const NSEC_PER_SEC: u64 = 1_000_000_000;

const NUM_NS_PER_TICK: u128 = (NSEC_PER_SEC as u128) * (PIT_DIVISOR as u128);
const DEN_NS_PER_TICK: u128 = PIT_BASE_HZ as u128;

static TICKS: AtomicU64 = AtomicU64::new(0);

static REALTIME_OFFSET_NS: AtomicUsize = AtomicUsize::new(0);
static OFFSET_INITED: AtomicU64 = AtomicU64::new(0); // 0 = no, 1 = yes

pub fn pit_tick_isr() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
fn monotonic_ns() -> u128 {
    let t = TICKS.load(Ordering::Relaxed) as u128;
    (t * NUM_NS_PER_TICK) / DEN_NS_PER_TICK
}

pub fn monotonic_ns_u64() -> u64 {
    core::cmp::min(monotonic_ns(), u64::MAX as u128) as u64
}

pub fn sleep_ns(nanoseconds: u64) {
    if nanoseconds == 0 {
        return;
    }

    let start = monotonic_ns_u64();
    let deadline = start.saturating_add(nanoseconds);

    while monotonic_ns_u64() < deadline {
        halt();
    }
}

pub fn sleep_timespec(req: &Timespec) -> Result<(), i64> {
    if req.tv_sec < 0 || req.tv_nsec < 0 || req.tv_nsec >= (NSEC_PER_SEC as i64) {
        return Err(-(EINVAL as i64));
    }

    let secs = req.tv_sec as u128;
    let nsec = req.tv_nsec as u128;
    let total = secs
        .saturating_mul(NSEC_PER_SEC as u128)
        .saturating_add(nsec);

    sleep_ns(core::cmp::min(total, u64::MAX as u128) as u64);
    Ok(())
}

pub fn init_realtime_offset_from_cmos() {
    // CMOS wall clock in seconds since Unix epoch
    let cmos_secs: u64 = CMOS::new().unix_time();
    let realtime_ns = (cmos_secs as u128) * (NSEC_PER_SEC as u128);
    let mono_ns = monotonic_ns();

    // REALTIME = OFFSET + MONOTONIC  =>  OFFSET = REALTIME - MONOTONIC
    let offset = realtime_ns.saturating_sub(mono_ns);
    REALTIME_OFFSET_NS.store(offset as usize, Ordering::Relaxed);
    OFFSET_INITED.store(1, Ordering::Relaxed);
}

pub const CLOCK_REALTIME: i32 = 0;
pub const CLOCK_MONOTONIC: i32 = 1;

pub fn sys_clock_gettime(clockid: i32, tp: *mut Timespec) -> i64 {
    if tp.is_null() {
        return -(EFAULT as i64);
    }

    // Lazy-init offset if you prefer not to call init at boot
    if OFFSET_INITED.load(Ordering::Relaxed) == 0 {
        init_realtime_offset_from_cmos();
    }

    let ns: u128 = match clockid {
        CLOCK_MONOTONIC => monotonic_ns(),
        CLOCK_REALTIME => {
            let off = REALTIME_OFFSET_NS.load(Ordering::Relaxed);
            off.saturating_add(monotonic_ns() as usize) as u128
        }
        _ => return -EINVAL as i64,
    };

    unsafe {
        (*tp).tv_sec = (ns / (NSEC_PER_SEC as u128)) as i64;
        (*tp).tv_nsec = (ns % (NSEC_PER_SEC as u128)) as i64;
    }
    0
}

pub fn uptime_duration() -> Duration {
    let t = TICKS.load(Ordering::Relaxed) as u128;
    let ns = (t * NUM_NS_PER_TICK) / DEN_NS_PER_TICK;
    let secs = (ns / (NSEC_PER_SEC as u128)) as u64;
    let nsec = (ns % (NSEC_PER_SEC as u128)) as u32;
    Duration::new(secs, nsec)
}

// optional: if you still want a f64 seconds helper
pub fn uptime() -> f64 {
    let d = uptime_duration();
    d.as_secs() as f64 + (d.subsec_nanos() as f64) / 1e9
}
