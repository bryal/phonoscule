# Changelog


v0.4.0 (unreleased)
------------------------------------------------------------

**Windows support.** The framework was portable in its core and Linux-only the
moment a player wanted to hear anything: the play engine wrote straight to
PulseAudio, so every player in the workspace needed libpulse to link at all. The
three platform-facing modules now each have two backends, chosen at compile
time, behind interfaces that were already platform-neutral.

- **`sink`** (new) - audio output, extracted from `player`. One blocking `write`,
  because that blocking is what paces the engine. PulseAudio on Linux (PipeWire
  included, via pipewire-pulse); WASAPI in shared mode on Windows, opened at the
  track's own rate with `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM` so the audio engine
  resamples to the device's - the same division of labour we relied on Pulse for.
  Behind its own feature, so a program that drives the decoders itself can play
  without taking the queue engine too.
  - A wall-clock-paced silent fallback, so a machine with no usable device
    advances its queue at playback speed instead of tearing through it.
  - A write error reopens the stream, so unplugging a headset or switching the
    default endpoint resumes on the new device within a chunk.
- **`volume`** - the mixerless fallback on Windows is now a real backend:
  `ISimpleAudioVolume` on the audio sessions `sink` opens for this process, which
  is the per-application slider the Windows volume mixer shows. Same process-id
  test as the PulseAudio side.
- **`media`** (new) - what `mpris` was to an application: the snapshot types, the
  handle, and the worker that coalesces published snapshots. It dispatches to
  `mpris`, unchanged in substance and still public, or to `smtc` on Windows - the
  now-playing flyout, the lock-screen card, and the media keys with them.
  Backends are told which half of a snapshot moved, so neither a properties
  signal nor a display-updater commit fires on a position tick.
  - The `mpris` feature becomes `media`. Applications that named the module
    directly want `media` now; the module remains for reaching at an MPRIS server
    on purpose.
  - zbus is no longer built on Windows, where it was reaching for a bus that
    cannot be there.
- **`dirs`** (new) - the platform's roots for settings, state and caches, so the
  players stop spelling them in XDG terms and landing dotfiles in a Windows user
  profile. XDG on Linux, `%APPDATA%` and `%LOCALAPPDATA%` on Windows. What goes
  in them is still the player's own business: it passes its name and decides its
  files.
- **`config`** - the default config path comes from `dirs`, and `config_help`
  names it as the platform spells it.


v0.3.0 (2026-07-26)
------------------------------------------------------------

Everything a *hosted* music player needs above the core, which until now lived
in the GUI where only the GUI could reach it. It moved here to be shared with
[phonoscule-tui](../tui/README.md), the terminal player released alongside.

The core is untouched, and stays what it was: enable none of the features below
and the crate pulls exactly the six dependencies it did at 0.2.0, so a
microcontroller build is no further away than it was.

Ten new modules, each behind a cargo feature and all off by default:

- **`config`** - the `phonoscule.toml` file, if a player wants one. Settings the
  framework reads sit at the top level; a player's own live in its
  `[app.<name>]` table, which the framework never looks inside, so several
  players can share one file. Each reads its own `$PHONOSCULE_<APP>_CONF`.
- **`library`** - scanning a music directory into albums. Albums are grouped by
  the files' tags (album artist + album title) rather than the directory layout,
  tags are cached between scans and validated by mtime and size, and cover art
  is found from folder images and cached as raw thumbnails. Results stream out
  as they are found.
- **`sort`** - album ordering, as a small serializable value a player can
  persist.
- **`search`** - fuzzy text ranking: every word of the query contained,
  contiguous hits ahead of scattered ones.
- **`queue`** - shuffling. Takes the album key of each slot and a seed, and
  returns a permutation, so albums can move as units or tracks singly, and the
  whole queue or everything but what is playing.
- **`player`** - the play-queue engine: a thread that owns the queue, decodes
  through the core and writes to PulseAudio, driven entirely over channels.
  Album runs, four repeat modes, seeking.
- **`volume`** - per-application volume through the OS mixer, so the setting is
  the audio server's to keep and an external mixer's changes are noticed.
- **`session`** - the queue and the state around it, across runs.
- **`watcher`** - the music directory noticed changing, debounced to one event
  per settled burst.
- **`mpris`** - an MPRIS server on the D-Bus session bus: media keys,
  `playerctl`, desktop now-playing.

Three of them are not simply the GUI's code relocated:

- **`library` no longer depends on a UI toolkit.** A cover's accent colour is a
  plain `Rgb`, and its thumbnail plain ref-counted bytes, so a consumer wraps
  them in whatever its own renderer wants. `decode_cover` takes the size to
  decode to, rather than always producing a fixed 900 square that anything
  drawing smaller would resize a second time. `read_thumbnail` reads one back by
  cover id, for a player that would rather load artwork as it shows it than hold
  a library's worth.
- **`queue::shuffle` is a pure function of a seed**, and returns a permutation
  rather than a reordered queue - which is also the only unambiguous answer when
  a queue holds the same track twice.
- **`config::Conf` can be built without a file**, which is what the module
  claims of itself: a player keeping its settings in a database, in
  non-volatile storage, or on its command line passes the same values in
  directly.

Nothing here decides where a player's files live. Paths are the caller's to
choose, so two players on one machine do not share a state directory by
accident.


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

Not yet: FLAC, Ogg Vorbis, and other formats.
