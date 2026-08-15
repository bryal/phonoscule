#!/bin/sh
#
# Runs the whole TUI side of the census: every configuration, several windows each, into one file.
#
#     scripts/census-matrix-tui.sh 30 3 > /tmp/tui-baseline.txt
#
# The rows are chosen so each one differs from another in exactly one thing, because a single CPU
# figure says nothing on its own:
#
#   T1 vs T2   paused against playing, same view - what the progress tick costs.
#   T3 vs T4   the same, in the view that draws a full cover.
#   T2 vs T6   a tall terminal against a short one - if they cost the same, a frame is priced by the
#              size of the library rather than by what is on screen.
#   T5         a queue holding every album, for the parts that walk it per frame.
#
# Playback is started over MPRIS rather than by typing, so no window has to be focused and no
# keystroke can go astray. View switching does need keys, which go in through the harness's pty.
#
# Nothing here touches the real player's directories: `census-env.sh` points it at scratch roots.

set -eu

window=${1:-30}
repeats=${2:-3}

cd "$(dirname "$0")/.."
eval "$(scripts/census-env.sh phonoscule-tui)"

player=./target/profiling/phonoscule-tui
conf=scripts/census-tui.toml
[ -x "$player" ] || { echo "$0: build it first: RUSTFLAGS='-C force-frame-pointers=yes' cargo build --profile profiling" >&2; exit 1; }

# Waits for the boot scan to stop costing anything, so a census measures steady state rather than the
# tail of a scan. Polls the process's own tick counter instead of sleeping a guessed amount: the scan
# walks every file in the library and how long that takes is not a constant.
settle() {
    pid=$1
    i=0
    while [ "$i" -lt 40 ]; do
        before=$(sed 's/.*) //' "/proc/$pid/stat" | awk '{print $12 + $13}')
        sleep 2
        [ -d "/proc/$pid" ] || return 1
        after=$(sed 's/.*) //' "/proc/$pid/stat" | awk '{print $12 + $13}')
        # Under 3 ticks (30 ms) of CPU in two seconds means whatever it was doing is done.
        [ $((after - before)) -lt 3 ] && { echo "# settled after $((i * 2 + 2))s" >&2; return 0; }
        i=$((i + 1))
    done
    echo "# never settled, measuring anyway" >&2
    return 0
}

row() {
    label=$1
    cols=$2
    rows=$3
    keys=$4
    play=$5
    # How long to let the player run before typing at it. Anything that acts on the album list (a
    # "queue everything shown" say) has to wait for the boot scan to have shown it something, or it
    # acts on an empty list and the row measures nothing.
    keys_after=${6:-3}

    echo "##############################################################################"
    echo "# $label   (${cols}x${rows}, keys='$keys' after ${keys_after}s, play=$play)"
    echo "##############################################################################"

    log=$(mktemp)
    if [ -n "$keys" ]; then
        scripts/tui-harness.py --cols "$cols" --rows "$rows" --keys "$keys" --keys-after "$keys_after" \
            -- "$player" "$conf" > "$log" 2>&1 &
    else
        scripts/tui-harness.py --cols "$cols" --rows "$rows" -- "$player" "$conf" > "$log" 2>&1 &
    fi
    harness=$!

    # The harness prints the player's pid as soon as it has forked it.
    pid=
    i=0
    while [ -z "$pid" ] && [ "$i" -lt 50 ]; do
        sleep 0.2
        pid=$(awk '/^pid /{print $2; exit}' "$log" 2>/dev/null || true)
        i=$((i + 1))
    done
    [ -n "$pid" ] || { echo "# harness never reported a pid; log follows"; cat "$log"; kill "$harness" 2>/dev/null || true; return 0; }

    settle "$pid" || { echo "# player died during settle; log follows"; cat "$log"; return 0; }

    if [ "$play" = yes ]; then
        # Looping, because a restored queue is a handful of tracks and a full matrix run is long
        # enough to reach the end of it. A row whose playback quietly stopped half way through reads
        # as a wonderful improvement, so make it unable to happen.
        playerctl -p phonoscule-tui loop Playlist 2>/dev/null || echo "# could not set the loop mode"
        playerctl -p phonoscule-tui play 2>/dev/null || echo "# playerctl play failed"
        sleep 5
        echo "# mpris says: $(playerctl -p phonoscule-tui status 2>/dev/null || echo '?')"
    fi

    # Re-checked after every window: a row that stopped playing measured the wrong thing, and it is
    # better to say so in the output than to average it in.
    check_playing() {
        [ "$play" = yes ] || return 0
        [ "$(playerctl -p phonoscule-tui status 2>/dev/null)" = Playing ] ||
            echo "# WARNING: not playing at the end of this window; treat the row as suspect"
    }

    CENSUS_EXTRA_PIDS=$harness
    export CENSUS_EXTRA_PIDS
    n=1
    while [ "$n" -le "$repeats" ]; do
        scripts/cpu-census.sh "$label run $n/$repeats" "$pid" "$window"
        check_playing
        n=$((n + 1))
    done

    kill "$harness" 2>/dev/null || true
    wait "$harness" 2>/dev/null || true
    rm -f "$log"
    # Let the audio server and the compositor go quiet before the next row starts.
    sleep 3
}

echo "TUI census, window=${window}s repeats=$repeats, halfblocks on a pty (no terminal emulator)"
echo "started $(date -Is)"
echo

#   label                     cols rows keys    play keys_after
row "T1-library-paused"        200   50 ""      no
row "T2-library-playing"       200   50 ""      yes
row "T3-player-paused"         200   50 '\t'    no
row "T4-player-playing"        200   50 '\t'    yes
# Ctrl+A queues every album shown and starts playing, which also switches to the player view. It has
# to land after the scan, hence the wait; MPRIS then plays as a backstop in case the key went astray.
row "T5-player-playing-fullq"  200   50 '\x01'  yes  20
row "T6-library-playing-80x24"  80   24 ""      yes

echo "finished $(date -Is)"
