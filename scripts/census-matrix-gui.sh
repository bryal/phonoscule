#!/bin/sh
#
# The graphical player's side of the census.
#
#     scripts/census-matrix-gui.sh 30 3 > /tmp/gui-baseline.txt
#
# No pty to size here, so the window is whatever the compositor gives it: recorded rather than chosen,
# and rows are comparable within a run only. Switching views needs `swaymsg` and `wtype`, the one part
# specific to this desktop; playback still goes over MPRIS.

set -eu

window=${1:-30}
repeats=${2:-3}

cd "$(dirname "$0")/.."
eval "$(scripts/census-env.sh phonoscule)"

player=./target/profiling/phonoscule-gui

# Built here rather than checked for: an existing binary says nothing about whether it is the one you
# meant to measure, and a stale one reads as a perfectly good result.
RUSTFLAGS="-C force-frame-pointers=yes" cargo build --profile profiling -p phonoscule-gui >&2 ||
    { echo "$0: build failed, refusing to measure" >&2; exit 1; }

# Pinned so runs are comparable; the graphics backend is not what is under test here.
WGPU_BACKEND=${WGPU_BACKEND:-vulkan}
export WGPU_BACKEND

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

row() {
    label=$1
    play=$2
    switch=$3

    echo "##############################################################################"
    echo "# $label   (play=$play, switch_view=$switch)"
    echo "##############################################################################"

    "$player" > /dev/null 2>&1 &
    gui=$!
    sleep 3
    pid=$(pgrep -x phonoscule-gui | head -1)
    [ -n "$pid" ] || { echo "# never started"; return 0; }

    settle "$pid" || { echo "# died during settle"; return 0; }

    if [ "$switch" = yes ]; then
        # Tab switches between the library and the player, and needs the window focused to receive it.
        swaymsg '[app_id="^phonoscule.*"] focus' >/dev/null 2>&1 || swaymsg '[class="^[Pp]honoscule.*"] focus' >/dev/null 2>&1 || true
        sleep 1
        wtype -k Tab 2>/dev/null || echo "# wtype failed; this row is in whatever view it started in"
        sleep 2
    fi

    if [ "$play" = yes ]; then
        playerctl -p phonoscule play 2>/dev/null || echo "# playerctl play failed"
        sleep 5
        echo "# mpris says: $(playerctl -p phonoscule status 2>/dev/null || echo '?')"
    fi

    # A property of the run, not a choice: the compositor sized it.
    echo "# window: $(swaymsg -t get_tree 2>/dev/null | grep -o '"name": "Phonoscule"[^}]*' | head -1 || echo '?')"

    n=1
    while [ "$n" -le "$repeats" ]; do
        scripts/cpu-census.sh "$label run $n/$repeats" "$pid" "$window"
        n=$((n + 1))
    done

    kill "$gui" 2>/dev/null || true
    wait "$gui" 2>/dev/null || true
    sleep 3
}

echo "GUI census, window=${window}s repeats=$repeats, WGPU_BACKEND=$WGPU_BACKEND"
echo "started $(date -Is)"
echo "NOTE: the mouse must stay off the window - hovering a cover is a message per motion event."
echo

#   label                  play switch
row "G1-library-paused"     no   no
row "G2-library-playing"    yes  no
row "G3-player-paused"      no   yes
row "G4-player-playing"     yes  yes

echo "finished $(date -Is)"
