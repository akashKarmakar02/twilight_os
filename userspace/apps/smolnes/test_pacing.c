// Host unit tests for smolnes frame-pacing arithmetic (issue #72).
//
// Built with the host cc (not musl-gcc) via `make test`. These pin down the
// pure invariants of checked_add_ns / advance_deadline_checked that the issue
// requires: first-frame init, saturating missed-period arithmetic, rebase on
// large discontinuity, no overflow, no unbounded catch-up.
//
// The clock_gettime-failure and clock_nanosleep-EINTR paths need the kernel
// and are exercised by the main binary at runtime; the *arithmetic* invariants
// are what these tests cover.

#include <assert.h>
#include <stdio.h>
#include <stdint.h>

#include "pacing.h"

static int tests_run = 0;
static int tests_failed = 0;

#define CHECK(cond) do {                                  \
    tests_run++;                                          \
    if (!(cond)) {                                        \
        printf("FAIL: %s (line %d)\n", #cond, __LINE__);  \
        tests_failed++;                                   \
    }                                                     \
} while (0)

static void test_checked_add_normal(void) {
    CHECK(checked_add_ns(0, 1000) == 1000);
    CHECK(checked_add_ns(1000, 2000) == 3000);
    CHECK(checked_add_ns(SMOLNES_NTSC_FRAME_NS, SMOLNES_NTSC_FRAME_NS) ==
          2 * SMOLNES_NTSC_FRAME_NS);
}

static void test_checked_add_saturates(void) {
    // base + add where add would overflow -> UINT64_MAX.
    CHECK(checked_add_ns(UINT64_MAX, 1) == UINT64_MAX);
    CHECK(checked_add_ns(UINT64_MAX - 5, 10) == UINT64_MAX);
    // base itself at max stays at max.
    CHECK(checked_add_ns(UINT64_MAX, 0) == UINT64_MAX);
    // Just below the wrap boundary.
    CHECK(checked_add_ns(UINT64_MAX - SMOLNES_NTSC_FRAME_NS + 1,
                         SMOLNES_NTSC_FRAME_NS) == UINT64_MAX);
}

static void test_advance_not_late_unchanged(void) {
    // Deadline in the future: returned unchanged.
    uint64_t now = 1000000;
    uint64_t dl = now + SMOLNES_NTSC_FRAME_NS;
    CHECK(advance_deadline_checked(dl, now, SMOLNES_NTSC_FRAME_NS) == dl);
}

static void test_advance_exactly_one_period_late(void) {
    // Exactly one period behind: the missed frame at `dl` is skipped and the
    // next deadline lands one period ahead of now (we sleep one period).
    uint64_t dl = 1000000;
    uint64_t now = dl + SMOLNES_NTSC_FRAME_NS;
    uint64_t r = advance_deadline_checked(dl, now, SMOLNES_NTSC_FRAME_NS);
    // missed = lag/period + 1 = 1 + 1 = 2 -> r = dl + 2*period = now + period.
    CHECK(r == dl + 2 * SMOLNES_NTSC_FRAME_NS);
    CHECK(r == now + SMOLNES_NTSC_FRAME_NS);
    CHECK(r > now);
}

static void test_advance_partial_period_late(void) {
    // Less than one period behind: advance by one period (the +1).
    uint64_t dl = 1000000;
    uint64_t now = dl + SMOLNES_NTSC_FRAME_NS / 2;
    uint64_t r = advance_deadline_checked(dl, now, SMOLNES_NTSC_FRAME_NS);
    CHECK(r == dl + SMOLNES_NTSC_FRAME_NS);
    CHECK(r > now);
}

static void test_advance_few_periods_late(void) {
    // 3 periods behind (under the rebase threshold): advance by exactly 3+1=4?
    // lag = 3*period, missed = lag/period + 1 = 3 + 1 = 4. New deadline =
    // dl + 4*period = now + period. Ahead of now by one period.
    uint64_t dl = 1000000;
    uint64_t now = dl + 3 * SMOLNES_NTSC_FRAME_NS;
    uint64_t r = advance_deadline_checked(dl, now, SMOLNES_NTSC_FRAME_NS);
    CHECK(r == dl + 4 * SMOLNES_NTSC_FRAME_NS);
    CHECK(r == now + SMOLNES_NTSC_FRAME_NS);
    CHECK(r > now);
}

static void test_advance_just_under_threshold(void) {
    // Just under MAX_CATCHUP_PERIODS behind: bounded advance, not rebase.
    uint64_t dl = 1000000;
    uint64_t lag = (SMOLNES_MAX_CATCHUP_PERIODS - 1) * SMOLNES_NTSC_FRAME_NS;
    uint64_t now = dl + lag;
    uint64_t r = advance_deadline_checked(dl, now, SMOLNES_NTSC_FRAME_NS);
    // Should be ahead of now (bounded path), not rebased to now+period.
    // missed = lag/period + 1 = (MAX-1) + 1 = MAX. r = dl + MAX*period.
    CHECK(r == dl + SMOLNES_MAX_CATCHUP_PERIODS * SMOLNES_NTSC_FRAME_NS);
    CHECK(r == now + SMOLNES_NTSC_FRAME_NS);
}

static void test_advance_over_threshold_rebases(void) {
    // Over MAX_CATCHUP_PERIODS behind: rebase to now+period, not advance.
    uint64_t dl = 1000000;
    uint64_t lag = SMOLNES_MAX_CATCHUP_PERIODS * SMOLNES_NTSC_FRAME_NS;
    uint64_t now = dl + lag;
    uint64_t r = advance_deadline_checked(dl, now, SMOLNES_NTSC_FRAME_NS);
    CHECK(r == checked_add_ns(now, SMOLNES_NTSC_FRAME_NS));
    CHECK(r == now + SMOLNES_NTSC_FRAME_NS);
    // At exactly the threshold the bounded advance would also give now+period,
    // so the rebase distinction is exercised by test_advance_huge_discontinuity.
}

static void test_advance_huge_discontinuity_rebases(void) {
    // A VM pause equivalent to hours: must rebase, not advance through millions
    // of phantom periods (no unbounded loop, no near-UINT64_MAX deadline).
    uint64_t dl = 1000000;
    uint64_t now = dl + (uint64_t)3600 * 1000000000ull;  // 1 hour later
    uint64_t r = advance_deadline_checked(dl, now, SMOLNES_NTSC_FRAME_NS);
    CHECK(r == now + SMOLNES_NTSC_FRAME_NS);
    CHECK(r < now + 2 * SMOLNES_NTSC_FRAME_NS);  // exactly one period ahead
}

static void test_advance_backward_time_rebases(void) {
    // now < deadline but by more than the threshold: this is the "deadline far
    // in the future" case, which is NOT late, so it returns unchanged. A true
    // backward clock jump (now << dl) still returns dl unchanged because the
    // monotonic clock shouldn't go backward; if it did, dl > now => not late.
    uint64_t dl = 1000000000ull;
    uint64_t now = 1000;  // way behind dl
    uint64_t r = advance_deadline_checked(dl, now, SMOLNES_NTSC_FRAME_NS);
    CHECK(r == dl);  // not late (dl > now), unchanged
}

static void test_advance_no_overflow_on_rebase(void) {
    // Rebase path with now near UINT64_MAX must saturate, not wrap.
    uint64_t dl = 1000;
    uint64_t now = UINT64_MAX - 10;
    uint64_t r = advance_deadline_checked(dl, now, SMOLNES_NTSC_FRAME_NS);
    CHECK(r == UINT64_MAX);  // saturated
}

static void test_first_frame_init_pattern(void) {
    // Simulates the !pacing_started branch in deobfuscated.c:
    //   next_present_ns = checked_add_ns(now, period);
    uint64_t now = 5000000;
    uint64_t next = checked_add_ns(now, SMOLNES_NTSC_FRAME_NS);
    CHECK(next == now + SMOLNES_NTSC_FRAME_NS);
    CHECK(next > now);
    // Subsequent frame: advance from next against a now that is still before it.
    uint64_t now2 = now + 1000;  // some work done, still before deadline
    uint64_t next2 = advance_deadline_checked(next, now2, SMOLNES_NTSC_FRAME_NS);
    CHECK(next2 == next);  // not late yet
}

static void test_no_alternation_sequence(void) {
    // Simulate a steady run: each frame, now advances by exactly one period
    // after the deadline. The deadline should advance by exactly one period
    // each time, with no short/long alternation.
    uint64_t now = 1000000;
    uint64_t dl = checked_add_ns(now, SMOLNES_NTSC_FRAME_NS);
    uint64_t prev_dl = dl;
    for (int i = 0; i < 50; i++) {
        // Emulation+present consumed up to the deadline; "now" at next pacing
        // point is approximately the previous deadline.
        now = dl;
        dl = advance_deadline_checked(dl, now, SMOLNES_NTSC_FRAME_NS);
        uint64_t step = dl - prev_dl;
        CHECK(step == SMOLNES_NTSC_FRAME_NS);  // constant stride, no alternation
        prev_dl = dl;
    }
}

int main(void) {
    test_checked_add_normal();
    test_checked_add_saturates();
    test_advance_not_late_unchanged();
    test_advance_exactly_one_period_late();
    test_advance_partial_period_late();
    test_advance_few_periods_late();
    test_advance_just_under_threshold();
    test_advance_over_threshold_rebases();
    test_advance_huge_discontinuity_rebases();
    test_advance_backward_time_rebases();
    test_advance_no_overflow_on_rebase();
    test_first_frame_init_pattern();
    test_no_alternation_sequence();

    printf("pacing tests: %d run, %d failed\n", tests_run, tests_failed);
    return tests_failed ? 1 : 0;
}
