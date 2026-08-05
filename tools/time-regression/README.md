# time-regression — QEMU clock and wake-latency regression matrix (#64)

An automated host/guest timing harness that boots the Twilight OS live ISO
headless under several QEMU configurations, drives the `clockcheck` guest probe
over serial, and correlates guest `CLOCK_MONOTONIC` samples against **host**
wall time.

This exists to catch clock regressions *before* they land. The previous
`clockcheck` compared `nanosleep` against the same guest clock used to implement
the sleep, so it could report success while both were slow relative to host/QEMU
time. This harness adds an independent host-timed reference.

Related: #62, PR #63.

## Prerequisites

- `qemu-system-x86_64` (any recent version; recorded in every result)
- `awk` (gawk or mawk)
- `/dev/kvm` (optional; KVM cells are skipped with a logged reason when absent)
- A built `twilight-os.iso` (run `make` in the repo root first)

No root is required. The harness never touches the developer's `hdd.img`.

## Safety

The harness is **non-destructive**:

- It boots the live ISO + initramfs with **no disk attached**. `hdd.img` is
  never referenced, mounted, or written.
- Each QEMU run uses `-no-reboot` and is cleaned up via process signals on exit
  (trap on EXIT/INT/TERM). Raw serial logs are preserved under `logs/` for any
  failed cell.
- A per-cell clean-shutdown timeout bounds how long a hung guest can delay the
  run; on timeout the QEMU process is force-killed and the failure is recorded.

## Quick start

From the repo root:

```sh
make                        # builds twilight-os.iso
make test-time              # runs the full matrix against the ISO
```

Or invoke the runner directly:

```sh
tools/time-regression/run.sh twilight-os.iso
```

## Environment variables

| Variable | Default | Meaning |
|---|---|---|
| `TIME_REGRESSION_ITERS` | `100` | Iterations per short duration |
| `TIME_REGRESSION_KVM` | `auto` | `auto` enables KVM cells when `/dev/kvm` is writable; `1`/`0` forces on/off |
| `TIME_REGRESSION_CELLS` | all | Space-separated `accel\|cpu\|smp` cells, e.g. `"tcg\|core2duo\|1 tcg\|qemu64\|1"` |
| `TIME_REGRESSION_TIMEOUT` | `60` | Per-cell clean-shutdown timeout in seconds |

## Matrix

Default cells:

- `-accel tcg -cpu core2duo` with `-smp 1` and `-smp 4`
- `-accel tcg -cpu qemu64` with `-smp 1`
- `-accel tcg -cpu max` with `-smp 1` (best-effort)
- `-enable-kvm -cpu host` with `-smp 1` and `-smp 4` (when KVM is available)

KVM may be skipped with an explicit reason when unavailable (issue non-goal:
KVM indexing is permitted).

## Guest protocol

`clockcheck` is extended to accept nanoseconds, iterations, and a mode, and emits
machine-readable records. See `userspace/apps/clockcheck/src/main.c` for the
full protocol. Modes:

- `rel` — relative `nanosleep(req)`
- `abs` — absolute `clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, now+req)`
- `read` — monotonic read-rate sample during CPU work (non-decrease check)
- `load` — N CPU-load workers contending while the parent sleeps
- `poll` — blocked `poll()` on an empty pipe with timeout = req

Test durations (ns): `10000`, `100000`, `500000`, `1000000`, `2000000`,
`5000000`, `16666667`, `100000000`, `1000000000`.

### Per-iteration record

```text
iter mode=<m> req_ns=<r> start_ns=<s> end_ns=<e> delta_ns=<d> lateness_ns=<l> result=<ok|early|backward>
```

- `lateness_ns` is **signed** `(actual - requested)`, computed in signed
  arithmetic so an early return does not underflow (this fixes the original
  unsigned `meas_ns - req_ns` expression).
- `result`:
  - `early` — a successful uninterrupted sleep returned before its guest
    deadline (`delta < req`). Hard-gate violation.
  - `backward` — a monotonic read went backward (`end < start`). Hard-gate
    violation.
  - `ok` — neither.

### Batch handshake (short durations)

For `req_ns ≤ 500000`, the guest emits `BATCH_START`, runs all iterations with
**no per-iteration serial output**, then emits `BATCH_END`. The host runner
timestamps marker receipt and reports a host-timed envelope. The guest buffers
its per-iteration records and flushes them only after `BATCH_END`, so guest
p50/p95/p99/max are preserved without per-iteration serial latency contaminating
the measurement. UART buffering and emulation overhead dominate 10–500 µs
measurements, so the host reports batch duration/rate **separately** from the
guest percentiles and never claims exact syscall-boundary timestamps.

For longer durations, per-iteration records are printed inline (serial latency
is negligible relative to the sleep).

## Output schema

Each run emits `META` lines (environment/metadata) and one `RESULT` line per
(mode × duration × cell):

```text
META qemu=... machine=q35 commit=<sha> dirty=<n> iso_sha=<sha> accel=<a> cpu=<c> smp=<n>
RESULT accel=<a> cpu=<c> smp=<n> mode=<m> req_ns=<r> iters=<n> \
  host_batch_ns=<ns> host_batch_overhead_ns=<ns> host_rate_ns_per_op=<ns> \
  SUMMARY mode=<m> req_ns=<r> iters=<n> \
  guest_p50_ns=.. guest_p95_ns=.. guest_p99_ns=.. guest_max_ns=.. \
  lateness_p50_ns=.. lateness_p95_ns=.. lateness_p99_ns=.. lateness_max_ns=.. \
  early=<e> backward=<b>
```

Recorded metadata (in every result block): QEMU version, machine type, kernel
commit, dirty-tree state, ISO/build ID (sha256), accelerator, CPU model, SMP
count.

## Thresholds and gates

**Hard gates** (failures regardless of baseline):

1. **Never early** — no successful, uninterrupted sleep returns before its
   selected guest deadline (`early == 0`).
2. **Monotonic non-decrease** — no guest monotonic read goes backward
   (`backward == 0`).
3. **Correct guest clock rate** — guest time rate is compared to host monotonic
   time via the host-timed batch envelope. A guest-only self-comparison is not
   accepted.

**Diagnostic** (until a baseline is recorded):

- Lateness thresholds (p50/p95/p99/max) are reported but not gating. Every
  threshold is defined in terms of signed `actual - requested`, not total
  duration. Once a baseline is captured, these become gates.

## Manual reproduction

To reproduce the `core2duo` defect (or any single cell) by hand:

```sh
qemu-system-x86_64 -M q35 -accel tcg -cpu core2duo -smp 1 -m 1024 \
  -cdrom twilight-os.iso -boot d -serial stdio -display none
```

At the `#` prompt:

```sh
clockcheck rel 10000 100
clockcheck abs 16666667 20
clockcheck read 50000000 10
```

To summarize a saved serial log offline:

```sh
awk -f tools/time-regression/parse.awk < serial.log
```

## Non-goals (from the issue)

- This harness does **not** fix kernel timing.
- It does **not** promise hard real-time scheduling.
- No new procfs/syscall diagnostic interface is added; timer-event and
  context-switch counts are optional/diagnostic until such an interface exists.
