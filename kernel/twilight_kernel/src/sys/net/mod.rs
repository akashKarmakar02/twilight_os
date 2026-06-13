use crate::driver::timer::cmos::CMOS;
use smoltcp::time::Instant;

pub mod bind_map;
pub mod gw;
pub mod ip;
pub mod mac;
pub mod socket;
pub mod usage;

pub fn time() -> Instant {
    let mut cmos = CMOS::new();
    Instant::from_micros((cmos.unix_time() * 1000000) as i64)
}
