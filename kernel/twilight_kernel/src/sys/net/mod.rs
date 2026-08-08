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

/// Drive the smoltcp stack once: ingress any pending NIC RX, advance protocol
/// timers, and emit any queued TX. This is the network poll pump (#69).
///
/// smoltcp is otherwise polled *inline* on each socket syscall path, which is
/// sufficient for a process actively reading/writing its own socket. But a
/// process blocked in `poll`/`select` waiting for TCP/UDP readiness cannot make
/// progress on the stack itself: a peer's segment arriving in the NIC RX ring,
/// or a protocol timer (retransmit, TIME-WAIT, delayed-ACK) expiring, changes
/// readiness without any syscall touching the socket. Without a pump, removing
/// the per-tick poll wake-all leaves such waiters asleep forever.
///
/// Returns `Some(ns_until_next_poll)` when smoltcp reports a pending protocol
/// timer deadline, so the caller can arm a deadline-driven wake; `None` when
/// no timed work is pending. The pump itself performs no blocking and is safe
/// to call from the poll loop before re-checking readiness.
pub fn pump_network() -> Option<u64> {
    let mut net = crate::driver::nic::NET.lock();
    let Some((iface, device)) = net.as_mut() else {
        return None;
    };
    let now = time();
    // Take NET then SOCKETS in that order, mirroring the per-socket call sites.
    let mut sockets = crate::sys::net::socket::SOCKETS.lock();
    iface.poll(now, device, &mut sockets);
    let delay = iface.poll_delay(now, &sockets);
    drop(sockets);
    drop(net);
    // poll_delay returns a smoltcp Duration (micros). Convert to monotonic ns
    // for the deadline queue. A zero duration means "poll again now"; surface
    // it as 0 so the caller treats it as immediate readiness.
    delay.map(|d| d.total_micros().saturating_mul(1000))
}
