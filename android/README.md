Phonoscule for Android
============================================================
Android version of `phonoscule-gui`: the same album-centric player on a phone.
Not a port of the desktop player but a sibling of it, the way the
[terminal player](../tui/README.md) is. A wall of covers to browse, search and
filter by genre and artist, and a Cover Flow to play into - stood on end, since
a phone is taller than it is wide.

Status: in daily use on one phone, a Galaxy S23 on Android 16.


Features
------------------------------------------------------------
- **Library.** A grid of album covers, grouped from the tags exactly as on the
  desktop, under a translucent header tinted with the playing cover's colour.
  Fuzzy search over titles and artists, and the desktop's sort orders. A row
  each for genres and artists shows what is picked; open one to pick from all
  of them: tap a value to show only it, long-press to add it. Back clears the
  search and the picks before it leaves the app. Long-press an album to enqueue
  it.
- **Scanning.** Pull down to scan the folder again, which carries on in the
  background with a notification counting the albums. Nothing scans unasked:
  through SAF a scan costs the system tens of seconds of CPU. A folder that
  cannot be read keeps its albums rather than losing them.
- **Album.** The cover and tags, Play and Enqueue, and the tracks: tap one to
  play the album from it, or enqueue it alone.
- **Player.** The queue's albums as a vertical Cover Flow over a backdrop glowing
  with the playing cover's colour. As on the desktop, the album the flow rests
  on plays: drag or fling to another, or tap one. Tap the playing album to pause
  and resume. The playing album's track names hang from the top; drag sideways
  across the flow to pick one, or flick to step a track, on into the next album
  at either end. Beneath, the title, repeat, previous/play/next, the four
  shuffles, and the seek bar.
- **Starts where you left off.** With a queue, the app opens on the player. The
  queue, position, repeat mode and sort order are restored across runs.
- **The desktop's font**, Iosevka, copied in from the GUI's assets at build
  time. What it lacks, such as Japanese, falls back to the system's font.
- **Playback in the background.** A media session puts the player on the lock
  screen, in the notification shade, and under the headset's buttons. It pauses
  when another app takes the audio or the headphones come out.


Architecture
------------------------------------------------------------
A Rust core with a thin Kotlin/Compose shell, talking over a single JNI
function, in the shape of neuma-android.

    Kotlin / Compose              one JNI fn            Rust
    +----------------------+                    +------------------------+
    | Compose UI           |                    | proto.rs               |
    | media session        | <-- JSON bytes --> | model.rs: library,     |
    | SAF: lists, opens    |                    |   queue, engine        |
    +----------------------+                    | saf.rs: the Fs         |
        owns all I/O on                         +------------------------+
        the music                                 phonoscule: scan, tags,
                                                  decoding, AAudio, session

Everything that makes a player is the `phonoscule` crate's, shared with the
desktop players: the library scan and its caches, the tag reading, the Opus and
WAV decoders, the play engine, shuffling, sorting, searching, and the saved
session. Audio goes out through AAudio, from `phonoscule::sink`.

What differs is how the music is reached. Android hands an app its music
folder through the Storage Access Framework, which lists directories and opens
documents but has no paths. So the library and the engine read through
`phonoscule::fs::Fs`, and this crate's `Fs` turns each listing or open into an
op that Kotlin performs: a few worker threads sit in `Core.nextOp()`, list the
directory or open the document, and answer with the entries or a file
descriptor. Paths are names within the picked folder - `/Artist/Album/01.opus` -
which Kotlin resolves to documents from the listings it has made. Kotlin never
gets called into: it asks.

It learns what changed the same way, with a thread parked in
`Core.nextNotices()`. The screens observe a few counters bumped from there and
ask the core again.

The core's own files - caches and the saved session - are written by Rust at
paths under the app's private directories, which Kotlin gives it at startup.
Covers reach the screens as the JPEGs of the desktop player's full-size cover
cache, which Android decodes at a fraction of their size for the grid.

The FFI boundary is exactly one symbol, `Java_zone_jo_phonoscule_Core_call`,
taking and returning a byte array. Everything behind it is safe Rust, which is
why the whole protocol is exercised by `cargo test` on the host, the SAF bridge
included, with a thread playing Kotlin's part. The symbol name encodes the
Kotlin package and class, so renaming `zone.jo.phonoscule.Core` means renaming
the function in `src/jni.rs` too.


Building
------------------------------------------------------------
Run `just` to list the available recipes:

    just build              # debug APK
    just run                # install onto the connected device and launch it
    just test               # host-side Rust tests, no device needed
    just logs               # follow the running app's log output
    just gradle <task>...   # arbitrary Gradle tasks

Gradle invokes `cargo ndk` itself via the `cargoNdk` task, so there is no
separate Rust build step. That task deliberately has no up-to-date check; it
always runs and lets cargo decide what actually needs rebuilding. It always
builds the core in release, debug APK or not: an unoptimised Opus decoder
cannot keep up with playback on a phone.

The crate is a member of the repository's Cargo workspace, so it shares the
lockfile and the `target/` directory with the desktop players.

The launcher icon is rendered from the desktop player's artwork stood on end
(`gui/assets/icon/phonoscule-vertical.svg`) at build time with `rsvg-convert`, so that
has to be installed. On Arch it comes with `librsvg`.


Development setup
------------------------------------------------------------
The same as neuma-android's: a JDK 21 (see `app/gradle.properties`), the
Android SDK with `platforms;android-36`, `build-tools;36.0.0` and
`ndk;27.2.12479018` under `~/Android/Sdk`, and

    rustup target add aarch64-linux-android
    cargo install cargo-ndk

Only `arm64-v8a` is built. It targets a phone, and there is no emulator
support, so the other ABIs would be dead cross-compile time.


License
------------------------------------------------------------
Released under the **Mozilla Public License 2.0**, like the rest of the
repository (see [`LICENSE`](../LICENSE)).
