/*
 * clockcheck — guest-side timing probe for the #64 regression matrix.
 *
 * Compares CLOCK_MONOTONIC against itself across sleep/wait primitives and
 * emits machine-readable records that the host runner
 * (tools/time-regression/run.sh) correlates against host wall time. The guest
 * never claims exact per-syscall host timestamps: for short durations it uses a
 * batch handshake (BATCH_START ... silent iterations ... BATCH_END) so the host
 * can timestamp marker receipt and report a timing envelope, while the guest
 * retains and flushes its per-iteration distribution afterwards.
 *
 * Backward compatible with the original `clockcheck [sleep_ms]` form, which
 * keeps its human-readable output. The extended protocol is:
 *
 *   clockcheck <mode> <req_ns> <iterations> [extra]
 *
 * Modes:
 *   rel   — relative nanosleep(req)
 *   abs   — absolute clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, now+req)
 *   read  — monotonic read-rate sample during CPU work (non-decrease check)
 *   load  — N CPU-load workers (extra=N) contending while parent sleeps rel
 *   poll  — blocked poll() on an empty pipe with timeout = req
 *
 * Per-iteration record (one line):
 *   iter mode=<m> req_ns=<r> start_ns=<s> end_ns=<e> delta_ns=<d>
 *        lateness_ns=<l> result=<ok|early|backward>
 *
 * lateness_ns is signed (actual - requested), computed in signed arithmetic so
 * an early return does not underflow. `early` means a successful uninterrupted
 * sleep returned before its guest deadline (delta < req). `backward` means a
 * monotonic read went backwards (end < start). Both are hard-gate violations.
 *
 * For short durations (<= BATCH_THRESHOLD_NS) the per-iteration records are
 * buffered and flushed only after BATCH_END, so per-iteration serial latency
 * does not contaminate the measurement. The host runner timestamps marker
 * receipt for the host-timed envelope.
 */

#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define NS_PER_SEC 1000000000ULL
/* Below this, use the host-timed batch handshake instead of inline output. */
#define BATCH_THRESHOLD_NS 500000ULL
#define MAX_ITERS 8192

/* result codes */
enum { RES_OK = 0, RES_EARLY = 1, RES_BACKWARD = 2 };

struct rec {
    uint64_t start_ns;
    uint64_t end_ns;
    uint64_t delta_ns;
    int64_t lateness_ns;
    int result;
};

static uint64_t now_ns(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {
        return 0;
    }
    return (uint64_t)ts.tv_sec * NS_PER_SEC + (uint64_t)ts.tv_nsec;
}

static long parse_long(const char *s) {
    long v = 0;
    while (*s >= '0' && *s <= '9') {
        v = v * 10 + (*s - '0');
        s++;
    }
    return v;
}

static uint64_t parse_u64(const char *s) {
    uint64_t v = 0;
    while (*s >= '0' && *s <= '9') {
        v = v * 10ULL + (uint64_t)(*s - '0');
        s++;
    }
    return v;
}

static const char *result_str(int r) {
    switch (r) {
    case RES_EARLY:   return "early";
    case RES_BACKWARD: return "backward";
    default:        return "ok";
    }
}

static void ns_to_timespec(uint64_t ns, struct timespec *t) {
    t->tv_sec = (time_t)(ns / NS_PER_SEC);
    t->tv_nsec = (long)(ns % NS_PER_SEC);
}

/* Classify one measured interval. req==0 means "not a sleep" (read mode). */
static int classify(uint64_t start, uint64_t end, uint64_t req, int is_sleep) {
    if (end < start) {
        return RES_BACKWARD;
    }
    if (is_sleep && end - start < req) {
        return RES_EARLY;
    }
    return RES_OK;
}

static void print_rec(const char *mode, uint64_t req, const struct rec *r) {
    printf("iter mode=%s req_ns=%llu start_ns=%llu end_ns=%llu "
           "delta_ns=%llu lateness_ns=%lld result=%s\n",
           mode, (unsigned long long)req,
           (unsigned long long)r->start_ns,
           (unsigned long long)r->end_ns,
           (unsigned long long)r->delta_ns,
           (long long)r->lateness_ns, result_str(r->result));
}

static void flush_recs(const char *mode, uint64_t req,
                      const struct rec *recs, unsigned n) {
    for (unsigned i = 0; i < n; i++) {
        print_rec(mode, req, &recs[i]);
    }
}

/* --- modes ---------------------------------------------------------------- */

static void do_rel(uint64_t req, unsigned iters, int batch,
                   struct rec *recs) {
    struct timespec ts;
    ns_to_timespec(req, &ts);
    for (unsigned i = 0; i < iters; i++) {
        uint64_t s = now_ns();
        nanosleep(&ts, NULL);
        uint64_t e = now_ns();
        struct rec r = { s, e, e - s, (int64_t)(e - s) - (int64_t)req,
                         classify(s, e, req, 1) };
        if (batch) recs[i] = r; else print_rec("rel", req, &r);
    }
}

static void do_abs(uint64_t req, unsigned iters, int batch,
                   struct rec *recs) {
    for (unsigned i = 0; i < iters; i++) {
        uint64_t s = now_ns();
        uint64_t deadline = s + req;
        struct timespec dl;
        ns_to_timespec(deadline, &dl);
        clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &dl, NULL);
        uint64_t e = now_ns();
        struct rec r = { s, e, e - s, (int64_t)(e - s) - (int64_t)req,
                         classify(s, e, req, 1) };
        if (batch) recs[i] = r; else print_rec("abs", req, &r);
    }
}

static void do_read(uint64_t req, unsigned iters, int batch,
                    struct rec *recs) {
    /* Sample CLOCK_MONOTONIC rapidly during ~req of CPU work. Assert
     * non-decrease across all samples in the iteration. */
    for (unsigned i = 0; i < iters; i++) {
        uint64_t s = now_ns();
        uint64_t prev = s;
        int backward = 0;
        volatile uint64_t sink = 0;
        uint64_t samples = 0;
        while (now_ns() - s < req) {
            uint64_t cur = now_ns();
            if (cur < prev) backward = 1;
            prev = cur;
            sink += samples;
            samples++;
        }
        uint64_t e = now_ns();
        struct rec r = { s, e, e - s, (int64_t)(e - s) - (int64_t)req,
                         backward ? RES_BACKWARD : RES_OK };
        (void)samples;
        if (batch) recs[i] = r; else print_rec("read", req, &r);
    }
}

static void do_poll(uint64_t req, unsigned iters, int batch,
                    struct rec *recs) {
    /* Blocked poll on an empty pipe: no writer ever writes, so poll blocks
     * until the timeout elapses. timeout is millisecond-granular, so sub-ms
     * requests return immediately — that limitation is part of what we measure. */
    int pfd[2];
    if (pipe(pfd) != 0) {
        printf("poll setup_error errno=%d\n", errno);
        return;
    }
    int timeout_ms = (int)(req / 1000000ULL);
    for (unsigned i = 0; i < iters; i++) {
        struct pollfd pf = { .fd = pfd[0], .events = POLLIN };
        uint64_t s = now_ns();
        poll(&pf, 1, timeout_ms);
        uint64_t e = now_ns();
        struct rec r = { s, e, e - s, (int64_t)(e - s) - (int64_t)req,
                         classify(s, e, req, 1) };
        if (batch) recs[i] = r; else print_rec("poll", req, &r);
    }
    close(pfd[0]);
    close(pfd[1]);
}

static void do_load(uint64_t req, unsigned iters, int batch,
                    struct rec *recs, unsigned workers) {
    /* Fork `workers` children that spin on CPU. Parent runs rel sleeps under
     * contention, then reaps the children. */
    pid_t pids[256];
    if (workers > 256) workers = 256;
    for (unsigned w = 0; w < workers; w++) {
        pid_t pid = fork();
        if (pid == 0) {
            volatile uint64_t spin = 0;
            while (1) spin++;
        } else if (pid > 0) {
            pids[w] = pid;
        }
    }
    struct timespec ts;
    ns_to_timespec(req, &ts);
    for (unsigned i = 0; i < iters; i++) {
        uint64_t s = now_ns();
        nanosleep(&ts, NULL);
        uint64_t e = now_ns();
        struct rec r = { s, e, e - s, (int64_t)(e - s) - (int64_t)req,
                         classify(s, e, req, 1) };
        if (batch) recs[i] = r; else print_rec("load", req, &r);
    }
    for (unsigned w = 0; w < workers; w++) {
        kill(pids[w], SIGKILL);
        waitpid(pids[w], NULL, 0);
    }
}

/* --- new-protocol dispatch ------------------------------------------------ */

static int is_all_digits(const char *s) {
    if (!s || !*s) return 0;
    while (*s) {
        if (*s < '0' || *s > '9') return 0;
        s++;
    }
    return 1;
}

static int run_protocol(const char *mode, uint64_t req, unsigned iters,
                        unsigned extra) {
    int batch = (req <= BATCH_THRESHOLD_NS) && iters <= MAX_ITERS;
    static struct rec recs[MAX_ITERS];

    if (iters > MAX_ITERS) iters = MAX_ITERS;
    if (iters == 0) iters = 1;

    if (batch) {
        printf("BATCH_START mode=%s req_ns=%llu iters=%u\n",
               mode, (unsigned long long)req, iters);
        fflush(stdout);
    }

    if (strcmp(mode, "rel") == 0) {
        do_rel(req, iters, batch, recs);
    } else if (strcmp(mode, "abs") == 0) {
        do_abs(req, iters, batch, recs);
    } else if (strcmp(mode, "read") == 0) {
        do_read(req, iters, batch, recs);
    } else if (strcmp(mode, "poll") == 0) {
        do_poll(req, iters, batch, recs);
    } else if (strcmp(mode, "load") == 0) {
        do_load(req, iters, batch, recs, extra);
    } else {
        printf("error unknown_mode=%s\n", mode);
        printf("DONE\n");
        fflush(stdout);
        return 1;
    }

    if (batch) {
        printf("BATCH_END\n");
        fflush(stdout);
        flush_recs(mode, req, recs, iters);
        fflush(stdout);
    }

    printf("DONE\n");
    fflush(stdout);
    return 0;
}

/* --- legacy back-compatible form ----------------------------------------- */

static int run_legacy(long sleep_ms) {
    uint64_t req_ns = (uint64_t)sleep_ms * 1000000ULL;

    uint64_t t0 = now_ns();
    struct timespec req = { .tv_sec = sleep_ms / 1000,
                            .tv_nsec = (sleep_ms % 1000) * 1000000L };
    nanosleep(&req, NULL);
    uint64_t t1 = now_ns();
    uint64_t meas_ns = t1 - t0;

    /* drift in parts per million, positive = clock slow (measured > requested).
     * Signed arithmetic: an early return no longer underflows. */
    int64_t lateness = (int64_t)meas_ns - (int64_t)req_ns;
    int64_t drift_ppm = (req_ns == 0) ? 0 : lateness * 1000000LL / (int64_t)req_ns;

    printf("sleep req_ns=%llu meas_ns=%llu drift_ppm=%lld\n",
           (unsigned long long)req_ns, (unsigned long long)meas_ns,
           (long long)drift_ppm);

    uint64_t a = now_ns();
    uint64_t b = now_ns();
    if (b < a) {
        printf("monotonicity FAIL a=%llu b=%llu\n",
               (unsigned long long)a, (unsigned long long)b);
        return 1;
    }
    printf("monotonicity OK\n");

    uint64_t w0 = now_ns();
    volatile uint64_t sink = 0;
    uint64_t target = 200 * 1000000ULL;
    while (now_ns() - w0 < target) {
        for (int i = 0; i < 1000; i++) sink += i;
    }
    uint64_t w1 = now_ns();
    printf("busy req_ns=%llu meas_ns=%llu\n",
           (unsigned long long)target, (unsigned long long)(w1 - w0));

    return 0;
}

int main(int argc, char **argv) {
    /* Legacy form: `clockcheck` or `clockcheck <sleep_ms>` (single integer). */
    if (argc <= 1 || (argc == 2 && is_all_digits(argv[1]))) {
        long sleep_ms = 1000;
        if (argc >= 2) {
            sleep_ms = parse_long(argv[1]);
            if (sleep_ms <= 0) sleep_ms = 1000;
        }
        return run_legacy(sleep_ms);
    }

    /* Extended protocol: `clockcheck <mode> <req_ns> <iters> [extra]`. */
    const char *mode = argv[1];
    uint64_t req = (argc >= 3) ? parse_u64(argv[2]) : 0;
    unsigned iters = (argc >= 4) ? (unsigned)parse_long(argv[3]) : 100;
    unsigned extra = (argc >= 5) ? (unsigned)parse_long(argv[4]) : 0;
    if (req == 0) req = 1000000ULL;
    return run_protocol(mode, req, iters, extra);
}
