# Changelog


v1.0.0 (2026-07-20)
------------------------------------------------------------

The first release: Phonoscule, the reference application of the
[phonoscule](../README.md) framework - a native, album-art-centric music player
built on [iced](https://iced.rs), meant as a lightweight alternative to
cover-grid MPD clients.

It has two views. The **Library** browses your collection as a grid of cover
art, with albums grouped from the files' tags (album artist + album title)
rather than the directory layout - so a multi-disc album spread across folders
is one album, and two same-named albums by different artists stay apart. You
narrow it with genre and artist chips, fuzzy album search (just start typing),
and a persisted sort order. The **Player** lays the play queue out as an
iPod-style Cover Flow on a reflective floor, over a backdrop that glows with the
playing cover's accent colour, and the album's track list alongside.

Beyond browsing and playing:

- **Playback.** A play queue with album runs, shuffle by album or track, four
  repeat modes, seeking, and per-application volume driven through the OS mixer.
- **Desktop integration.** An MPRIS server (media keys and now-playing), and the
  session - queue, playback position, sort order - restored across runs.
- **Keyboard-driven.** Nearly everything is reachable from the keyboard.

It plays **WAV** (8/16/24/32-bit integer and 32-bit float, mono or stereo) and
**Ogg Opus** - the latter decoded by the pure-Rust
[opuscule](https://codeberg.org/jojo-laplace/opuscule) decoder through the
framework - with cover art taken from folder images
(`{cover,folder,front,albumart}.{jpg,jpeg,png,webp}`), out through PulseAudio
(and PipeWire via pipewire-pulse) on Linux. The music directory is set in
`~/.config/phonoscule.toml` (the `music-dir` key, defaulting to `~/Music`).

Not yet: FLAC, Ogg Vorbis, and other formats, and audio output beyond Linux /
PulseAudio. Released under the Mozilla Public License 2.0.
