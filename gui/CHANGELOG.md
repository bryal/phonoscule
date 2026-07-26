# Changelog


v1.1.0 (2026-07-21)
------------------------------------------------------------

- **UI scaling.** A `scaling` factor in the config sizes the whole interface for
  high-DPI displays (1.0 is unscaled, larger is bigger). Ctrl+plus and Ctrl+minus
  zoom it live and Ctrl+= resets to the configured value; the live changes last
  only for the session and are not written back to the config.
- **Cover Flow fits short windows.** On a short window the now-playing cover no
  longer slides up behind the top bar or down behind the playback bar. It scales
  to fit the clear space between the two, sized from the window height and seated
  a little above centre so its reflection keeps room below.


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
[opuscule](https://github.com/bryal/opuscule) decoder through the
framework - with cover art taken from folder images
(`{cover,folder,front,albumart}.{jpg,jpeg,png,webp}`), out through PulseAudio
(and PipeWire via pipewire-pulse) on Linux. The music directory is set in
`~/.config/phonoscule.toml` (the `music-dir` key, defaulting to `~/Music`).

Not yet: FLAC, Ogg Vorbis, and other formats, and audio output beyond Linux / PulseAudio.
