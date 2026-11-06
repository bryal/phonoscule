# Phonoscule (framework)

A framework for building music players:
a pure-Rust library of the reusable backend pieces -
reading tags, decoding audio, seeking, sample-format conversion,
and a small source/sink plumbing layer to connect a decoder to an output.

It needs only an [embedded-io async](https://crates.io/crates/embedded-io-async) reader,
is `no_std`-friendly and light on allocation,
and is intended to run anywhere from a desktop GUI down to a headless microcontroller.

## What it does

- **Tags.** Reads metadata as it parses -
  title, artist, album, album artist, genre, track/disc number, and date -
  from WAV `LIST`/`INFO` chunks and Ogg Opus (Vorbis) comments.
- **Decoding.** WAV (8/16/24/32-bit integer and 32-bit float, mono or stereo)
  and Ogg Opus, the latter via the pure-Rust
  [opuscule](https://github.com/bryal/opuscule) decoder.
- **Playback plumbing.** Sample-accurate seeking, sample-format conversion,
  and a small `Source`/`Sink` layer for wiring a decoder to whatever plays it.

FLAC and Ogg Vorbis are on the roadmap, but not included as of yet.

## Optional pieces

The core above is always built and stays portable.
Everything a *hosted* player wants on top of it - and every heavy dependency -
sits behind a cargo feature, all off by default,
so enabling none of them leaves the portable core exactly as it is.

| Feature | Module | What it brings |
| --- | --- | --- |
| `config` | `config` | A `phonoscule.toml` config file, for players that want one. |
| `dirs` | `dirs` | The platform's directories for settings, state and caches. |
| `fs` | `fs` | Reading the music through something other than plain paths, such as Android's Storage Access Framework. |
| `library` | `library` | Scanning a music directory into albums, with a tag cache and cover thumbnails. |
| `sort` | `sort` | Album ordering. |
| `search` | `search` | Fuzzy text ranking, for searching a library. |
| `queue` | `queue` | Shuffling a play queue, and finding its album runs. |
| `sink` | `sink` | Audio output. |
| `player` | `player` | The play-queue engine, playing through `sink`. |
| `volume` | `volume` | Per-application volume through the OS mixer. |
| `session` | `session` | Persisting the queue and the state around it across runs. |
| `watcher` | `watcher` | Watching the music directory for changes. |
| `media` | `media`, `mpris`, `smtc` | Media keys and desktop now-playing. |

These want `std` and, mostly, a filesystem.
`sink` speaks PulseAudio on Linux, WASAPI on Windows and AAudio on Android;
`volume` speaks PulseAudio or the Windows mixer, and `media` D-Bus or the Windows media controls.
Each module's documentation says what it needs.

## Example applications

The repository this crate lives in contains four players built on this library:

- `phonoscule-gui`, the graphical reference application - see [`gui/README.md`](../gui/README.md).
- `phonoscule-tui`, its terminal counterpart - see [`tui/README.md`](../tui/README.md).
- An Android app, reading the music through `fs` - see [`android/README.md`](../android/README.md).
- `phonoscule-cli`, a minimal example that plays the files named on its command line.
  It uses the core alone: no library scan, no config file, no session.

## License

Released under the **Mozilla Public License 2.0** (see [`LICENSE`](../LICENSE)).
Copyright (c) 2026 Jojo <jo@jo.zone>.
