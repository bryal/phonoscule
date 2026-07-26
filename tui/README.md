# Phonoscule (terminal player)

The terminal counterpart of the [graphical player](../gui/README.md),
built on the same [Phonoscule](../README.md) framework:
album-centric browsing and playback in a terminal,
with real cover art where the terminal can draw it.

Intended primarily as a fallback for machines the graphical player is too much for -
single-board computers, older laptops, anything without a GPU worth the name -
and for people who simple like having all their stuff in the terminal.

## Features

- **Album-centric browsing.**
  A list of albums with the selected one's cover, tags and tracks alongside.
  Albums are grouped from the files' tags (album artist + album title)
  rather than the directory layout, so a multi-disc album spread across folders is one album,
  and two same-named albums by different artists stay apart.
- **Cover art in the terminal.**
  Through kitty, sixel or iTerm2 graphics where the terminal speaks one,
  and unicode half blocks where it does not.
  The player draws the artwork itself; the browser a thumbnail beside its track list.
- **Filtering, search, and sorting.**
  Genre, artist and order pickers, each with its own search,
  and fuzzy album search by just starting to type.
- **Track granularity.**
  Open an album to play or queue one track at a time,
  or take the album, or everything the filter lets through.
- **Playback.**
  A play queue grouped into album runs, shuffle (by album or track, all or all-but-playing),
  four repeat modes, seeking, and per-application volume through the OS mixer.
- **Desktop integration.**
  An MPRIS server, so media keys, `playerctl` and desktop now-playing widgets work,
  and the session - queue, position, repeat mode, order - restored across runs.
- **Bounded cover art.**
  Artwork is loaded as it is shown, into caches of fixed size,
  and decoded and encoded off the drawing thread so scrolling never waits for it.
  A 759-album library costs about 38 MiB browsing and 51 MiB playing.
  Only the album list itself grows with the library, by roughly a megabyte per
  750 albums - the artwork's share is fixed however much music you have.

## Formats & platform

- **Audio:** WAV (8/16/24/32-bit integer and 32-bit float, mono or stereo) and Ogg Opus.
- **Cover art:** folder images (`{cover,folder,front,albumart}.{jpg,jpeg,png,webp}`).
- **Output:** PulseAudio, and PipeWire via pipewire-pulse, on Linux.
- **Terminal:** any; cover art is sharper on one that speaks an image protocol.
  Detection is `ratatui-image`'s; the half-block fallback needs nothing of the terminal.

Not yet: FLAC and Ogg Vorbis and other formats as well as audio output beyond Linux/PulseAudio.

## Configure

Settings are read from `~/.config/phonoscule.toml`
(or `$XDG_CONFIG_HOME/phonoscule.toml`):

```toml
music-dir = "~/Music"

[app.tui]
image-protocol = "kitty"
```

`music-dir` is the shared setting;
this player's own settings live in its `[app.tui]` table,
so the graphical player reading the same file keeps its own under `[app.gui]`.
Pass a path as the first argument, or set `$PHONOSCULE_TUI_CONF`,
to read a different file instead.

Without a config file the music directory defaults to `~/Music`.
Relative paths resolve against the config file's location.
`~` and environment variables are expanded.

- **`music-dir`** - path to the music library.
- **`[app.tui] image-protocol`** - draw cover art with this protocol
  (`kitty`, `sixel`, `iterm2` or `halfblocks`)
  instead of asking the terminal which it speaks. Optional.

Setting it is worth knowing about for a second reason.
The detection asks the terminal and waits for a reply,
on a thread that goes on reading the keyboard until one arrives -
so in a terminal that answers slowly, or not at all,
the first couple of seconds of typing are swallowed.
Naming the protocol skips the question entirely.

## Run

```sh
cargo run --release -p phonoscule-tui
```

A debug build works, but decoding and encoding cover art is many times slower in one,
so the artwork takes visibly longer to appear.

## Install

```sh
# From this directory
cargo install --path .
# Or from the parent directory
cargo install --path tui
```

If `~/.cargo/bin` is in your `$PATH`,
you should now be able to launch `phonoscule-tui` from the terminal.

## Controls

Letters carry no bindings of their own in the browser - typing searches - so everything
else is a chord. Anywhere:

| Key | |
| --- | --- |
| **Space** | play / pause |
| **Tab** / **Shift+Tab** | switch between the Library and Player views |
| **Ctrl+Q** / **Ctrl+C** | quit |
| **Alt+R** | cycle the repeat mode (off, track, album, queue) |
| **Ctrl+S** / **Ctrl+Z** | shuffle the whole queue, by album / by track |
| **Alt+S** / **Alt+Z** | shuffle everything but what is playing, by album / by track |
| **Ctrl+K** | clear the queue |

In the Library:

| Key | |
| --- | --- |
| **↑ ↓ PgUp PgDn Home End** | move the selection |
| **type anything** | search album titles, narrowing as you go |
| **Backspace** | correct the search |
| **Ctrl+F** | back to the search |
| **Ctrl+G** / **Ctrl+T** / **Ctrl+O** | pick a genre / an artist / an order |
| **Ctrl+W** | clear every filter |
| **Enter** | open the selected album's tracks |
| **Alt+Enter** | queue the selected album |
| **Ctrl+A** / **Alt+A** | play / queue every album shown |

In an album's tracks:

| Key | |
| --- | --- |
| **↑ ↓ PgUp PgDn Home End** | move the selection |
| **Enter** | play that track alone |
| **Alt+Enter** | queue it, stepping onto the next |
| **Ctrl+A** / **Alt+A** | play / queue the whole album |
| **Esc** | close |

In a picker: **type** to search it, **↑ ↓** to move, **Enter** to take it, **Esc** to leave.
Its first row clears the filter it sets.

In the Player:

| Key | |
| --- | --- |
| **← →** | seek |
| **↑ ↓** | volume |
| **Home** / **End** | previous / next track |

## License

Released under the **Mozilla Public License 2.0** (see [`LICENSE`](../LICENSE)).
Copyright (c) 2026 Jojo <jo@jo.zone>.
