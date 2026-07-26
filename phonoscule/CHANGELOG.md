# Changelog


v0.2.0 (2026-07-20)
------------------------------------------------------------

The framework as the first release of
[Phonoscule](../gui/CHANGELOG.md) was built on: the pieces a music player needs
between a file and a sound, and nothing above them. It takes an
[embedded-io async](https://crates.io/crates/embedded-io-async) reader and asks
for nothing else - no filesystem, no executor, no allocation it can avoid - so
the same code suits a desktop player and a microcontroller.

- **Tags.** Read as the file parses, and pushed to a callback as borrowed
  `Tag<'_>` values rather than collected into a map: title, artist, album, album
  artist, genre, track and disc number, and date. From WAV `LIST`/`INFO` chunks
  and Ogg Opus (Vorbis) comments. Numbers and dates arrive as their raw text,
  since "3/12" and a bare year both occur in the wild and guessing is the
  consumer's business. Opus streams can be parsed for their headers alone, so
  reading a library's tags need not decode any audio.
- **Decoding.** WAV in 8, 16, 24 and 32-bit integer and 32-bit float, mono or
  stereo, and Ogg Opus through the pure-Rust
  [opuscule](https://codeberg.org/jojo-laplace/opuscule) decoder.
- **Seeking.** Sample-accurate. Opus seeks by bisecting granule positions rather
  than decoding and discarding from the start, and finds a stream's length by
  scanning its tail where the input can seek.
- **Sample conversion.** Whatever a file holds, converted to what an output
  wants, with the common case - 16-bit stereo to 16-bit stereo - passing
  straight through.
- **Plumbing.** A small `Source`/`Sink` layer for wiring a decoder to whatever
  plays it.

Not yet: FLAC, Ogg Vorbis, and other formats. Released under the Mozilla Public
License 2.0.
