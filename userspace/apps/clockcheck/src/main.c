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
 *   sigint — relative nanosleep interrupted by a self-sent SIGUSR1 after ~req/2,
 *            verifying -EINTR and a non-zero remainder (req - elapsed). Emits one
 *            record per iteration with rc, errno and rem_ns.
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

/* Read CLOCK_MONOTONIC. Returns 1 on success and writes ns to *out; returns
 * 0 on failure. On failure the caller must emit a clock_error record and abort
 * the mode rather than recording a zero timestamp, which would be misclassified
 * as a backward-clock violation. */
static int now_ns(uint64_t *out) {
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {
        return 0;
    }
    *out = (uint64_t)ts.tv_sec * NS_PER_SEC + (uint64_t)ts.tv_nsec;
    return 1;
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

/* Emit a clock_error record and return 1 from the caller to abort the mode.
 * A failed clock read must not be recorded as a zero timestamp, since that
 * would be misclassified as a backward-clock hard-gate violation. */
#define CLOCK_ERROR(mode) do { \
    printf("clock_error mode=%s errno=%d\n", (mode), errno); \
    fflush(stdout); \
} while (0)

static void do_rel(uint64_t req, unsigned iters, int batch,
                   struct rec *recs) {
    struct timespec ts;
    ns_to_timespec(req, &ts);
    for (unsigned i = 0; i < iters; i++) {
        uint64_t s, e;
        if (!now_ns(&s)) { CLOCK_ERROR("rel"); continue; }
        int rc = nanosleep(&ts, NULL);
        if (!now_ns(&e)) { CLOCK_ERROR("rel"); continue; }
        /* rc != 0 means the sleep was interrupted (EINTR); do not gate an
         * interrupted sleep as an early wakeup. */
        int is_sleep = (rc == 0);
        struct rec r = { s, e, e - s, (int64_t)(e - s) - (int64_t)req,
                         classify(s, e, req, is_sleep) };
        if (batch) recs[i] = r; else print_rec("rel", req, &r);
    }
}

static void do_abs(uint64_t req, unsigned iters, int batch,
                   struct rec *recs) {
    for (unsigned i = 0; i < iters; i++) {
        uint64_t s, e;
        if (!now_ns(&s)) { CLOCK_ERROR("abs"); continue; }
        uint64_t deadline = s + req;
        struct timespec dl;
        ns_to_timespec(deadline, &dl);
        /* clock_nanosleep returns the error number directly (not via errno). */
        int rc = clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &dl, NULL);
        if (!now_ns(&e)) { CLOCK_ERROR("abs"); continue; }
        int is_sleep = (rc == 0);
        struct rec r = { s, e, e - s, (int64_t)(e - s) - (int64_t)req,
                         classify(s, e, req, is_sleep) };
        if (batch) recs[i] = r; else print_rec("abs", req, &r);
    }
}

static void do_read(uint64_t req, unsigned iters, int batch,
                    struct rec *recs) {
    /* Sample CLOCK_MONOTONIC rapidly during ~req of CPU work. Assert
     * non-decrease across all samples in the iteration. */
    for (unsigned i = 0; i < iters; i++) {
        uint64_t s, e;
        if (!now_ns(&s)) { CLOCK_ERROR("read"); continue; }
        uint64_t prev = s;
        int backward = 0;
        volatile uint64_t sink = 0;
        uint64_t samples = 0;
        uint64_t cur;
        while (now_ns(&cur) && cur - s < req) {
            if (cur < prev) backward = 1;
            prev = cur;
            sink += samples;
            samples++;
        }
        if (!now_ns(&e)) { CLOCK_ERROR("read"); continue; }
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
    /* Sub-ms requests cannot be expressed as a poll timeout; do not gate them
     * as early wakeups, since the immediate return is a granularity artifact
     * rather than a missed deadline. */
    int gate_early = (timeout_ms > 0);
    for (unsigned i = 0; i < iters; i++) {
        struct pollfd pf = { .fd = pfd[0], .events = POLLIN };
        uint64_t s, e;
        if (!now_ns(&s)) { CLOCK_ERROR("poll"); continue; }
        int rc = poll(&pf, 1, timeout_ms);
        if (!now_ns(&e)) { CLOCK_ERROR("poll"); continue; }
        /* rc == 0 means the timeout elapsed (successful wait). rc > 0 means a
         * fd was ready (unexpected here). rc < 0 is EINTR. Only a clean
         * timeout (rc == 0) is gated as a sleep. */
        int is_sleep = (rc == 0) && gate_early;
        struct rec r = { s, e, e - s, (int64_t)(e - s) - (int64_t)req,
                         classify(s, e, req, is_sleep) };
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
    unsigned spawned = 0;
    if (workers > 256) workers = 256;
    for (unsigned w = 0; w < workers; w++) {
        pid_t pid = fork();
        if (pid == 0) {
            volatile uint64_t spin = 0;
            while (1) spin++;
        } else if (pid > 0) {
            pids[spawned++] = pid;
        } else {
            printf("load fork_error errno=%d\n", errno);
        }
    }
    struct timespec ts;
    ns_to_timespec(req, &ts);
    for (unsigned i = 0; i < iters; i++) {
        uint64_t s, e;
        if (!now_ns(&s)) { CLOCK_ERROR("load"); continue; }
        int rc = nanosleep(&ts, NULL);
        if (!now_ns(&e)) { CLOCK_ERROR("load"); continue; }
        int is_sleep = (rc == 0);
        struct rec r = { s, e, e - s, (int64_t)(e - s) - (int64_t)req,
                         classify(s, e, req, is_sleep) };
        if (batch) recs[i] = r; else print_rec("load", req, &r);
    }
    for (unsigned w = 0; w < spawned; w++) {
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

/* sigint mode: verify that a signal interrupts nanosleep with -EINTR and a
 * non-zero remainder. A child forks, sleeps for ~req/2, then sends SIGUSR1 to
 * the parent. The parent's nanosleep(req, &rem) must return -1/EINTR with
 * rem roughly (req - elapsed). Each iteration emits:
 *   sigint iter=<i> rc=<rc> errno=<e> rem_ns=<r> elapsed_ns=<el>
 *                 verdict=<ok|no_eintr|zero_rem|rem_too_big|rem_too_small>
 * `ok` requires rc==-1, errno==EINTR, and 0 < rem_ns <= req with rem within
 * [elapsed - slack, req]. rem_too_small/rem_too_big flag a wrong remainder. */
static volatile sig_atomic_t got_sig;

static void sigusr1_handler(int sig) {
    (void)sig;
    got_sig = 1;
}

static void do_sigint(uint64_t req, unsigned iters) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = sigusr1_handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = 0; /* no SA_RESTART: interrupt the sleep */
    sigaction(SIGUSR1, &sa, NULL);

    struct timespec ts;
    ns_to_timespec(req, &ts);

    for (unsigned i = 0; i < iters; i++) {
        pid_t child = fork();
        if (child == 0) {
            /* Child: wait ~req/2 then signal the parent. */
            struct timespec half;
            ns_to_timespec(req / 2, &half);
            nanosleep(&half, NULL);
            kill(getppid(), SIGUSR1);
            _exit(0);
        } else if (child < 0) {
            printf("sigint iter=%u fork_error errno=%d\n", i, errno);
            continue;
        }

        got_sig = 0;
        uint64_t s, e;
        if (!now_ns(&s)) { CLOCK_ERROR("sigint"); continue; }
        struct timespec rem = { 0, 0 };
        int rc = nanosleep(&ts, &rem);
        int saved_errno = errno;
        if (!now_ns(&e)) { CLOCK_ERROR("sigint"); continue; }
        uint64_t elapsed = e - s;
        uint64_t rem_ns = (uint64_t)rem.tv_sec * NS_PER_SEC + (uint64_t)rem.tv_nsec;

        const char *verdict;
        if (rc != -1 || saved_errno != EINTR) {
            verdict = "no_eintr";
        } else if (rem_ns == 0) {
            verdict = "zero_rem";
        } else if (rem_ns > req) {
            verdict = "rem_too_big";
        } else {
            /* Remainder should be roughly req - elapsed. Allow generous slack
             * for scheduling/signal-delivery latency on each side. */
            uint64_t slack = req / 4 + 5 * 1000000ULL;
            uint64_t lo = (elapsed > slack) ? (elapsed - slack) : 0;
            if (rem_ns < lo) {
                verdict = "rem_too_small";
            } else {
                verdict = "ok";
            }
        }

        printf("sigint iter=%u rc=%d errno=%d rem_ns=%llu elapsed_ns=%llu "
               "got_sig=%d verdict=%s\n",
               i, rc, saved_errno, (unsigned long long)rem_ns,
               (unsigned long long)elapsed, (int)got_sig, verdict);
        fflush(stdout);

        /* Reap the interrupter child. */
        waitpid(child, NULL, 0);
    }
}

static int run_protocol(const char *mode, uint64_t req, unsigned iters,
                        unsigned extra) {
    static struct rec recs[MAX_ITERS];

    if (iters > MAX_ITERS) iters = MAX_ITERS;
    if (iters == 0) iters = 1;

    /* Validate the mode before emitting BATCH_START so an unknown mode does not
     * leave the host waiting for a BATCH_END that never arrives. */
    int valid_mode = (strcmp(mode, "rel") == 0 || strcmp(mode, "abs") == 0 ||
                      strcmp(mode, "read") == 0 || strcmp(mode, "poll") == 0 ||
                      strcmp(mode, "load") == 0 || strcmp(mode, "sigint") == 0);
    if (!valid_mode) {
        printf("error unknown_mode=%s\n", mode);
        printf("DONE\n");
        fflush(stdout);
        return 1;
    }

    /* sigint emits its own per-iteration records (rc/errno/rem) and is never
     * batched, since the interrupt timing is the measurement. */
    if (strcmp(mode, "sigint") == 0) {
        if (req < 20 * 1000000ULL) {
            /* Need enough room for a req/2 child delay plus signal latency. */
            req = 20 * 1000000ULL;
        }
        do_sigint(req, iters);
        printf("DONE\n");
        fflush(stdout);
        return 0;
    }

    int batch = (req <= BATCH_THRESHOLD_NS);

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

    uint64_t t0, t1;
    if (!now_ns(&t0)) {
        printf("clock_error errno=%d\n", errno);
        return 1;
    }
    struct timespec req = { .tv_sec = sleep_ms / 1000,
                            .tv_nsec = (sleep_ms % 1000) * 1000000L };
    nanosleep(&req, NULL);
    if (!now_ns(&t1)) {
        printf("clock_error errno=%d\n", errno);
        return 1;
    }
    uint64_t meas_ns = t1 - t0;

    /* drift in parts per million, positive = clock slow (measured > requested).
     * Signed arithmetic: an early return no longer underflows. */
    int64_t lateness = (int64_t)meas_ns - (int64_t)req_ns;
    int64_t drift_ppm = (req_ns == 0) ? 0 : lateness * 1000000LL / (int64_t)req_ns;

    printf("sleep req_ns=%llu meas_ns=%llu drift_ppm=%lld\n",
           (unsigned long long)req_ns, (unsigned long long)meas_ns,
           (long long)drift_ppm);

    uint64_t a, b;
    if (!now_ns(&a)) { printf("clock_error errno=%d\n", errno); return 1; }
    if (!now_ns(&b)) { printf("clock_error errno=%d\n", errno); return 1; }
    if (b < a) {
        printf("monotonicity FAIL a=%llu b=%llu\n",
               (unsigned long long)a, (unsigned long long)b);
        return 1;
    }
    printf("monotonicity OK\n");

    uint64_t w0, w1, cur;
    if (!now_ns(&w0)) { printf("clock_error errno=%d\n", errno); return 1; }
    volatile uint64_t sink = 0;
    uint64_t target = 200 * 1000000ULL;
    while (now_ns(&cur) && cur - w0 < target) {
        for (int i = 0; i < 1000; i++) sink += i;
    }
    if (!now_ns(&w1)) { printf("clock_error errno=%d\n", errno); return 1; }
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
