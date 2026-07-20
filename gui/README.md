# Phonoscule (graphical player)

The reference application of the [Phonoscule](../README.md) framework: a music player
that arranges your library by its album art, meant as a lightweight, native alternative
to cover-grid MPD clients.

## Features

- **Album-cover library.** A grid of cover art you browse by sight. Albums are grouped
  from the files' tags (album artist + album title) rather than the directory layout, so
  a multi-disc album spread across folders is one album, and two same-named albums by
  different artists stay apart.
- **Filtering, search, and sorting.** Genre and artist chips, fuzzy album search (just
  start typing), and a configurable, persisted sort order.
- **Cover Flow now-playing view.** The queue's covers on a reflective floor over a
  backdrop that glows with the current cover's colour, the album's track list alongside.
- **Playback.** A play queue with album runs, shuffle (by album or track), four repeat
  modes, seeking, and per-application volume driven through the OS mixer.
- **Desktop integration.** An MPRIS server (media keys and now-playing), and the session
  — queue, position, sort order — restored across runs.
- **Keyboard-driven.** Nearly everything is reachable from the keyboard (see below).

## Formats & platform

- **Audio:** WAV (8/16/24/32-bit integer and 32-bit float, mono or stereo) and Ogg Opus.
- **Cover art:** folder images (`cover.jpg`, `folder.png`, `front.*`, `albumart.*`).
- **Output:** PulseAudio, and PipeWire via pipewire-pulse, on Linux.

Not yet: FLAC and other formats, cover art embedded in the audio files, and audio output
beyond Linux/PulseAudio.

## Running

```sh
cargo run --release -p phonoscule-gui
```

## Configuration

The music directory is read from `~/.config/phonoscule.toml` (or
`$XDG_CONFIG_HOME/phonoscule.toml`):

```toml
music-dir = "~/Music"
```

Without a config file it defaults to `~/Music`. Relative paths resolve against the config
file's location; `~` and environment variables are expanded.

## Controls

- **Space** — play / pause (from anywhere)
- **Tab / Shift+Tab** — switch between the Library and Player views
- In the Library: **type** to search albums, **Ctrl+F** to focus the search, **Ctrl+W**
  to clear the filters; **Enter** opens an album's track list, and **Ctrl+Enter** /
  **Alt+Enter** play / queue everything currently shown
- On a selected album: **Ctrl+Space** plays it, **Alt+Space** queues it
- In the Player: **←/→** seek, **↑/↓** volume, **Home/End** previous/next track,
  **PageUp/PageDown** previous/next album, **Ctrl+Home/End** jump to the queue's ends
- **Alt+R** cycles the repeat mode; **Alt+S** / **Alt+Z** shuffle albums / tracks (hold
  **Ctrl** to shuffle the whole queue); **Ctrl+K** clears the queue
- The mouse wheel over the Player scrolls tracks (vertically) or albums (horizontally);
  over the volume bar it sets the volume

---

Part of the [Phonoscule](../README.md) project, under the MPL 2.0.
