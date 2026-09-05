#!/bin/sh
#
# Where the graphical player's resident memory sits, snapshotted at five states in ONE launch:
#
#     scripts/memory-census-gui.sh /tmp/memdump > /tmp/gui-memory.txt
#
# The states: the player view after the boot scan settles (a restored session opens there), the
# same view playing (the high-res cover window fills), after hopping albums (LRU churn), the
# library after walking the grid (every card on the way drawn once), and the player again (what
# the library visit left behind). VmHWM at the end is the peak, scan included.
#
# The player runs inside its own headless compositor (a nested sway on WLR_BACKENDS=headless), so
# nothing appears on screen and no synthetic key ever goes near the user's seat: wtype talks to the
# nested display, where the player is the only window and always focused. Playback control still
# goes over MPRIS, which is display-independent.
#
# Attribution needs no instrumentation: thumbnails (~189 KiB) and high-res covers (~2.5 MiB) are
# above glibc's mmap threshold, so they land in mmap'd anonymous memory - coalesced into a few
# large regions, but separable from [heap] and countable in aggregate. What is left after the
# arithmetic is malloc arenas, the GPU driver, and the binary - split out by mapping name.

set -eu

snapdir=${1:?usage: $0 SNAPDIR}
mkdir -p "$snapdir"

cd "$(dirname "$0")/.."
eval "$(scripts/census-env.sh phonoscule)"

player=./target/profiling/phonoscule-gui
RUSTFLAGS="-C force-frame-pointers=yes" cargo build --profile profiling -p phonoscule-gui >&2 ||
    { echo "$0: build failed, refusing to measure" >&2; exit 1; }

WGPU_BACKEND=${WGPU_BACKEND:-vulkan}
export WGPU_BACKEND

# MPRIS is how this script controls playback, and the bus name is first come, first served: if a
# player of the user's own already holds it, our commands would land on theirs. A running player
# that is NOT on the bus (a stale instance that lost its registration) is harmless - our instance
# claims the free name and the commands reach only it.
if playerctl -l 2>/dev/null | grep -q '^phonoscule'; then
    echo "$0: a phonoscule already owns the MPRIS name; play/volume would hit the user's player" >&2
    exit 1
fi

# --- the headless compositor -------------------------------------------------------------------

sway_log=$snapdir/sway.log
# --verbose because the display announcement below is an info-level log line.
WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
    timeout 900 sway --verbose -c scripts/census-headless-sway.conf > "$sway_log" 2>&1 &
sway_pid=$!

cleanup() {
    [ -n "${gui:-}" ] && kill "$gui" 2>/dev/null || true
    kill "$sway_pid" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# The nested instance picks the first free wayland-N; it says which in its log.
nested=
i=0
while [ "$i" -lt 20 ]; do
    nested=$(sed -n "s/.*Running compositor on wayland display '\(wayland-[0-9]*\)'.*/\1/p" "$sway_log" | head -1)
    [ -n "$nested" ] && break
    kill -0 "$sway_pid" 2>/dev/null || { echo "$0: headless sway died:" >&2; tail -5 "$sway_log" >&2; exit 1; }
    sleep 0.5
    i=$((i + 1))
done
[ -n "$nested" ] || { echo "$0: headless sway never reported its display" >&2; exit 1; }
sway_sock=$(ls "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"/sway-ipc.*."$sway_pid".sock 2>/dev/null | head -1)
echo "# headless compositor on $nested (ipc ${sway_sock:-?})" >&2

# --- instruments -------------------------------------------------------------------------------

vram() {
    for f in /sys/class/drm/card*/device/mem_info_vram_used /sys/class/drm/card*/device/mem_info_gtt_used; do
        [ -r "$f" ] && echo "  $(basename "$f")  $(($(cat "$f") / 1048576)) MB"
    done
}

snap() {
    label=$1
    pid=$2
    [ -d "/proc/$pid" ] || { echo "# $label: process gone"; return 0; }
    cp "/proc/$pid/smaps" "$snapdir/$label.smaps" 2>/dev/null || true

    echo "=============================================================================="
    echo "snapshot: $label   ($(date -Is))"
    echo "=============================================================================="
    grep -E '^(VmRSS|VmHWM|RssAnon|RssFile|RssShmem|VmSwap)' "/proc/$pid/status" | awk '{printf "  %-10s %8.1f MB\n", $1, $2/1024}'
    echo
    echo "--- resident, by mapping kind ---"
    awk '
        /^[0-9a-f]+-[0-9a-f]+ / {
            name = ""
            for (i = 6; i <= NF; i++) name = name (name ? " " : "") $i
            if (name == "")                    kind = "anonymous"
            else if (name == "[heap]")         kind = "[heap]"
            else if (name == "[stack]")        kind = "[stack]"
            else if (name ~ /phonoscule-gui/)  kind = "the binary"
            else if (name ~ /\.so/)            kind = "shared libraries"
            else if (name ~ /memfd|shm|SYSV/)  kind = "shmem (memfd etc.)"
            else                               kind = "other files"
        }
        /^Rss:/ { rss[kind] += $2 }
        END { for (k in rss) printf "  %-20s %8.1f MB\n", k, rss[k]/1024 }
    ' "/proc/$pid/smaps" | sort -k2 -rn
    echo
    echo "--- anonymous mappings, count x size (top 12 sizes by total Rss) ---"
    awk '
        /^[0-9a-f]+-[0-9a-f]+ / { anon = (NF < 6) }
        anon && /^Rss:/ && $2 > 0 { n[$2]++ }
        END { for (s in n) printf "%12.0f %6d x %8.0f KB\n", s * n[s], n[s], s }
    ' "/proc/$pid/smaps" | sort -rn | head -12 | awk '{printf "  %8.1f MB   %5d mappings of %8.0f KB\n", $1/1024, $2, $4}'
    echo
    echo "--- gpu, system-wide (RELATIVE only: shared GPU on a machine in use) ---"
    vram
    echo
}

settle() {
    pid=$1
    i=0
    while [ "$i" -lt 60 ]; do
        before=$(sed 's/.*) //' "/proc/$pid/stat" | awk '{print $12 + $13}')
        sleep 2
        [ -d "/proc/$pid" ] || return 1
        after=$(sed 's/.*) //' "/proc/$pid/stat" | awk '{print $12 + $13}')
        [ $((after - before)) -lt 5 ] && { echo "# settled after $((i * 2 + 2))s" >&2; return 0; }
        i=$((i + 1))
    done
    echo "# never settled, measuring anyway" >&2
    return 0
}

# Keys go to the nested display, where the player is the only window. The headless seat has no
# keyboard until wtype attaches its virtual one, and sway only assigns keyboard focus once a
# keyboard exists - so prime the seat with a throwaway key, re-focus, and only then send the burst.
keys() {
    key=$1
    count=$2
    WAYLAND_DISPLAY=$nested wtype -k Shift_L 2>/dev/null || true
    [ -n "$sway_sock" ] && swaymsg -s "$sway_sock" '[app_id="^phonoscule.*"] focus' >/dev/null 2>&1 || true
    sleep 0.3
    i=0
    while [ "$i" -lt "$count" ]; do
        WAYLAND_DISPLAY=$nested wtype -k "$key" 2>/dev/null || { echo "# wtype failed at $key $((i + 1))/$count"; return 0; }
        sleep 0.05
        i=$((i + 1))
    done
}

# --- the run -----------------------------------------------------------------------------------

echo "GUI memory census, WGPU_BACKEND=$WGPU_BACKEND, smaps dumps in $snapdir"
echo "started $(date -Is)"
echo "binary        $player ($(date -Is -r "$player"))"
echo "phonoscule    $(git rev-parse --short HEAD 2>/dev/null || echo '?')"
echo

# Under heaptrack when asked (CENSUS_HEAPTRACK=1): every allocation with its call stack, streamed
# to disk as it happens, so even a killed run leaves a usable trace. Costs some run speed and a
# little resident memory of its own - the smaps snapshots of a traced run read a few MB high.
runner=
if [ -n "${CENSUS_HEAPTRACK:-}" ]; then
    runner="heaptrack -o $snapdir/heaptrack"
    echo "# tracing allocations with heaptrack" >&2
fi
WAYLAND_DISPLAY=$nested DISPLAY= timeout 600 $runner "$player" > /dev/null 2>&1 &
gui=$!
sleep 3
# Not necessarily our direct child (heaptrack wraps), and a player of the user's own may be
# running too (harmlessly, per the pre-flight check) - the newest instance is the one just
# launched.
pid=$(pgrep -nx phonoscule-gui || true)
[ -n "$pid" ] || { echo "# never started"; exit 1; }

# A restored session boots into the player view; settling means the scan is done and every
# thumbnail is loaded.
settle "$pid" || { echo "# died during settle"; exit 1; }
snap "A-player-paused-scan-done" "$pid"

# Muted playback, muted BEFORE playing: the mixer sits past the decoder, so the measured state is
# unchanged and the speakers never blip.
playerctl -p phonoscule volume 0 2>/dev/null || echo "# playerctl mute failed"
playerctl -p phonoscule play 2>/dev/null || echo "# playerctl play failed"
sleep 12
echo "# mpris says: $(playerctl -p phonoscule status 2>/dev/null || echo '?')"
snap "B-player-playing" "$pid"

# Hop 15 albums forward: high-res decodes, LRU eviction, cover-flow texture churn.
keys Page_Down 15
sleep 8
snap "C-player-after-15-album-hops" "$pid"

# The library, walked 40 rows down: every card on the way is laid out and its cover drawn once.
keys Tab 1
sleep 2
keys Down 40
sleep 5
snap "D-library-after-grid-walk" "$pid"

# Back to the player: what the library visit left resident.
keys Tab 1
sleep 3
snap "E-player-again" "$pid"

if [ -n "$sway_sock" ]; then
    echo "# window: $(swaymsg -s "$sway_sock" -t get_tree 2>/dev/null | grep -o '"name": "Phonoscule"[^}]*' | head -1 || echo '?')"
fi

# TERM the player itself (not just the wrapper), then let heaptrack finish writing its trace.
kill "$pid" 2>/dev/null || true
kill "$gui" 2>/dev/null || true
wait "$gui" 2>/dev/null || true
echo "finished $(date -Is)"
