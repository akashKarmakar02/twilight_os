// smolnes frame-pacing helpers.
//
// Pure, side-effect-free arithmetic for accumulated-absolute-deadline frame
// pacing. Kept in a separate header so it can be unit-tested on the host
// without a framebuffer or the kernel (see test_pacing.c).
//
// Invariants enforced here (see issue #72):
//   - No integer-millisecond truncation controls cadence; everything is ns.
//   - Each deadline is derived from the previous deadline, not from a
//     timestamp taken before sleeping -> no short/long alternation, no drift.
//   - A late frame does not permanently phase-shift later deadlines: catch-up
//     is bounded; beyond a threshold we rebase to now+period.
//   - Overload causes bounded lateness/skipping, not an unbounded catch-up loop.
//   - Saturating arithmetic: the next deadline can never overflow past UINT64_MAX.

#ifndef SMOLNES_PACING_H
#define SMOLNES_PACING_H

#include <stdint.h>

// NTSC NES frame period.
//
// The emulator is NTSC-only (262 scanlines). The master clock is 21.477272 MHz;
// one frame = 262 scanlines * 341 dots/scanline * 3 CPU cycles/dot, giving a
// frame rate of 21477272 / (262*341*3) ~= 60.0988 Hz. Frame period in ns is
// round(1e9 / fps) = 16639268 ns.
//
// PAL selection is not required until PAL emulation exists.
#define SMOLNES_NTSC_FRAME_NS 16639268ull

// Beyond this many missed periods, rebase the deadline to now+period instead of
// advancing through phantom periods. ~6 frames (~100 ms): large enough that
// normal scheduler jitter never triggers it, small enough that a VM pause or
// debugger stop doesn't cause a multi-second catch-up burst or pin the
// deadline near UINT64_MAX.
#define SMOLNES_MAX_CATCHUP_PERIODS 6ull

// Saturating add: returns base + add, or UINT64_MAX on overflow so the next
// deadline can never wrap around to a small value.
static uint64_t checked_add_ns(uint64_t base, uint64_t add) {
    uint64_t r = base + add;
    return r < base ? UINT64_MAX : r;
}

// Advance an absolute presentation deadline to the next unexpired boundary.
//
//   next_deadline: the previous target presentation time.
//   now:           current CLOCK_MONOTONIC timestamp.
//   period_ns:     frame period (SMOLNES_NTSC_FRAME_NS).
//
// If the deadline has not yet passed, it is returned unchanged. If it has
// passed, the deadline is advanced by whole periods until it lies ahead of
// `now`. The advance is computed by division (O(1)), not a loop, and is
// bounded: if the lag exceeds SMOLNES_MAX_CATCHUP_PERIODS periods (a large
// time discontinuity such as a VM pause), the deadline rebases to
// checked_add_ns(now, period_ns) instead of advancing through potentially
// thousands of phantom periods.
//
// The "+1" in the bounded path ensures the new deadline lands strictly ahead
// of `now` (a deadline <= now would cause an immediate no-sleep present rather
// than a one-frame wait).
static uint64_t advance_deadline_checked(uint64_t next_deadline, uint64_t now,
                                         uint64_t period_ns) {
    if (next_deadline > now)
        return next_deadline;

    uint64_t lag = now - next_deadline;
    if (lag >= period_ns * SMOLNES_MAX_CATCHUP_PERIODS)
        return checked_add_ns(now, period_ns);

    uint64_t missed = lag / period_ns + 1;
    return checked_add_ns(next_deadline, missed * period_ns);
}

#endif // SMOLNES_PACING_H
