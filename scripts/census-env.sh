#!/bin/sh
#
# Prepares a scratch cache and state root for one player and prints the environment that points it
# there, warmed from the real one so a run measures steady state rather than a first launch:
#
#     eval "$(scripts/census-env.sh phonoscule-tui)"
#     ./target/profiling/phonoscule-tui scripts/census-tui.toml
#
# A measurement run browses, plays and rescans, all of which write. Scratch roots keep a suite of runs
# from leaving the real player somewhere it did not put itself, and make warm-versus-cold a choice
# rather than an accident of ordering. `dirs` resolves both roots through those two variables, so
# overriding them is enough.
#
# Idempotent: the copy happens once. Delete the root to force a re-warm.

set -eu

if [ $# -lt 1 ]; then
    echo "usage: eval \"\$($0 PLAYER)\"   # e.g. phonoscule-tui, phonoscule-gui" >&2
    exit 1
fi

player=$1
root=${CENSUS_ROOT:-$HOME/.cache/phonoscule-census}/$player

if [ ! -d "$root" ]; then
    # Copied, not hardlinked: the scan rewrites thumbnails in place, which through a hardlink would
    # land in the real player's directory - the one thing this exists to prevent.
    mkdir -p "$root/cache" "$root/state"
    [ -d "$HOME/.cache/$player" ] && cp -a "$HOME/.cache/$player" "$root/cache/$player"
    [ -d "$HOME/.local/state/$player" ] && cp -a "$HOME/.local/state/$player" "$root/state/$player"
    echo "# warmed $root from the real $player directories" >&2
fi

echo "export XDG_CACHE_HOME='$root/cache'"
echo "export XDG_STATE_HOME='$root/state'"
