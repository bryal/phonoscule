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

## Reference application

The repository this crate lives in also contains a graphical music player,
*Phonoscule*, built on this library as its reference application -
see [`gui/README.md`](../gui/README.md).

## License

Released under the **Mozilla Public License 2.0** (see [`LICENSE`](../LICENSE)).
Copyright (c) 2026 Jojo <jo@jo.zone>.
