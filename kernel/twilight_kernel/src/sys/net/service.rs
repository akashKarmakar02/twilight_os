//! Deferred network service on the bootstrap (PID 0) kernel stack. This module
//! owns packet/protocol progress and detached TCP lifetimes, not the socket ABI.
//! IRQs only wake WORK; all smoltcp operations and reclamation run with IF clear
//! in task context. No lock or IRQ guard survives an await/scheduler boundary.

use crate::driver::nic::NET;
use crate::sys::net::socket::SOCKETS;
use crate::utils::sync::{Mutex, WaitQueue};
use smoltcp::iface::{PollIngressSingleResult, PollResult, SocketHandle};
use smoltcp::socket::tcp;

static WORK: WaitQueue = WaitQueue::new();
/// Internet socket syscall waiters, separate from the service's work queue.
pub static SOCKET_WAIT: WaitQueue = WaitQueue::new();

const RX_BUDGET: usize = 32;
const RETRY_NS: u64 = 1_000_000;
const LEGACY_NIC_POLL_NS: u64 = 10_000_000;
// Bounded resource retention if a peer disappears during close. FreeBSD keeps
// detached protocol state too; this small kernel has no configurable linger ABI.
const CLOSE_TIMEOUT_NS: u64 = 30_000_000_000;
const MAX_CLOSING: usize = 128;
static CLOSING: Mutex<[Option<(SocketHandle, u64)>; MAX_CLOSING]> = Mutex::new([None; MAX_CLOSING]);

/// Notify the deferred service. Allocation-free and safe from a NIC interrupt.
pub fn request() {
    WORK.notify_one();
}

/// Retain a closed descriptor's TCP state long enough to drain data and FIN.
/// Lock order is SOCKETS -> CLOSING; no socket method may take the reverse order.
pub fn detach(handle: SocketHandle) {
    let mut sockets = SOCKETS.lock();
    let socket = sockets.get_mut::<tcp::Socket>(handle);
    socket.close();
    if socket.state() == tcp::State::Closed {
        sockets.remove(handle);
    } else {
        let mut closing = CLOSING.lock();
        let slot = super::policy::closing_slot(&closing, |old_handle| {
            matches!(
                sockets.get::<tcp::Socket>(old_handle).state(),
                tcp::State::Closed | tcp::State::TimeWait
            )
        });
        if let Some(index) = slot {
            // Like FreeBSD's bounded TIME-WAIT reuse, reclaim completed state
            // under pressure before sacrificing a connection with queued data.
            if let Some((old_handle, _)) = closing[index].take() {
                sockets.remove(old_handle);
            }
            closing[index] = Some((
                handle,
                crate::driver::time::monotonic_ns().saturating_add(CLOSE_TIMEOUT_NS),
            ));
        } else {
            // Explicit bounded-resource policy: abort rather than leak protocol
            // buffers if all detached-close slots are occupied.
            sockets.get_mut::<tcp::Socket>(handle).abort();
            sockets.remove(handle);
        }
    }
    drop(sockets);
    request();
}

/// Service one bounded packet batch and return an absolute monotonic deadline.
fn pump() -> Option<u64> {
    let mut net = NET.lock();
    let Some((iface, device)) = net.as_mut() else {
        return None;
    };
    let now_ns = crate::driver::time::monotonic_ns();
    let now = super::time();
    let mut sockets = SOCKETS.lock();
    let mut changed = false;
    let mut exhausted = true;
    for _ in 0..RX_BUDGET {
        match iface.poll_ingress_single(now, device, &mut sockets) {
            PollIngressSingleResult::None => {
                exhausted = false;
                break;
            }
            PollIngressSingleResult::SocketStateChanged => changed = true,
            PollIngressSingleResult::PacketProcessed => {}
        }
    }
    changed |= iface.poll_egress(now, device, &mut sockets) == PollResult::SocketStateChanged;
    let mut deadline = super::policy::protocol_deadline(
        now_ns,
        iface
            .poll_delay(now, &sockets)
            .map(|delay| delay.total_micros()),
        RETRY_NS,
    );
    {
        let mut closing = CLOSING.lock();
        for slot in closing.iter_mut() {
            if let Some((handle, expires)) = *slot {
                if now_ns >= expires
                    || sockets.get::<tcp::Socket>(handle).state() == tcp::State::Closed
                {
                    sockets.remove(handle);
                    *slot = None;
                } else {
                    deadline = Some(deadline.map_or(expires, |current| current.min(expires)));
                }
            }
        }
    }
    if exhausted {
        deadline = Some(deadline.map_or(now_ns.saturating_add(RETRY_NS), |current| {
            current.min(now_ns.saturating_add(RETRY_NS))
        }));
    }
    if !device.enable_interrupts() {
        let fallback = now_ns.saturating_add(LEGACY_NIC_POLL_NS);
        deadline = Some(deadline.map_or(fallback, |current| current.min(fallback)));
    }
    // A zero smoltcp delay means queued egress, often waiting for a descriptor
    // or ARP. Yield with a real deadline rather than spinning at an expired one.
    deadline = deadline
        .map(|value| value.max(crate::driver::time::monotonic_ns().saturating_add(RETRY_NS)));
    drop(sockets);
    drop(net);
    if changed {
        SOCKET_WAIT.notify_all();
        crate::sys::proc::poll_wait_queue().notify_all();
    }
    deadline
}

/// Resume PID 0 as the network service after starting init (PID 1).
pub fn run() -> ! {
    // PROCESS_TABLE is IRQ-shared raw state: keep service task context under the
    // same IF-clear contract as syscalls. await_io_until opens IF only in its
    // guarded idle-HLT path, after publishing the blocked process.
    x86_64::instructions::interrupts::disable();
    loop {
        let pid = WORK.prepare_current();
        let deadline = pump();
        crate::sys::proc::await_io_until(deadline);
        WORK.finish_wait(pid);
    }
}
