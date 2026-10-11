# HTTP-triggered stutter and network stalls

Date: 2026-10-12 (Asia/Kolkata). Investigation against commit
`07a2db65e53a19d64c94e533b09e34b8784b2330`, with the pre-existing local workspace
changes preserved. Investigation branch: `fix/httpd-stutter-83`.

Main tracking issue: [#83 — HTTP traffic can halt the BSP and strand network pollers](https://github.com/akashKarmakar02/twilight_os/issues/83).
Separately discovered RTC defect: [#84](https://github.com/akashKarmakar02/twilight_os/issues/84).

Status: **fix implemented; kernel/userspace builds and VM regressions passed**.
One legacy file probe requires a writable installed system, as recorded below.
The investigation below records the original image's behavior and source line
numbers at the baseline commit. The implementation and final verification
sections at the end describe the corrected code submitted with this report.

## Original finding (before the fix)

The observed system-wide pause has a concrete kernel cause: HTTP sends enter
`TcpSocket::write()`, which holds network locks and waits in kernel HLT loops.
The final HLT has no wake deadline. With kernel preemption disabled and the
one-shot LAPIC timer disarmed, the BSP can remain halted inside httpd's syscall.
The shell, compositor, logging daemon, and init cannot execute until that syscall
makes progress and releases ownership. A five-second timeout in the loop does
not bound a HLT that never returns.

There is also a separate, reproducible network-only failure: an idle HTTP
listener can sleep through incoming packets because RTL8139 RX interrupts are
disabled and there is no NIC readiness wakeup. In that case HTTP requests time
out while the shell and ordinary sleeping processes still work.

Two additional timing defects make this architecture unreliable: relative
smoltcp delays are passed as absolute scheduler deadlines, and the network stack
uses a CMOS clock with one-second resolution. These defects are verified in
source; their individual contributions to runtime cost were not isolated.

## Test environment and safety

The test reproduced the device configuration in `GNUmakefile:93-112`:

- BIOS boot from the existing live ISO, default `pc` machine and TCG accelerator.
- 1024 MiB RAM, four advertised vCPUs; Twilight still schedules on the BSP only.
- RTL8139 with QEMU user networking, qemu-xhci, virtio-blk, standard VGA.
- QEMU 10.2.2 (`qemu-10.2.2-1.fc44`); guest selected HPET and one-shot LAPIC.
- The existing hdd.img was attached with `snapshot=on`; guest disk writes went
  to a temporary overlay, preserving the original disk.
- Host HTTP forwarding used `127.0.0.1:18000` rather than the target's port 8000.
  Display was headless; serial logs, monitor access, packet capture on the repeat
  run, and a local debugger endpoint were added.
- QEMU interrupt tracing remained enabled, with output under `/tmp` rather than
  overwriting the repository's qemu.log.

The literal `make run-blk-bios` target was not invoked because it rebuilds the
ISO and opens the persistent disk for writes. Its QEMU device arguments were
used directly with the diagnostic/safety differences above.

The existing ISO was deliberately retained to investigate the user's current
image. The kernel extracted from that ISO and the output of `make kernel` were
byte-identical:

```text
ISO SHA-256:
94dbc74d7e76e5fa8299b669ab0842bacb55f53660483195ce334bcf03c822fd

ISO kernel and freshly built kernel SHA-256:
4223fd3944f3c80e4f6d4e160e3a30a3c9ede125f29ecb4dd88cd9d1fa38c2f8
```

Userspace was the existing ISO build; it was not freshly rebuilt. Daemon status
confirmed httpd PID 3, shell PID 4, twilogd PID 2, and twland PID 5, with no
httpd restarts. The disabled network service is a placeholder, not a background
network pump.

## Runtime evidence

### A. Whole-guest stall inside a TCP write

The first VM boot reached the shell. A host GET of `/` produced:

```text
curl: (28) Operation timed out after 12002 milliseconds with 0 bytes received
http=000 first_byte=0.000000 total=12.002829 bytes=0
```

Subsequent monitor-injected keyboard commands were not processed normally; the
serial prompt acquired only the first `e` of the attempted echo command.
Two debugger attachments, separated by further investigation, found the same
halted instruction and call chain:

```text
#0  0xffffffff80091655 arch::x86_64::halt
#1  0xffffffff800690c5 TcpSocket as FileIO::write
#2  0xffffffff80011058 sys::syscall::service::write
#3  0xffffffff800d97e1 sys::syscall::syscall_handler
#4  0xffffffff800b941f arch::x86_64::syscall::x86_64_syscall_handler

PID = 3
PREEMPT_COUNT = 3
IRQ_DISABLE_DEPTH = 1
RFLAGS.IF = 1, CPL = 0, HLT = 1

LAPIC LVTT: masked, one-shot
Timer current_count = 0
ISR: none; IRR: none
```

The preemption count is consistent with the open-file, NET, and SOCKETS guards
held by this path. The sampled IF bit was enabled at HLT; this was not merely
an interrupt-disabled spin. The IRQ depth value is recorded as observed, not
treated as evidence of a separate counter defect.

### B. Healthy baseline and input-dependent HTTP progress

On a fresh VM, QEMU keyboard injection successfully ran:

```sh
echo baseline
twinitctl status
clockcheck rel 10000000 10
```

All ten requested 10 ms sleeps completed. Their measured median was
**10.587770 ms**, with maximum **11.055750 ms**, and every record reported `ok`.

A GET of `/` was then started, followed about one second later by keyboard
input for `echo afterhttp`. This request completed:

```text
http=200 first_byte=1.054325 total=1.184481 bytes=4357
```

Packet capture, expressed relative to its initial SYN:

| Event | Time after SYN |
| --- | ---: |
| Host SYN enters guest network | 0 ms |
| Guest SYN/ACK | 996.988 ms |
| Host GET | 997.189 ms |
| Guest HTTP 200 header | 1053.688 ms |
| Last body segment | 1177.038 ms |
| Guest RST | 1183.748 ms |

The first response closely followed injected input. TTY input explicitly
notifies the shared poll wait queue (`sys/console/tty.rs:919-935`), while keyboard
interrupts also wake HLT. Thus interacting with the VM can change the symptom;
it must not be mistaken for an HTTP fix. No artificial keyboard events should
be used in the final HTTP liveness regression.

### C. Incoming packets stranded while the shell remains usable

After the successful GET, a stylesheet request with no keyboard injection timed
out after **8.002296 s**, receiving no bytes. Image attempts timed out after
**10.001946 s** and **8.002786 s**, also with no bytes.

The capture showed incoming SYNs and retransmissions without a guest SYN/ACK.
Samples in this phase found PID 1 in its normal scheduler-aware nanosleep idle
path, with `PREEMPT_COUNT=0` and an armed one-shot timer, rather than PID 3 in
the TCP HLT path. The shell subsequently printed `stillalive`, service status,
and `twilogctl show`. A further 100-iteration clockcheck completed while the
image request failed. The request log contained the successful `200 GET /`;
the later attempts never reached normal HTTP request handling.

Brief modifier-key injection did not reliably restore these requests. Keyboard
and unrelated IPC wakeups can help progress, but they are not a deterministic
recovery mechanism.

## Root cause details

### 1. Scheduled TCP operations use kernel HLT waits while holding locks

Relevant locations:

- `kernel/twilight_kernel/src/sys/net/socket/tcp.rs:284-387` — read/write/close/poll.
- The same file, lines 58-154 and 172-224 — connect/listen/blocking accept waits.
- `kernel/twilight_kernel/src/sys/syscall/service.rs:603-724` — write holds the
  open-file guard while invoking the Internet socket operation.
- `kernel/twilight_kernel/src/task/executor.rs:18-51` — kernel-context sleep.
- `kernel/twilight_kernel/src/arch/x86_64/mod.rs:12-22` — trailing HLT helper.
- `kernel/twilight_kernel/src/sys/proc/mod.rs:2308-2345` — kernel timer interrupts
  do not switch tasks with kernel preemption disabled.

`TcpSocket::write()` takes NET, then SOCKETS. Even when `send_slice()` has
accepted bytes, it does not return immediately. It optionally waits 4 ms and
then unconditionally halts before the next loop iteration can poll/transmit and
return the positive byte count.

```text
HTTP write syscall; open-file guard held
  -> take NET and SOCKETS
  -> poll stack; queue response bytes
  -> executor::sleep(4 ms), if poll_delay is Some
       arm kernel wake -> HLT until deadline -> clear kernel wake
  -> arch::x86_64::halt(), without a new deadline
       no future timer/IRQ guaranteed
       kernel timer path cannot switch to shell/compositor
  -> next poll / return / timeout check is unreachable until an IRQ arrives
```

The scheduler clears an expired quantum rather than continually rearming it
(`sys/preempt.rs:310-319`). The clockevent intentionally disarms when no live
deadline remains (`driver/time/clockevent.rs:91-107`). These are valid one-shot
policies; TCP incorrectly relies on incidental periodic wakeups that no longer
exist. Other software deadlines may temporarily wake the HLT, then become
exhausted without allowing their owning process to run.

The 4 ms waits also retain the BSP and network locks instead of blocking only
the calling process. The 1024-byte TCP transmit buffer forces larger HTTP
responses through more such writes. Extra interrupts can turn a hard stall
into repeated pauses, explaining why the issue may appear as stutter.

### 2. NIC RX has no event path to a sleeping poller

`driver/nic/rtl8139.rs:185-186` leaves the interrupt-mask write commented out.
The driver registers no NIC IRQ handler. `pump_network()` ingests RX only when
explicitly called; it is not an independent worker.

For an idle listener, smoltcp can report no protocol timer. Infinite
`poll(..., -1)` then blocks without a deadline. Incoming SYNs sit in the NIC RX
ring without causing an event wake. Periodic init sleeps continue to execute,
but do not themselves establish a network pump/wakeup contract.

Any activity that wakes the shared poll queue can mask the problem. This is why
keyboard-driven testing alone is insufficient and why the repeated SYN-only
capture matters.

### 3. Relative delay is used as an absolute deadline

`sys/net/mod.rs:27-46` explicitly returns nanoseconds **until** the next poll.
`sys/syscall/poll.rs:261-271` compares that duration directly with an absolute
user deadline and passes the result to `await_io_until()`. The latter's contract
is an **absolute monotonic** deadline (`sys/proc/mod.rs:1777-1789`).

For example, at monotonic time 10 s, a network delay of 100 ms should produce
deadline 10.1 s. The current code instead arms deadline 0.1 s, which is already
9.9 s in the past. Pending protocol timers can cause immediate timeout/re-pump
cycles and unnecessary timer/CPU work, including for unrelated descriptors
whose poll loops also pump networking. This defect is statically proven;
an isolated CPU-profile comparison was not performed.

### 4. smoltcp time is still whole-second RTC time

`sys/net/mod.rs:11-13` returns `CMOS::unix_time() * 1_000_000`. Multiplication
changes units without adding resolution. NIC initialization has another copy
of this clock, and TCP connect reads CMOS directly. `sys/clk.rs::epoch_time()`
also returns whole-second RTC values for legacy TCP timeouts.

Subsecond smoltcp deadlines therefore see a clock that is unchanged for nearly
a second and then jumps. They also inherit wall-clock adjustments and repeated
CMOS I/O. Networking should use the existing validated monotonic clocksource,
with one consistent conversion to smoltcp microseconds. This does not mean that
Twilight's actual `CLOCK_MONOTONIC` is based on RTC or interrupts; that subsystem
correctly selected HPET in these runs.

### 5. Listener readiness and TCP teardown need correction

`TcpSocket::poll(IO::Read)` only calls `can_recv()`, including for a listener.
A handshake completed without payload therefore does not make the listening
descriptor readable, although accept readiness must depend on an incoming
connection. This is source-verified; a dedicated handshake-without-payload test
was not run.

`service::close()` drops the fd entry, and `TcpSocket::drop()` immediately
removes its smoltcp socket. The successful capture ended with RST instead of a
graceful FIN. Socket lifetime must preserve pending transmit/retransmission and
close state. That GET delivered all 4357 bytes; this investigation did not prove
response truncation or attribute the whole-guest stall to teardown.

## Why these failures surfaced together

The TCP implementation still contains legacy assumptions about periodic IRQs
and synchronous stack pumping. One-shot clockevents (#68) remove incidental
ticks, and deadline-aware poll (#69) removes periodic poll wake-all. Those newer
mechanisms require every wait to have a real event producer or an explicit
protocol deadline. The old TCP/NIC paths do not satisfy that contract.

History inspected: `48b942f` introduced the pump and the relative-delay/absolute-
deadline mismatch; `28b4e8c` followed with poll review fixes. `17646f0` explicitly
armed the kernel-context sleep deadline but did not remove TCP's subsequent bare
HLT. This identifies the integration gap; no bisect was performed, so a single
first-bad commit for the user's subjective stutter is not established.

The run target's `-smp 4` cannot compensate: APs are parked. Interrupt trace
logging can add host overhead, but it cannot explain the sampled PID 3 HLT with
a masked timer. No host-throughput or graphical frame-time benchmark was made.

## FreeBSD reference and original correction plan

The reference reviewed was FreeBSD
[`uipc_socket.c`](https://cgit.freebsd.org/src/tree/sys/kern/uipc_socket.c),
[`uipc_sockbuf.c`](https://cgit.freebsd.org/src/tree/sys/kern/uipc_sockbuf.c), and
[`sys_generic.c`](https://cgit.freebsd.org/src/tree/sys/kern/sys_generic.c).
Its socket buffer waits release the relevant mutex while sleeping and readiness
changes wake registered consumers; listener polling checks completed connections.
Twilight should follow those ownership and notification semantics.

Recommended implementation sequence:

1. Replace scheduled-process TCP HLT waits with short readiness/transfer
   operations plus scheduler-aware waits. Reuse WaitQueue and the token-safe
   absolute deadline queue; release open-file, NET, and SOCKETS guards before
   blocking. Return positive partial writes promptly. Preserve errno, EOF,
   O_NONBLOCK, interruption, and timeout semantics.
2. Add a genuine NIC event/deferred-work path. Hard IRQ handling must be bounded
   and allocation-free; the current RX ingestion allocates Vec buffers, so it
   must remain in suitable deferred/task context. Re-audit any lock moved into
   IRQ context and update `docs/kernel-locking.md`.
3. Use the monotonic clock for all smoltcp timestamps. Convert advisory delays
   into absolute monotonic deadlines at a clearly owned boundary, with
   saturation and immediate-work handling. Add host tests for this pure math
   and update `docs/timekeeping.md`.
4. Correct listener/EOF readiness and retain protocol state through graceful
   close or an explicitly defined abort policy.
5. Add network regressions that run without keyboard/serial wakeups, plus
   concurrent HTTP and deadline/interactive responsiveness tests.

Merely increasing timeouts, inserting more sleeps, restoring periodic wake-all,
or enabling kernel preemption would leave the missing ownership and event
contracts unresolved. Increasing socket buffers may reduce the number of
transfers, but does not fix an unbounded wait.

## Investigation verification (before implementation)

| Check | Result |
| --- | --- |
| `make kernel` | Passed; existing non-root workspace profile warning only |
| Extract ISO kernel with xorriso; compare SHA-256 with rebuilt kernel | Identical |
| BIOS/RTL8139/xHCI/virtio-blk live boot, twice | Passed |
| Monitor `sendkey`: echo, status, clockcheck | Passed on fresh VM |
| Host HTTP GET with no incidental input | Reproduced timeout and TCP HLT stall |
| GET with keyboard activity | HTTP 200; timing and packet capture recorded |
| Later CSS/image requests | Reproduced missed network progress, shell still usable |
| GDB stack/PID/counters; HMP register and LAPIC inspection | Captured both failure states |
| `twilogctl show` | Confirmed successful request log and running daemons |
| Targeted timing regression | Passed: 15 combinations, 200 iterations, `failures=0` |
| VM cleanup | Both investigation VMs exited through monitor `quit` |

The exact timing command was:

```sh
TIME_REGRESSION_CELLS='tcg|qemu64|4' TIME_REGRESSION_ITERS=10 \
TIME_REGRESSION_DURATIONS='10000 1000000 16666667' \
make -o twilight-os.iso test-time
```

This exercises rel/abs/read/poll/load at three durations on one TCG cell. All
reported early/backward counts were zero. `-o` kept the already-verified ISO
instead of rebuilding userspace. The full default matrix, KVM, new network
regressions, and guest disk/Wayland test suites were not run. No userspace source
was changed, so `make userspace` was not run. The harness prints a PTY I/O error
when the guest shuts down, followed by `META done failures=0` and exit status 0.

Passing timing tests do not validate HTTP liveness: the existing harness has no
HTTP traffic. The visually reported frame stutter was not quantified; the
confirmed results are syscall/CPU stalls, request timeouts, and network wakeup
failure. No performance improvement is claimed because no fix was applied.

## How to interact with the VM

Yes: the agent can boot QEMU, type guest commands through emulated keyboard
events or serial input, inspect the screen, make host HTTP calls, capture
packets, and attach GDB. All of these were exercised here except serial command
injection; keyboard input was used for the guest commands.

A repeatable diagnostic launch, preserving the disk, is:

```sh
qemu-system-x86_64 \
  -m 1024 -smp 4 -boot d \
  -netdev user,id=net0,hostfwd=tcp:127.0.0.1:18000-:80 \
  -device rtl8139,netdev=net0 -device qemu-xhci \
  -cdrom twilight-os.iso \
  -drive file=hdd.img,if=none,format=raw,id=vd0,snapshot=on \
  -device virtio-blk-pci,drive=vd0 \
  -chardev socket,id=ser,path=/tmp/twilight-httpd-serial.sock,server=on,wait=off,logfile=/tmp/twilight-httpd-repeat-serial.log \
  -serial chardev:ser \
  -monitor unix:/tmp/twilight-httpd-monitor.sock,server=on,wait=off \
  -gdb tcp:127.0.0.1:12345 \
  -object filter-dump,id=httpcapture,netdev=net0,file=/tmp/twilight-httpd-repeat.pcap \
  -d int,guest_errors,unimp -D /tmp/twilight-httpd-repeat-qemu.log \
  -vga std -display none -no-reboot
```

Use available ports and fresh temporary paths when another test VM is present.
The actual runs enabled GDB later through the monitor's `gdbserver` command;
the launch above enables the same endpoint immediately.

In another terminal:

```sh
curl --noproxy '*' --max-time 12 http://127.0.0.1:18000/
curl --noproxy '*' --max-time 12 http://127.0.0.1:18000/style.css
curl --noproxy '*' --max-time 12 http://127.0.0.1:18000/img.png

# Type echo hi using the emulated keyboard. Send each key separately and allow
# time for its press/release before the next command.
for key in e c h o spc h i ret; do
  printf 'sendkey %s 20\n' "$key" |
    socat - UNIX-CONNECT:/tmp/twilight-httpd-monitor.sock >/dev/null
  sleep 0.08
done

printf 'info registers\ninfo lapic\n' |
  socat - UNIX-CONNECT:/tmp/twilight-httpd-monitor.sock
printf 'screendump /tmp/twilight-httpd-screen.ppm\n' |
  socat - UNIX-CONNECT:/tmp/twilight-httpd-monitor.sock
ffmpeg -v error -i /tmp/twilight-httpd-screen.ppm -frames:v 1 /tmp/twilight-httpd-screen.png

gdb -q -nx -batch kernel/twilight_kernel/kernel \
  -ex 'target remote 127.0.0.1:12345' -ex 'bt 6' -ex 'detach'

tcpdump -tt -nn -r /tmp/twilight-httpd-repeat.pcap 'tcp port 80'

# Stop only the test instance created above.
printf 'quit\n' | socat - UNIX-CONNECT:/tmp/twilight-httpd-monitor.sock
```

The serial Unix socket supports bidirectional guest terminal interaction too.
Reading `/tmp/twilight-httpd-repeat-serial.log` is sufficient for passive output.
Monitor/GDB access briefly pauses or samples execution; debugger-paused time
must not be included in latency measurements. The successful GET's timing was
collected before the subsequent debugger attachment.

The sandbox initially blocked monitor and HTTP connections. Scoped local-socket
escalations were approved and used; no sandbox restriction was bypassed.

## Evidence files

Critical observations are embedded above so the report survives temporary-file
cleanup. Additional artifacts from this session remain under `/tmp`:

- `twilight-httpd-serial.log`, `twilight-httpd-qemu.log` — first boot/stall.
- `twilight-httpd-gdb-first.log`, `twilight-httpd-gdb-stall.log` — stalled stack.
- `twilight-httpd-repeat-serial.log`, `twilight-httpd-repeat-qemu.log` — second VM.
- `twilight-httpd-repeat.pcap` — incoming SYNs and successful response timing.
- `twilight-httpd-reproduce.log`, `twilight-httpd-wakeup-probe.log` — host probes.
- `twilight-httpd-gdb-css.log`, `twilight-httpd-gdb-loaded.log` — later idle state.
- `twilight-httpd-kernel-build.log`, `twilight-httpd-time-regression.log` — checks.
- `twilight-httpd-boot.png`, `twilight-httpd-stall.ppm`,
  `twilight-httpd-repeat-final.ppm` — screen captures.

No ROMs, generated guest binaries, disk images, or large trace files are included
in this change.

## Separate RTC observation

While tracing network time, `driver/timer/cmos.rs:116-120` was found to test the
update-in-progress bit with `value & 0x80 == 1`. Masking can only produce 0 or
128, so it always returns false; an exhaustive host check of all 256 byte values
confirmed this. RTC reads can consequently span an update without the intended
guard. Dynamic rollover corruption was not reproduced, and this is not claimed
as the cause of the TCP HLT stall. It is tracked separately in
[#84](https://github.com/akashKarmakar02/twilight_os/issues/84).

## Implemented correction (#83)

The kernel now separates nonblocking socket buffer operations from scheduled
protocol progress. TCP and UDP contain no HLT/executor-sleep loops. Positive
TCP sends return their partial byte count immediately; EAGAIN is handled by
`sys/syscall/net_io.rs`, which publishes a waiter, attempts work, releases the
OpenFile/network guards and awaits an event before retrying. The retained Arc
keeps the descriptor alive; accept reacquires the current process only after
its wait. Nonblocking I/O, signal interruption, partial transfers and EOF are
preserved. Default-ignored signals do not interrupt a wait.

PID 0 reuses its existing bootstrap kernel stack as the deferred network
service after launching init as PID 1. This avoids a new thread constructor,
address space or task type, and preserves the existing userspace PID layout.
PID 0 runs with IF clear and no user execution frame. Its only blocking
boundary is the existing scheduler-aware await_io_until path, after every
lock has been dropped. Kernel preemption remains disabled.

RTL8139 now registers an INTx handler. Hard IRQ checks its status, masks and
acknowledges causes, and wakes the service through an allocation-free queue.
The worker processes at most 32 RX packets per batch, performs bounded smoltcp
egress, unmasks notifications and awaits an absolute deadline. New packets
latched while masked raise another interrupt after unmask. IRQs do not run
smoltcp or allocate. Shared IRQ registration retains up to eight handlers and
uses lock_irq everywhere, preserving USB/NIC handlers on a shared PCI line.

An I/O event that adds a runnable peer while the current task has no quantum
now rearms a scheduling event. Otherwise waking the network service from an
IRQ could leave it behind a sole CPU-bound userspace task with no timer armed.

smoltcp reads monotonic microseconds rather than CMOS whole seconds. Interface
construction uses zero before clocksource initialization; boot DHCP then uses
the initialized monotonic clock. Its existing protocol socket is retained after
configuration for lease timer progress, and removed on boot timeout. The unused
CMOS-based sys/clk.rs helper was removed. Poll/select arm only their user
deadline; the service independently converts protocol delays into absolute
monotonic nanoseconds. Immediate work gets a 1 ms future boundary to yield
rather than spin on a past deadline. PCNET retains a 10 ms service fallback
because its IRQ path is not implemented; this timer is not used for RTL8139 or
machines without a supported NIC.

TCP listener readiness now means a completed handshake, including a connection
with no application payload. Connecting sockets do not report writable or EOF
before establishment. Accept waiting follows the listener's O_NONBLOCK flag;
accept4 flags apply to the new descriptor. UDP connect is synchronous peer
selection and cannot hang when the socket is already bound. UDP receives consume
one datagram and truncate to the supplied buffer.

Last-fd TCP close transfers protocol state to deferred service so queued bytes
and FIN drain after the descriptor disappears. The service reclaims Closed
connections or expires abandoned teardown after 30 seconds. The fixed 128-slot
close table reuses the oldest completed Closed/TIME-WAIT entry under pressure;
connections still sending data/FIN are never recycled. If all slots need active
teardown, the new close is explicitly aborted rather than leaking buffers.
This is a documented resource bound, not full SO_LINGER support.

Related defects fixed while implementing the event path:

- RTL8139 no longer busy-waits under NET when all four TX descriptors are busy;
  it returns no TxToken until a descriptor becomes available. PCNET also checks
  ownership before exposing its next TX buffer.
- RTL8139 copies RX data before CAPR hands its storage back to DMA. Acquire/
  release fences order DMA consumption and publication; TX publication occurs
  before the transmit port is written.
- The timing harness now exits on INT/TERM through its EXIT cleanup. Its old
  cleanup-only signal trap stopped QEMU but continued the test loop against a
  closed PTY; this was reproduced while stopping a superseded test run.

The implementation follows FreeBSD's socket buffer wait/restart and detached
protocol lifetime design in
[uipc_socket.c](https://github.com/freebsd/freebsd-src/blob/main/sys/kern/uipc_socket.c),
interrupt mask/acknowledge ordering in
[if_rl.c](https://github.com/freebsd/freebsd-src/blob/main/sys/dev/rl/if_rl.c),
and its historical bounded TIME-WAIT reuse policy documented in the
[FreeBSD network discussion](https://lists.freebsd.org/pipermail/freebsd-net/2014-March/038124.html).
The PID 0 reuse and fixed close-table/PCNET fallback are written Twilight
adaptations. Detailed invariants are in docs/networking.md, kernel-locking.md,
timekeeping.md, process-management.md and syscall-layer.md.

## Regression development findings

The first corrected VM completed 40 HTTP requests with exact response bodies,
median 10.647 ms and maximum 109.098 ms, plus a request delayed by two seconds
and a graceful EOF. No keyboard input was injected during the HTTP window.
A later 40-request run had median 8.476 ms and maximum 113.383 ms and passed
TCP/UDP, nonblocking, partial I/O, EOF and signal checks.

The new guest probe caught a handshake readiness error during development:
POLLOUT was reported during SynSent because !may_send was treated as a terminal
write error. That has been corrected: SynSent/SynReceived are pending I/O,
not writable or EOF. The successful nonblocking-connect regression covers it.

A stricter slow-reader assertion showed QEMU could buffer the original 512 KiB
fixture completely. That fixture verified integrity and FIN but did not prove
a blocked write. It was increased to 8 MiB, and the harness now requires the
writer to remain incomplete before testing scheduler responsiveness. This is a
fixture correction; the assertion was retained.

The initial 160-request close-pressure run completed with all bodies intact,
median 11.337 ms and maximum 354.349 ms while timing VMs also ran. These are
liveness/integrity observations, not a controlled throughput benchmark or a
percentage speedup comparison with the input-assisted baseline.

## Final verification

The final network run booted the corrected kernel in QEMU with the
`run-blk-bios` device configuration and a snapshot disk. No install or persistent
disk modification was performed. The harness controls the guest
through its serial PTY and exposes an HMP Unix monitor for status, keyboard
injection and screenshots. This provides repeatable VM interaction without a
desktop display.

| Check | Result |
| --- | --- |
| `make kernel` | Passed; no new Rust warnings. The existing non-root Cargo profile warning remains. |
| `make userspace` | Passed on retry with scoped sandbox escalation for the existing Alpine distrobox container. The first attempt could not access Podman's writable runtime directory. |
| Targeted `net_io_test` build | Passed using static musl, `-Wall -Wextra -Werror`. |
| ISO packing | Passed using the repository image recipe and the staged userspace. The image was packed before the later successful full userspace retry. |
| `make -o twilight-os.iso test-net` | Passed: eight host unit tests, HTTP integrity/liveness, TCP/UDP semantics, signal interruption, slow-peer backpressure and eight existing IPC/poll probes. One file probe explicitly skipped as described below. |
| Timing matrix through `make -o twilight-os.iso test-time` | All seven CPU/accelerator configurations passed: 315 mode/duration cases, 22,225 guest samples, zero failures, early returns or backward-clock observations. |
| Timing harness TERM cleanup | Verified exit status 143 and removal of the test's QEMU process. |
| Source checks | `git diff --check` passed; new/reworked Rust modules formatted; Python and shell syntax checked. |

The new guest probe was built with:

```sh
musl-gcc -static -fno-pie -no-pie -Wall -Wextra -Werror \
  userspace/apps/net_io_test/main.c -o rootfs/bin/net_io_test
```

Final network evidence is in `/tmp/twilight-net-rw_aqlev/` and the complete
command output is `/tmp/twilight-net-final.log`. Results:

- Initial 160 sequential HTTP requests, without injected serial/keyboard input:
  all exact bodies matched; median **9.625 ms**, maximum **108.497 ms**.
- TCP/UDP, blocking/nonblocking connect, partial I/O, EOF, EAGAIN, truncated UDP
  datagrams and SIGUSR1 interruption passed.
- Accept completed before application data arrived. While the accepted read
  blocked, the shell completed 100 ten-ms clockcheck sleeps.
- The **8 MiB** response forced a blocked writer before the host began draining.
  Another 100 sleeps and four HTTP requests completed during backpressure;
  HTTP median **18.669 ms**, maximum **84.826 ms**. The full byte pattern and
  orderly FIN/EOF then matched.
- After teardown, another 100 sleeps and four HTTP requests passed; HTTP median
  **12.013 ms**, maximum **82.899 ms**. Thus the run covered 168 HTTP requests
  and 300 timing sleeps in total.
- `pipe_ipc_test`, `unix_bind_test`, `fdpass_pipe_test`,
  `fdpass_socketpair_test`, `memfd_basic_test`, `memfd_fdpass_test`,
  `memfd_mmap_test` and `poll_test` passed.
- `fdpass_file_test` was **SKIP**, not PASS: its fixed `/var/fdpass.txt` fixture
  cannot be created on the live ISO's read-only `/var`. The harness reports that
  exact open failure separately; it still rejects other probe failures. Run
  this legacy test on an installed writable system to cover file fd-passing.

Timing cells were TCG `core2duo` with 1/4 vCPUs, TCG `qemu64` with 1/4 vCPUs,
TCG `max` with one vCPU, and KVM `host` with 1/4 vCPUs. Each cell completed 45
cases and 3,175 samples. Twilight still executes tasks on the BSP; these vCPU
settings exercise boot/clock configuration, not parallel kernel execution.
Individual logs are `/tmp/twilight-time-final-<accel>-<cpu>-<smp>.log`;
`/tmp/twilight-time-final-summary.json` contains the per-cell results.

The timing matrix ran an image containing the earlier 512 KiB network fixture;
the final network image contains the stricter 8 MiB fixture. **Both images
contain the same corrected kernel**, verified by extraction and SHA-256:

```text
Corrected kernel (build output and both images):
c86075e3e3b56283805a6aa44c6c9c0ca8921b3a041aa40d6e8f3044671971f5

Timing-matrix ISO:
0924c095660bb5e52af30cc3e415a4260db074a6750b281d574d21586dfd89ec

Final network ISO, now twilight-os.iso:
de30351db5082c1372c9b58ae3335095800d35940bd4547a3b0a64074673f4aa
```

The successful full userspace build output is
`/tmp/twilight-httpd-userspace-final.log`. The first attempt is recorded in
`/tmp/twilight-httpd-userspace-build.log`: the sandbox made Podman's runtime
directory read-only. A scoped escalation confirmed the existing Alpine
container was running, and `make userspace` then exited successfully. No new
container was created. The successful image build is recorded in
`/tmp/twilight-httpd-final-build.log`.
The superseded interrupted timing run in `twilight-httpd-time-regression.log`
is not counted as validation: it exposed the cleanup-trap bug corrected above.

Remaining limits: real hardware and PCNET were not exercised; the PCNET fallback
is covered by source review only. Existing listener backlog, SO_ERROR/shutdown
and later DHCP configuration-application limitations remain documented in
`docs/networking.md`. httpd remains a sequential server, so a slow client can
delay another HTTP client even while other OS tasks continue. RTC issue #84
remains separate and unfixed; Internet protocol timing no longer depends on
that CMOS helper. Pre-existing user changes and third-party assets were retained.
