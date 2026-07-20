# Phonoscule

Phonoscule is a music player built around your album art: a graphical application
([`phonoscule-gui`](gui)), a terminal player ([`phonoscule-cli`](cli)), and the
[`phonoscule`](phonoscule) library they share.

The graphical app is the centrepiece — an album-cover-centric library browser with a
Cover Flow-style now-playing view, meant as a lightweight, native replacement for
cover-grid MPD clients.

## The graphical player (`phonoscule-gui`)

- **Album-cover library.** A grid of cover art you browse by sight, grouped into albums
  from the files' tags (album artist + album title) rather than the directory layout, so
  a multi-disc album spread over folders is one album and two same-named albums by
  different artists stay apart.
- **Filtering, search, and sorting.** Genre and artist chips, fuzzy album search (just
  start typing), and a configurable, persisted sort order.
- **Cover Flow now-playing view.** The queue's covers on a reflective floor over a
  backdrop that glows with the current cover's colour, with the album's track list
  alongside.
- **Playback.** A play queue with album runs, shuffle (by album or track), four repeat
  modes, seeking, and per-application volume driven through the OS mixer.
- **Desktop integration.** An MPRIS server (media keys and now-playing in your desktop),
  and the session — queue, position, sort — restored across runs.
- **Keyboard-driven.** Nearly everything is reachable from the keyboard (see
  [Controls](#controls)).

## The terminal player (`phonoscule-cli`)

A minimal REPL-style player: pass audio files on the command line and control playback
from the keyboard (play/pause, seek, next/previous). A thin wrapper over the library,
useful as a reference and for quick playback.

## The library (`phonoscule`)

`no_std`-friendly, async building blocks for a music player: metadata/tag reading, WAV
and Ogg Opus decoding (Opus via the sibling [opuscule](https://codeberg.org/jojo-laplace/opuscule)
crate), sample-accurate seeking, sample-format conversion, and a small source/sink
plumbing layer. It only requires an async ([embedded-io](https://crates.io/crates/embedded-io-async))
reader, so the same code runs from a desktop application down to a microcontroller.

## Formats & platform

- **Audio:** WAV (8/16/24/32-bit integer and 32-bit float, mono or stereo) and Ogg Opus.
- **Cover art:** folder images (`cover.jpg`, `folder.png`, `front.*`, `albumart.*`).
- **Output:** the players play through PulseAudio (and PipeWire, via pipewire-pulse) on
  Linux; the library itself is platform-agnostic.

Not in this release, but on the roadmap: FLAC and other formats, cover art embedded in
the audio files, and audio output beyond Linux/PulseAudio.

## Building & running

The graphical player:

```sh
cargo run --release -p phonoscule-gui
```

The terminal player:

```sh
cargo run --release -p phonoscule-cli -- track.opus another.wav
```

The graphical player reads its music directory from `~/.config/phonoscule.toml` (or
`$XDG_CONFIG_HOME/phonoscule.toml`):

```toml
music-dir = "~/Music"
```

Without a config file it defaults to `~/Music`. Relative paths are resolved against the
config file's location; `~` and environment variables are expanded.

## Controls

The essentials of the graphical player:

- **Space** — play / pause (from anywhere)
- **Tab / Shift+Tab** — switch between the Library and Player views
- In the Library: **type** to search albums, **Ctrl+F** to focus the search, **Ctrl+W**
  to clear the filters; **Enter** opens an album's track list, **Ctrl+Enter** / **Alt+Enter**
  play / queue everything currently shown
- On a selected album: **Ctrl+Space** plays it, **Alt+Space** queues it
- In the Player: **←/→** seek, **↑/↓** volume, **Home/End** previous/next track,
  **PageUp/PageDown** previous/next album, **Ctrl+Home/End** jump to the queue's ends
- **Alt+R** cycles the repeat mode; **Alt+S** / **Alt+Z** shuffle albums / tracks (hold
  **Ctrl** to shuffle the whole queue); **Ctrl+K** clears the queue
- The mouse wheel over the Player scrolls tracks (vertically) or albums (horizontally);
  over the volume bar it sets the volume

## License

Released under the **Mozilla Public License 2.0** (see [`LICENSE`](LICENSE)).
Copyright (c) 2026 Jojo <jo@jo.zone>.

Opus decoding is provided by the sibling [opuscule](https://codeberg.org/jojo-laplace/opuscule)
crate (also MPL 2.0), which is itself a derivative of the libopus reference decoder; its
upstream BSD notices and Opus patent grants travel with that crate.
