# Phonoscule

Phonoscule is a lightweight music player library, which can be used in services, desktop applications, as well as embedded systems.

This library can't actually make your system audio device play any sound by itself,
but rather acts as a framework that provides you with most of the backend features needed in a music player, such as:

- Reading metadata / tags from audio files
- Browsing tracks by genre / artist / album / etc.
- Search
- Playback queue
- Playlists
- Reading .wav & decoding .opus files into samples / PCM data

This repo also contains `phonoscule-cli`, which is a simple CLI music player based on the `phonoscule` library.
It's essentially just a very thin wrapper that provides a REPL for interacting with the library, as well as actual playback on device on Linux using Pulseaudio.

## Roadmap 

- [ ] Demux .wav files to PCM data
- [ ] Demux .opus container
- [ ] Decode Opus stream to PCM
- [ ] Get tracks by genre, artist, & album
- [ ] Add track(s) to playback queue
- [ ] Play / pause
- [ ] Next / previous track in queue
- [ ] phonoscule-cli
- [ ] phonoscule-gui
