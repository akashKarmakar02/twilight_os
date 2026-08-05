# parse.awk — deterministic percentile summary for clockcheck records.
#
# Reads `iter mode=<m> req_ns=<r> ... delta_ns=<d> lateness_ns=<l> result=<ok|early|backward>`
# lines from stdin and emits one summary line:
#
#   SUMMARY mode=<m> req_ns=<r> iters=<n> \
#     guest_p50_ns=.. guest_p95_ns=.. guest_p99_ns=.. guest_max_ns=.. \
#     lateness_p50_ns=.. lateness_p95_ns=.. lateness_p99_ns=.. lateness_max_ns=.. \
#     early=<e> backward=<b>
#
# Percentiles are computed by sorting the samples with an in-awk insertion
# sort; this keeps the parser deterministic and free of external processes.
# lateness is signed; its percentiles are computed on the signed values.
#
# Usage:
#   awk -f tools/time-regression/parse.awk < serial.log
#   awk -v mode=rel -v req=10000 -f tools/time-regression/parse.awk < recs.txt
#
# When -v mode=/req= are given, only matching records are summarized; otherwise
# all `iter` lines are summarized grouped by (mode, req_ns).

function reset_stats() {
    n = 0
    early = 0
    backward = 0
    delete delta
    delete late
}

function pct(arr, n, p,    i, idx) {
    # p in [0,1]; returns the p-th percentile of the sorted array arr[1..n].
    if (n == 0) return 0
    idx = int(p * (n - 1)) + 1
    if (idx < 1) idx = 1
    if (idx > n) idx = n
    return arr[idx]
}

# Sort a numeric array arr[1..n] in place using insertion sort. n is small
# (<= ~8192 iters per duration), so O(n^2) worst case is acceptable and avoids
# spawning a coprocess per call. For the largest cases this is ~67M comparisons,
# which is still sub-second on any modern host.
function sort_numeric(arr, n,    i, j, key) {
    for (i = 2; i <= n; i++) {
        key = arr[i]
        j = i - 1
        while (j >= 1 && arr[j] > key) {
            arr[j + 1] = arr[j]
            j--
        }
        arr[j + 1] = key
    }
}

function summarize(m, r,    p50, p95, p99, max, lp50, lp95, lp99, lmax) {
    sort_numeric(delta, n)
    sort_numeric(late, n)
    p50 = pct(delta, n, 0.50)
    p95 = pct(delta, n, 0.95)
    p99 = pct(delta, n, 0.99)
    max = (n > 0) ? delta[n] : 0
    lp50 = pct(late, n, 0.50)
    lp95 = pct(late, n, 0.95)
    lp99 = pct(late, n, 0.99)
    lmax = (n > 0) ? late[n] : 0
    printf "SUMMARY mode=%s req_ns=%d iters=%d " \
           "guest_p50_ns=%d guest_p95_ns=%d guest_p99_ns=%d guest_max_ns=%d " \
           "lateness_p50_ns=%d lateness_p95_ns=%d lateness_p99_ns=%d lateness_max_ns=%d " \
           "early=%d backward=%d\n",
           m, r, n, p50, p95, p99, max, lp50, lp95, lp99, lmax, early, backward
}

BEGIN {
    reset_stats()
    cur_mode = ""
    cur_req = ""
}

/^iter / {
    # Parse fields: mode= req_ns= delta_ns= lateness_ns= result=
    m = ""; r = ""; d = ""; l = ""; res = ""
    for (i = 1; i <= NF; i++) {
        if (index($i, "mode=") == 1) m = substr($i, 6)
        else if (index($i, "req_ns=") == 1) r = substr($i, 8)
        else if (index($i, "delta_ns=") == 1) d = substr($i, 10)
        else if (index($i, "lateness_ns=") == 1) l = substr($i, 13)
        else if (index($i, "result=") == 1) res = substr($i, 8)
    }
    if (m == "" || r == "") next

    # Filter if -v mode=/req= given.
    if (mode != "" && m != mode) next
    if (req != "" && r+0 != req+0) next

    # Group boundary.
    if (cur_mode == "") { cur_mode = m; cur_req = r }
    if (m != cur_mode || r != cur_req) {
        summarize(cur_mode, cur_req)
        reset_stats()
        cur_mode = m
        cur_req = r
    }

    n++
    delta[n] = d + 0
    late[n] = l + 0
    if (res == "early") early++
    else if (res == "backward") backward++
    next
}

END {
    if (n > 0) summarize(cur_mode, cur_req)
}
