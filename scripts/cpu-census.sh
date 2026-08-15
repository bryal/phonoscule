#!/bin/sh
#
# What CPU a running player is spending, per thread, alongside the processes around it:
#
#     scripts/cpu-census.sh player-playing $(pgrep -x phonoscule-tui) 60
#
# An observer only - getting a player into a given state differs per machine, reading the cost of it
# does not. The per-thread split is the point: the threads are already named, so the decoder's cost
# and the drawing's separate without instrumentation.
#
# Percentages are of ONE core, not of the machine.

set -eu

if [ $# -lt 2 ]; then
    echo "usage: $0 LABEL PID [WINDOW_SECONDS]" >&2
    exit 1
fi

label=$1
pid=$2
window=${3:-60}

if [ ! -d "/proc/$pid" ]; then
    echo "$0: no process $pid" >&2
    exit 1
fi

# Where a player's behaviour shows up besides itself: a change that looks free here can be paid for
# next door. `CENSUS_EXTRA_PIDS` adds whatever else a run wants watched, the terminal above all.
neighbours=$(pgrep -d, -x 'pipewire|pipewire-pulse|wireplumber|easyeffects|sway' 2>/dev/null || true)
if [ -n "${CENSUS_EXTRA_PIDS:-}" ]; then
    neighbours="$neighbours,$CENSUS_EXTRA_PIDS"
fi

# utime and stime, fields 14 and 15 of /proc/pid/stat. Strip through the last ") " first: a comm can
# contain spaces and parentheses. State then lands at $1, so the two are at $12 and $13.
cpu_ticks() {
    sed 's/.*) //' "/proc/$1/stat" | awk '{print $12 + $13}'
}

tick_hz=$(getconf CLK_TCK)
gpu=/sys/class/drm/card1/device/gpu_busy_percent

echo "=============================================================================="
echo "census: $label"
echo "=============================================================================="
echo "when          $(date -Is)"
echo "pid           $pid ($(tr -d '\0' < "/proc/$pid/comm"))"
echo "command       $(tr '\0' ' ' < "/proc/$pid/cmdline")"
exe=$(readlink -f "/proc/$pid/exe" 2>/dev/null || echo '?')
echo "binary        $exe"
[ -f "$exe" ] && echo "built         $(date -Is -r "$exe")"
echo "window        ${window}s"
echo "phonoscule    $(git -C "$(dirname "$0")/.." rev-parse --short HEAD 2>/dev/null || echo '?')"
echo "opuscule      $(git -C "$(dirname "$0")/../../opuscule" rev-parse --short HEAD 2>/dev/null || echo '?')"
echo "rustc         $(rustc -V 2>/dev/null || echo '?')"
echo "RUSTFLAGS     ${RUSTFLAGS:-(unset)}"
echo "load before   $(cut -d' ' -f1-3 /proc/loadavg)"
echo

# Same window as pidstat, so the two cross-check each other.
before=$(cpu_ticks "$pid")
start=$(date +%s.%N)

pidstat_out=$(mktemp)
pidstat -h -u -w -t -p "$pid" "$window" 1 > "$pidstat_out" 2>&1 &
pidstat_pid=$!

neighbours_out=$(mktemp)
if [ -n "$neighbours" ]; then
    pidstat -h -u -w -p "$neighbours" "$window" 1 > "$neighbours_out" 2>&1 &
    neighbours_pid=$!
else
    neighbours_pid=
fi

# Relative within a run and nothing more: this is a shared GPU on a machine someone is using, so
# anything else drawing lands in the same counter.
gpu_samples=$(mktemp)
if [ -r "$gpu" ]; then
    i=0
    while [ "$i" -lt "$window" ]; do
        cat "$gpu" >> "$gpu_samples"
        sleep 1
        i=$((i + 1))
    done
fi

wait "$pidstat_pid" || true
[ -n "$neighbours_pid" ] && { wait "$neighbours_pid" || true; }

after=$(cpu_ticks "$pid")
end=$(date +%s.%N)

echo "--- process total, from /proc/$pid/stat (percent of one core) ---"
awk -v b="$before" -v a="$after" -v s="$start" -v e="$end" -v hz="$tick_hz" 'BEGIN {
    secs = e - s
    printf "  cpu       %.2f%%   (%d ticks over %.2fs at %d Hz)\n", (a - b) / hz / secs * 100, a - b, secs, hz
}'
echo

echo "--- per thread, from pidstat (%usr %system, and cswch/s as the wakeup count) ---"
cat "$pidstat_out"
echo

if [ -n "$neighbours" ]; then
    echo "--- neighbours: the audio server, the compositor, the terminal ---"
    cat "$neighbours_out"
    echo
fi

if [ -s "$gpu_samples" ]; then
    echo "--- gpu_busy_percent, RELATIVE ONLY (shared GPU on a machine in use) ---"
    awk '{ t += $1; if (NR == 1 || $1 < lo) lo = $1; if ($1 > hi) hi = $1 }
         END { printf "  min %d  mean %.1f  max %d  (n=%d)\n", lo, t / NR, hi, NR }' "$gpu_samples"
    echo
fi

echo "load after    $(cut -d' ' -f1-3 /proc/loadavg)"
rm -f "$pidstat_out" "$neighbours_out" "$gpu_samples"
