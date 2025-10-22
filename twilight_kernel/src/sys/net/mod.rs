use smoltcp::time::Instant;
use crate::driver::timer::cmos::CMOS;

pub mod gw;
pub mod ip;
pub mod mac;
pub mod usage;
pub mod socket;

pub fn time() -> Instant {
    let mut cmos = CMOS::new();
    Instant::from_micros((cmos.unix_time() * 1000000) as i64)
}