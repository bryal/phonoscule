#!/bin/sh
#
# Prepares a scratch cache and state root for one player and prints the environment that points it
# there, warmed from the real one so a run measures steady state rather than a first launch:
#
#     eval "$(scripts/census-env.sh phonoscule-tui)"
#     ./target/profiling/phonoscule-tui scripts/census-tui.toml
#
# Why not just run the player normally: a measurement run browses, plays, and triggers rescans, and
# all of that writes - the session, the album index, the tag cache, thumbnails. Pointing it at
# scratch roots means a suite of runs cannot leave the real player somewhere it did not put itself,
# and means "warm" and "cold" become a thing the operator chooses rather than a thing that depends on
# which run happened to go first.
#
# `dirs` resolves both roots through $XDG_CACHE_HOME and $XDG_STATE_HOME, so overriding those is
# enough - no flags, no config keys, nothing to keep in step with the code.
#
# Idempotent: the copy happens once and later calls just print. Delete the root to force a re-warm.

set -eu

if [ $# -lt 1 ]; then
    echo "usage: eval \"\$($0 PLAYER)\"   # e.g. phonoscule-tui, phonoscule-gui" >&2
    exit 1
fi

player=$1
root=${CENSUS_ROOT:-$HOME/.cache/phonoscule-census}/$player

if [ ! -d "$root" ]; then
    # A full copy rather than hardlinks. The scan writes thumbnails in place when it decides one is
    # missing or stale, and a hardlinked cache would land that write in the real player's directory -
    # which is exactly the thing this script exists to prevent. A couple of hundred megabytes is a
    # cheap price for that not being a question.
    mkdir -p "$root/cache" "$root/state"
    [ -d "$HOME/.cache/$player" ] && cp -a "$HOME/.cache/$player" "$root/cache/$player"
    [ -d "$HOME/.local/state/$player" ] && cp -a "$HOME/.local/state/$player" "$root/state/$player"
    echo "# warmed $root from the real $player directories" >&2
fi

echo "export XDG_CACHE_HOME='$root/cache'"
echo "export XDG_STATE_HOME='$root/state'"
