//! Internet protocol state and its monotonic clock. The network service owns
//! packet processing; socket methods only change buffers and request service.
//! No NIC interrupt may take NET or SOCKETS, allocate, or run smoltcp.

use smoltcp::time::Instant;

pub mod bind_map;
pub mod gw;
pub mod ip;
pub mod mac;
mod policy;
pub mod service;
pub mod socket;
pub mod usage;

/// smoltcp uses the same monotonic timeline as scheduler deadlines (#83).
pub fn time() -> Instant {
    Instant::from_micros((crate::driver::time::monotonic_ns() / 1000).min(i64::MAX as u64) as i64)
}
