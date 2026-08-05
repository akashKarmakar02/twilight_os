#!/usr/bin/env bash
# tools/time-regression/run.sh — host/guest timing regression matrix for #64.
#
# Boots the Twilight OS live ISO headless under several QEMU configurations,
# drives the `clockcheck` guest probe over serial, and correlates guest
# CLOCK_MONOTONIC samples against host wall time. Non-destructive: it never
# touches the developer's hdd.img — it boots the ISO + initramfs with no disk.
#
# Usage:
#   tools/time-regression/run.sh [twilight-os.iso]
#
# Environment:
#   TIME_REGRESSION_ITERS   iterations per short duration (default 100)
#   TIME_REGRESSION_KVM     "auto"|"1"|"0" (default auto)
#   TIME_REGRESSION_CELLS   space-separated "accel|cpu|smp" cells (default: all)
#   TIME_REGRESSION_TIMEOUT per-cell clean-shutdown timeout, seconds (default 60)
#
# Output: RESULT/META lines on stdout; raw serial logs under logs/.
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
LOG_DIR="$SCRIPT_DIR/logs"
mkdir -p "$LOG_DIR"

ITERS="${TIME_REGRESSION_ITERS:-100}"
KVM_MODE="${TIME_REGRESSION_KVM:-auto}"
CELL_TIMEOUT="${TIME_REGRESSION_TIMEOUT:-60}"

ISO="${1:-$REPO_ROOT/twilight-os.iso}"
if [ ! -f "$ISO" ]; then
    echo "error: ISO not found: $ISO" >&2
    echo "hint: run 'make' first to build twilight-os.iso" >&2
    exit 2
fi

ISO_SHA="$(sha256sum "$ISO" | awk '{print $1}')"
QEMU_BIN="qemu-system-x86_64"
if ! command -v "$QEMU_BIN" >/dev/null 2>&1; then
    echo "error: $QEMU_BIN not found on PATH" >&2
    exit 2
fi
QEMU_VERSION="$("$QEMU_BIN" --version | head -1)"
COMMIT="$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
DIRTY="$(git -C "$REPO_ROOT" status --porcelain 2>/dev/null | wc -l | tr -d ' ')"
[ -z "$DIRTY" ] && DIRTY=0

# Must track the guest's BATCH_THRESHOLD_NS in userspace/apps/clockcheck/src/main.c.
BATCH_THRESHOLD_NS=500000

# Durations from the issue (nanoseconds).
DURATIONS=(10000 100000 500000 1000000 2000000 5000000 16666667 100000000 1000000000)
MODES=(rel abs read poll load)

build_cells() {
    local cells=()
    cells+=("tcg|core2duo|1")
    cells+=("tcg|core2duo|4")
    cells+=("tcg|qemu64|1")
    cells+=("tcg|max|1")
    if [ "$KVM_MODE" = "1" ] || { [ "$KVM_MODE" = "auto" ] && [ -w /dev/kvm ]; }; then
        cells+=("kvm|host|1")
        cells+=("kvm|host|4")
    else
        echo "META note=kvm_skipped reason=/dev/kvm_unavailable_or_disabled" >&2
    fi
    echo "${cells[@]}"
}

if [ -n "${TIME_REGRESSION_CELLS:-}" ]; then
    read -ra CELLS <<<"$TIME_REGRESSION_CELLS"
else
    read -ra CELLS <<<"$(build_cells)"
fi

# --- QEMU lifecycle --------------------------------------------------------

QEMU_PID=""
PTY_DEV=""

cleanup_qemu() {
    if [ -n "$QEMU_PID" ] && kill -0 "$QEMU_PID" 2>/dev/null; then
        kill -TERM "$QEMU_PID" 2>/dev/null || true
        local i
        for i in $(seq 1 20); do
            kill -0 "$QEMU_PID" 2>/dev/null || break
            sleep 0.2
        done
        kill -9 "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
    fi
    QEMU_PID=""
}
trap cleanup_qemu EXIT INT TERM

# accel_args <accel> <cpu> — result in the ACCEL_ARGS array.
accel_args() {
    case "$1" in
        kvm) ACCEL_ARGS=(-enable-kvm -cpu "$2") ;;
        tcg) ACCEL_ARGS=(-accel tcg -cpu "$2") ;;
        *)   ACCEL_ARGS=(-accel "$1" -cpu "$2") ;;
    esac
}

# Wait until `marker` appears in `file`, polling at 0.2s. `timeout` is seconds.
wait_for_marker() {
    local file="$1" marker="$2" timeout="${3:-10}"
    local ticks=0 max_ticks=$((timeout * 5))
    while [ "$ticks" -lt "$max_ticks" ]; do
        if grep -qF "$marker" "$file" 2>/dev/null; then
            return 0
        fi
        sleep 0.2
        ticks=$((ticks + 1))
    done
    return 1
}

wait_for_prompt() {
    local serial="$1" timeout="${2:-90}"
    local elapsed=0
    while [ "$elapsed" -lt "$timeout" ]; do
        # oksh prints "# " or "$ " as a prompt; also detect the uart init line
        # as a sign that userspace is up.
        if grep -qE '#[[:space:]]*$|\$[[:space:]]*$|serial input enabled' "$serial" 2>/dev/null; then
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    return 1
}

# send_guest <pty> <string>
send_guest() {
    printf '%s\r\n' "$2" >"$1"
}

# --- per-cell runner -------------------------------------------------------

run_cell() {
    local accel="$1" cpu="$2" smp="$3"
    local tag="${accel}-${cpu}-smp${smp}"
    local raw="$LOG_DIR/raw-${tag}.log"
    local transcript="$LOG_DIR/transcript-${tag}.log"
    : > "$raw"
    : > "$transcript"

    echo "META qemu=$QEMU_VERSION machine=pc commit=$COMMIT dirty=$DIRTY iso_sha=$ISO_SHA accel=$accel cpu=$cpu smp=$smp"

    accel_args "$accel" "$cpu"
    # Use -serial pty for bidirectional communication. QEMU prints the PTY
    # device path on stderr. Use the default machine (pc/i440FX) — q35 hangs
    # under TCG on this kernel.
    "$QEMU_BIN" \
        -M pc \
        "${ACCEL_ARGS[@]}" \
        -smp "$smp" \
        -m 1024 \
        -cdrom "$ISO" \
        -boot d \
        -serial pty \
        -monitor none \
        -display none \
        -no-reboot \
        >"$LOG_DIR/qemu-stderr-${tag}.log" 2>&1 &
    QEMU_PID=$!

    # Wait for QEMU to print the PTY device path.
    local pty_wait=0
    while [ "$pty_wait" -lt 30 ]; do
        PTY_DEV="$(grep -oE '/dev/pts/[0-9]+' "$LOG_DIR/qemu-stderr-${tag}.log" 2>/dev/null | head -1)"
        [ -n "$PTY_DEV" ] && break
        sleep 0.5
        pty_wait=$((pty_wait + 1))
    done

    if [ -z "$PTY_DEV" ]; then
        echo "RESULT accel=$accel cpu=$cpu smp=$smp status=no_pty" >&2
        cleanup_qemu
        return 1
    fi

    # Configure the PTY for raw mode and start draining it into the raw log.
    stty -F "$PTY_DEV" raw -echo 2>/dev/null || true
    cat "$PTY_DEV" >"$raw" &
    local cat_pid=$!

    if ! wait_for_prompt "$raw" 90; then
        echo "RESULT accel=$accel cpu=$cpu smp=$smp status=boot_timeout" >&2
        cat "$raw" >>"$transcript"
        kill "$cat_pid" 2>/dev/null || true
        cleanup_qemu
        return 1
    fi

    # Give the shell a moment to initialize after the prompt appears.
    sleep 2
    send_guest "$PTY_DEV" ""
    sleep 1

    # Marker-overhead calibration: 1-iteration batch, host timestamps
    # BATCH_START..BATCH_END receipt.
    local calib_start="" calib_end="" calib_delta=0
    cat "$raw" >>"$transcript"
    : > "$raw"
    send_guest "$PTY_DEV" "clockcheck rel 1 1"
    wait_for_marker "$raw" "BATCH_START" 10 && calib_start="$(date +%s%N)"
    wait_for_marker "$raw" "BATCH_END" 15 && calib_end="$(date +%s%N)"
    wait_for_marker "$raw" "DONE" 10
    cat "$raw" >>"$transcript"
    if [ -n "$calib_end" ] && [ -n "$calib_start" ]; then
        calib_delta=$((calib_end - calib_start))
        echo "META cell=$tag marker_overhead_ns=$calib_delta"
    else
        echo "META cell=$tag marker_overhead_ns=unavailable reason=no_batch_end" >&2
    fi

    local cell_failures=0
    local mode dur iters extra
    for mode in "${MODES[@]}"; do
        for dur in "${DURATIONS[@]}"; do
            case "$dur" in
                1000000000) iters=5 ;;
                100000000)  iters=10 ;;
                16666667)   iters=20 ;;
                *)          iters="$ITERS" ;;
            esac
            extra=0
            [ "$mode" = "load" ] && extra=2

            : > "$raw"
            send_guest "$PTY_DEV" "clockcheck $mode $dur $iters $extra"

            local batch_start="" batch_end=""
            if [ "$dur" -le "$BATCH_THRESHOLD_NS" ]; then
                wait_for_marker "$raw" "BATCH_START" 10 && batch_start="$(date +%s%N)"
                wait_for_marker "$raw" "BATCH_END" 15 && batch_end="$(date +%s%N)"
            fi
            if ! wait_for_marker "$raw" "DONE" 30; then
                echo "RESULT accel=$accel cpu=$cpu smp=$smp mode=$mode req_ns=$dur status=timeout" >&2
                cat "$raw" >>"$transcript"
                cell_failures=$((cell_failures + 1))
                continue
            fi

            local host_batch_ns=0 host_rate_ns_per_op=0
            if [ -n "$batch_start" ] && [ -n "$batch_end" ]; then
                host_batch_ns=$((batch_end - batch_start))
                [ "$iters" -gt 0 ] && host_rate_ns_per_op=$((host_batch_ns / iters))
            fi

            local summary
            summary="$(awk -v mode="$mode" -v req="$dur" -f "$SCRIPT_DIR/parse.awk" "$raw")"
            [ -z "$summary" ] && summary="SUMMARY mode=$mode req_ns=$dur iters=0 missing=1"
            echo "RESULT accel=$accel cpu=$cpu smp=$smp mode=$mode req_ns=$dur iters=$iters \
host_batch_ns=$host_batch_ns host_batch_overhead_ns=$calib_delta \
host_rate_ns_per_op=$host_rate_ns_per_op $summary"

            # Evaluate hard gates: never early, monotonic non-decrease.
            local early backward
            early="$(printf '%s' "$summary" | sed -n 's/.* early=\([0-9]*\).*/\1/p')"
            backward="$(printf '%s' "$summary" | sed -n 's/.* backward=\([0-9]*\).*/\1/p')"
            if [ "${early:-0}" -ne 0 ] || [ "${backward:-0}" -ne 0 ]; then
                cell_failures=$((cell_failures + 1))
                echo "GATE_VIOLATION accel=$accel cpu=$cpu smp=$smp mode=$mode req_ns=$dur early=${early:-0} backward=${backward:-0}" >&2
            fi

            cat "$raw" >>"$transcript"
        done
    done

    # QMP stop/resume semantics test: with -monitor none we cannot exercise the
    # monitor from the host. Record as not-run; the cleanup path uses process
    # signals instead. A dedicated QMP test can be added when a monitor socket
    # is wired in.
    echo "META cell=$tag qmp_stop_resume=not_run reason=monitor_disabled_for_bidir_serial"

    # Clean shutdown.
    send_guest "$PTY_DEV" "poweroff"
    local i
    for i in $(seq 1 "$CELL_TIMEOUT"); do
        kill -0 "$QEMU_PID" 2>/dev/null || break
        sleep 1
    done
    if kill -0 "$QEMU_PID" 2>/dev/null; then
        echo "RESULT accel=$accel cpu=$cpu smp=$smp status=shutdown_timeout" >&2
        cell_failures=$((cell_failures + 1))
    fi
    kill "$cat_pid" 2>/dev/null || true
    cleanup_qemu
    return "$cell_failures"
}

# --- main ------------------------------------------------------------------

echo "META harness=time-regression iso=$ISO iso_sha=$ISO_SHA iters=$ITERS"
echo "META qemu_version=$QEMU_VERSION"
echo "META commit=$COMMIT dirty=$DIRTY"
echo "META cells=${CELLS[*]}"

failures=0
for cell in "${CELLS[@]}"; do
    IFS='|' read -r accel cpu smp <<<"$cell"
    run_cell "$accel" "$cpu" "$smp"
    failures=$((failures + $?))
done

echo "META done failures=$failures"
[ "$failures" -gt 255 ] && failures=255
exit "$failures"
