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
| `config` | `config` | The shared `phonoscule.toml` config file. |
| `library` | `library` | Scanning a music directory into albums, with a tag cache and cover thumbnails. |
| `sort` | `sort` | Album ordering. |
| `player` | `player` | The play-queue engine and its audio output. |
| `volume` | `volume` | Per-application volume through the OS mixer. |
| `session` | `session` | Persisting the queue and the state around it across runs. |
| `watcher` | `watcher` | Watching the music directory for changes. |
| `mpris` | `mpris` | An MPRIS server: media keys and desktop now-playing. |

These want `std` and, mostly, a filesystem;
`player` and `volume` speak PulseAudio, `mpris` speaks D-Bus.
Each module's documentation says what it needs.

## Reference applications

The repository this crate lives in contains two music players built on this
library: a graphical one, see [`gui/README.md`](../gui/README.md),
and a terminal one, see [`tui/README.md`](../tui/README.md).
They are the best worked examples of using the pieces above.

## License

Released under the **Mozilla Public License 2.0** (see [`LICENSE`](../LICENSE)).
Copyright (c) 2026 Jojo <jo@jo.zone>.
