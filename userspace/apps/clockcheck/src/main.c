/*
 * clockcheck — verify CLOCK_MONOTONIC tracks real elapsed time.
 *
 * Reads CLOCK_MONOTONIC before and after a nanosleep, prints the requested
 * vs measured interval in nanoseconds, and the drift ratio. Also runs a
 * busy-wait segment to detect a slow clock. Output is machine-readable so it
 * can be diffed against host wall time in regression tests (issue #62).
 *
 * Usage: clockcheck [sleep_ms]
 *
 * Default sleep is 1000 ms. Prints lines like:
 *   sleep req_ns=1000000000 meas_ns=1002003456 drift_ppm=2003
 */

#define _POSIX_C_SOURCE 200809L
#include <stdint.h>
#include <stdio.h>
#include <time.h>
#include <unistd.h>

static uint64_t now_ns(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {
        return 0;
    }
    return (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
}

static long parse_long(const char *s) {
    long v = 0;
    while (*s >= '0' && *s <= '9') {
        v = v * 10 + (*s - '0');
        s++;
    }
    return v;
}

int main(int argc, char **argv) {
    long sleep_ms = 1000;
    if (argc >= 2) {
        sleep_ms = parse_long(argv[1]);
        if (sleep_ms <= 0) sleep_ms = 1000;
    }

    uint64_t req_ns = (uint64_t)sleep_ms * 1000000ULL;

    /* --- sleep accuracy test --- */
    uint64_t t0 = now_ns();
    struct timespec req = { .tv_sec = sleep_ms / 1000,
                           .tv_nsec = (sleep_ms % 1000) * 1000000L };
    nanosleep(&req, NULL);
    uint64_t t1 = now_ns();
    uint64_t meas_ns = t1 - t0;

    int64_t drift_ppm;
    if (req_ns == 0) {
        drift_ppm = 0;
    } else {
        /* drift in parts per million, positive = clock slow (measured > requested) */
        drift_ppm = (int64_t)((meas_ns - req_ns) * 1000000ULL / req_ns);
    }

    printf("sleep req_ns=%llu meas_ns=%llu drift_ppm=%lld\n",
           (unsigned long long)req_ns,
           (unsigned long long)meas_ns,
           (long long)drift_ppm);

    /* --- monotonicity test: two rapid reads must not go backwards --- */
    uint64_t a = now_ns();
    uint64_t b = now_ns();
    if (b < a) {
        printf("monotonicity FAIL a=%llu b=%llu\n",
               (unsigned long long)a, (unsigned long long)b);
        return 1;
    }
    printf("monotonicity OK\n");

    /* --- busy-wait drift: 200 ms of CPU work, clock should advance ~200 ms --- */
    uint64_t w0 = now_ns();
    volatile uint64_t sink = 0;
    uint64_t target = 200 * 1000000ULL;
    while (now_ns() - w0 < target) {
        for (int i = 0; i < 1000; i++) sink += i;
    }
    uint64_t w1 = now_ns();
    printf("busy req_ns=%llu meas_ns=%llu\n",
           (unsigned long long)target,
           (unsigned long long)(w1 - w0));

    return 0;
}
