# Networking

The smoltcp interface and NIC live in `driver/nic::NET`; protocol buffers live
in `sys/net/socket::SOCKETS`. Internet socket methods make one nonblocking
attempt. `sys/syscall/net_io.rs` owns blocking retries, while
`sys/net/service.rs` owns packet processing and protocol timer progress (#83).

## Ownership and execution

```text
RTL8139 INTx -> mask/acknowledge status -> WORK.notify_one()
                                               |
Socket buffer changes -> service::request() ----+
                                               v
                              PID 0 deferred network service
                              RX batch (at most 32 packets)
                              bounded smoltcp egress
                              detached-close reclamation
                              unmask NIC / choose absolute deadline
                                               |
                   socket state change --------+--> SOCKET_WAIT
                                               +--> poll_wait_queue
```

The bootstrap task already owns a kernel stack and saved context. After
`ProcessTable::run()` launches init as PID 1, PID 0 remains Runnable and resumes
into the network service. The service runs with IF clear, like syscalls, so its
process-table and wakeup accesses obey the BSP IRQ exclusion contract. Only the
scheduler's published idle wait opens IF. All locks and IRQ guards are dropped
before `await_io_until()`; kernel preemption remains disabled.

RTL8139 interrupts do not acquire NET/SOCKETS, allocate memory, parse packets,
or schedule. The handler checks only this device's ISR, masks its IMR,
acknowledges its causes and wakes deferred service. After draining a batch, the
service unmasks notifications; status latched while masked raises INTx again.
The fixed IRQ registry retains up to eight callbacks per line, invokes a copied
list outside its lock and rejects overflow. This preserves xHCI and NIC
handlers when PCI assigns them the same IRQ.

RTL8139 TX token creation first reserves a free descriptor. When all four are
busy, smoltcp receives `None`; completion IRQs or the protocol retry deadline
resume progress. No TX ring busy-wait remains. RX data is copied before CAPR
releases its DMA ring storage back to the device. PCNET also checks descriptor
ownership before exposing a TX token, but lacks an interrupt handler: only that
legacy backend uses a 10 ms service fallback. RTL8139 and machines with no NIC
do not acquire an idle periodic network timer.

## Socket waits and readiness

`net_io::retry()` keeps an Arc to the open file and follows publish / attempt /
release / await / recheck. Only EAGAIN is retried. O_NONBLOCK and MSG_DONTWAIT
return EAGAIN immediately. Interruptible waits return EINTR. No open-file,
NET, SOCKETS, process-table reference or queue guard is used across a wait.

TCP sends return the number of bytes accepted into the buffer immediately,
including partial sends. Reads return available data, zero at EOF, or EAGAIN.
Handshake-in-progress sockets are neither readable nor writable; they return
EAGAIN for I/O. A listening descriptor becomes readable when the handshake is
complete, including when the peer has not sent payload. A reset half-open
listener is restored to Listen on its next readiness/accept check.

`accept4` uses the listener's O_NONBLOCK flag to decide whether accepting can
wait. SOCK_NONBLOCK and SOCK_CLOEXEC apply to the new descriptor; plain accept
does not inherit the listener's O_NONBLOCK flag. TCP connect starts a handshake
once, then either returns EINPROGRESS for a nonblocking descriptor or waits with
a five-second absolute deadline. UDP connect just selects a peer and binds an
unbound socket, including when the socket was already bound. UDP receive
truncates a datagram to the supplied buffer and consumes that datagram.

The network service notifies socket/poll waiters after smoltcp reports a socket
state change. These are event wakes, never per-tick wake broadcasts. The
Internet socket wait queue currently has the common WaitQueue limit of 64
concurrent waiters; readiness routing is shared rather than indexed per socket.

## Time and close lifetime

smoltcp timestamps use monotonic microseconds from the validated clocksource,
not CMOS calendar seconds. Interface construction occurs before clocksource
initialization and starts at zero; boot DHCP switches to the monotonic clock
once timekeeping is initialized. DHCP configuration uses a boot-only synchronous pump. Its socket is retained
after configuration so the deferred service continues the existing lease timer
behavior, and is removed on boot timeout. The deferred service starts afterward
and is the only scheduled protocol pump. Applying later DHCP configuration
change events remains separate work; the existing boot configuration policy is
preserved.

Protocol delays in relative microseconds are added to the sampled monotonic
nanosecond timestamp. Immediate retry work gets a one-millisecond future
boundary so a busy descriptor/ARP resolution cannot create a past-deadline
spin. The service publishes the earlier protocol, RX-budget, legacy-backend or
detached-close deadline. Poll/select own only their user deadline: they no
longer compare relative smoltcp durations with absolute scheduler timestamps.

Dropping the last TCP file reference asks smoltcp to close and transfers the
handle to deferred service. Queued data and FIN can drain after the fd is gone;
state is removed when Closed, or after a 30-second retention bound if the peer
vanishes. The fixed table holds 128 detached sockets. Under pressure it reuses the oldest
Closed/TIME-WAIT entry before considering an abort, following FreeBSD's bounded
TIME-WAIT reclamation policy; queued-data/FIN states are never recycled. If all
128 entries still need active teardown, the new close is explicitly aborted
rather than leaking buffers. This bounded policy is a
Twilight limitation, not an implementation of configurable SO_LINGER. Socket
handles are removed exactly once. UDP drops remove their handle immediately.

The design follows FreeBSD's separation of socket consumer lifetime from
protocol teardown in
[`uipc_socket.c`](https://github.com/freebsd/freebsd-src/blob/main/sys/kern/uipc_socket.c),
its buffer wait/restart semantics, bounded TIME-WAIT reuse described in
[FreeBSD's tcp_timewait history](https://lists.freebsd.org/pipermail/freebsd-net/2014-March/038124.html),
and interrupt mask/acknowledge ordering in
[`if_rl.c`](https://github.com/freebsd/freebsd-src/blob/main/sys/dev/rl/if_rl.c).
PID 0 stack reuse, fixed waiter/close caps and PCNET fallback are deliberate
adaptations to Twilight's BSP-only scheduler and current driver support.

## Verification and limits

`make test-net` boots a snapshot VM with the run-blk-bios devices, checks HTTP
body integrity without input wakeups, then runs `net_io_test` against local
TCP/UDP peers. It checks blocking and nonblocking connect, partial transfers,
EOF, EAGAIN, EINTR, bound UDP connect, UDP truncation, listener readiness without
payload, an 8 MiB slow-reader transfer and scheduler responsiveness while a
socket read/write is blocked. See
[`tools/net-regression/README.md`](../tools/net-regression/README.md).
`make test-time` separately checks existing clock/scheduler modes.

The listener still represents one pending smoltcp TCP connection; backlog is
not a full Unix accept queue. httpd still handles clients sequentially, so a
slow client can delay other HTTP clients while the rest of the OS remains
schedulable. SO_ERROR, configurable socket I/O timeouts, full shutdown-direction
semantics and socket-indexed wake queues remain unsupported/separate work.
Physical RTL8139 hardware and PCNET behavior require additional device testing.
