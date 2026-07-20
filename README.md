# Phonoscule

Phonoscule is a framework for building music players: a Rust library of the reusable
backend pieces — reading tags, decoding audio, seeking, sample-format conversion, and a
small source/sink plumbing layer to connect a decoder to an output. It needs only an
async ([embedded-io](https://crates.io/crates/embedded-io-async)) reader, is
`no_std`-friendly and light on allocation, and is built to run anywhere from a desktop
application down to a microcontroller.

It is also the name of the reference application built on that framework: **[Phonoscule
GUI](gui)**, a graphical music player that arranges your library by its cover art. Most
of what makes that app pleasant to use is a choice of the reference application, not of
the framework itself, so its details live in [`gui/README.md`](gui/README.md).

## The framework (`phonoscule`)

- **Tags.** Reads metadata as it parses — title, artist, album, album artist, genre,
  track/disc number, and date — from WAV `LIST`/`INFO` chunks and Ogg Opus (Vorbis)
  comments.
- **Decoding.** WAV (8/16/24/32-bit integer and 32-bit float, mono or stereo) and Ogg
  Opus, the latter via the sibling [opuscule](https://codeberg.org/jojo-laplace/opuscule)
  decoder.
- **Playback plumbing.** Sample-accurate seeking, sample-format conversion, and a small
  `Source`/`Sink` layer for wiring a decoder to whatever plays it.
- **Portable.** `no_std`-friendly and async; hand it an `embedded-io` reader and it runs,
  desktop to microcontroller.

FLAC and other formats are on the roadmap, not in this release.

## The reference application (`phonoscule-gui`)

A graphical, album-art-centric music player — a lightweight native alternative to
cover-grid MPD clients. Browse a wall of covers, filter and search, and play into a Cover
Flow-style now-playing view, with desktop media integration and keyboard-driven controls.

```sh
cargo run --release -p phonoscule-gui
```

It plays through PulseAudio (and PipeWire) on Linux. See [`gui/README.md`](gui/README.md)
for its features, configuration, and controls.

## A terminal example (`phonoscule-cli`)

A small worked example of driving the framework — a terminal player that takes files on
the command line, with a few keyboard controls. More an example than an application.

```sh
cargo run --release -p phonoscule-cli -- track.opus
```

## License

Released under the **Mozilla Public License 2.0** (see [`LICENSE`](LICENSE)).
Copyright (c) 2026 Jojo <jo@jo.zone>.

Opus decoding is provided by the sibling [opuscule](https://codeberg.org/jojo-laplace/opuscule)
crate (also MPL 2.0), itself a derivative of the libopus reference decoder; its upstream
BSD notices and Opus patent grants travel with that crate.
