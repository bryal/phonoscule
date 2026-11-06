//! The wire protocol between the Kotlin shell and the Rust core.
//!
//! Kotlin mirrors these types by hand in `app/src/Core.kt`. There is exactly one JNI function, and
//! it carries one encoded [`Request`] in and one encoded [`Response`] out. That keeps the JNI layer
//! fixed while the protocol grows, and keeps everything below it in safe Rust that `cargo test` can
//! run on the host.
//!
//! Two requests block until there is something to say: [`Request::NextOp`], which is how Kotlin
//! learns what to read from the listener's music for the core, and [`Request::NextNotices`], which
//! is how it learns what has changed. Kotlin keeps threads of its own parked on them.
//!
//! Ids are 64-bit hashes, sent as `i64` with the same bits, because `org.json` reads an integer past
//! `i64::MAX` as a lossy double.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, anyhow};
use phonoscule::library::{Album, Rgb};
use phonoscule::player::Repeat;
use phonoscule::queue::{Grouping, Scope};
use phonoscule::sort::SortOrder;
use serde::{Deserialize, Serialize};

use crate::model::{self, AlbumRef, Core, Paths};
use crate::saf::{Answer, Op};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    /// The core's version, target architecture and build profile.
    Version,

    /// Load the library and the session the last run left, and ready the engine. Every request
    /// below needs this first. Later calls do nothing, as when the Activity is rebuilt.
    /// `files` is for what must survive a cache wipe, `cache` for what a rescan rebuilds.
    Start {
        files: String,
        cache: String,
    },

    /// The next piece of I/O for Kotlin to do. Blocks until there is one.
    NextOp,
    /// What the op numbered `id` came to.
    Answer {
        id: u64,
        answer: Answer,
    },

    /// What has changed since the last call. Blocks until something has.
    NextNotices,

    /// Scan the picked folder, or queue one more scan if one is running.
    Scan,

    /// The albums matching `query`, limited to `genre` unless it is absent. An empty `query`
    /// matches every album.
    Albums {
        #[serde(default)]
        query: String,
        #[serde(default)]
        genre: Option<String>,
    },
    /// One album with its tracks. The response is `None` if there is no album by that id.
    Album {
        #[serde(with = "id")]
        id: u64,
    },
    /// Every genre some album has, sorted.
    Genres,
    /// Every order the albums can be listed in, and which is current.
    Sorts,
    /// List the albums in order `index` of what [`Request::Sorts`] answered.
    SetSort {
        index: usize,
    },
    /// The cover with this id, as a JPEG file. Blocks while the file is made from the artwork.
    Cover {
        #[serde(with = "id")]
        id: u64,
    },

    /// The play queue, in tracks and in album runs.
    Queue,
    /// Replace the queue with the tracks of `albums`, and play from track `track` of the first.
    Play {
        #[serde(with = "ids")]
        albums: Vec<u64>,
        #[serde(default)]
        track: usize,
    },
    /// Add the tracks of `albums` to the end of the queue, or only track `track` of the one album.
    Enqueue {
        #[serde(with = "ids")]
        albums: Vec<u64>,
        #[serde(default)]
        track: Option<usize>,
    },
    Clear,
    /// Shuffle the queue in place, by `albums` or by track, and `all` of it or all but what plays.
    Shuffle {
        albums: bool,
        all: bool,
    },

    /// What is playing.
    Playback,
    /// Play the track at queue index `index`.
    JumpTo {
        index: usize,
    },
    SetPlaying {
        playing: bool,
    },
    TogglePlaying,
    Next,
    Prev,
    Seek {
        ms: u64,
    },
    SetRepeat {
        repeat: RepeatMode,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    Version {
        /// `Cargo.toml`'s `package.version`.
        core_version: String,
        /// The architecture the `.so` was built for, such as `aarch64`.
        arch: String,
        /// `debug` or `release`.
        profile: String,
    },
    /// The request was carried out, and there is nothing to say about it.
    Done,
    Op {
        op: Op,
    },
    Notices {
        library: bool,
        queue: bool,
        /// Whether a scan is running.
        scanning: bool,
        #[serde(with = "ids")]
        covers: Vec<u64>,
        /// What is playing, if that changed.
        playback: Option<Playback>,
    },
    Albums {
        albums: Vec<AlbumSummary>,
    },
    Album {
        album: Option<AlbumDetail>,
    },
    Genres {
        genres: Vec<String>,
    },
    Sorts {
        labels: Vec<String>,
        current: usize,
    },
    Cover {
        /// `None` if the artwork has not been found by a scan yet, or will not decode.
        path: Option<String>,
    },
    /// What plays in it is [`Playback::current`].
    Queue {
        tracks: Vec<QueueTrack>,
        runs: Vec<Run>,
    },
    Playback {
        playback: Playback,
    },
    /// A failure that should not happen, such as a request the core cannot parse. Kotlin shows
    /// `message` rather than crashing.
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlbumSummary {
    #[serde(with = "id")]
    pub id: u64,
    pub title: String,
    pub artist: String,
    /// Empty when no track has a genre tag.
    pub genre: String,
    pub year: Option<u32>,
    #[serde(with = "opt_id")]
    pub cover: Option<u64>,
    /// The cover's most distinct colour as `0xRRGGBB`, once known.
    pub accent: Option<u32>,
    pub tracks: usize,
}

impl AlbumSummary {
    fn of(album: &Album) -> Self {
        AlbumSummary {
            id: album.id,
            title: album.title.clone(),
            artist: album.artist.clone(),
            genre: album.genre.clone(),
            year: album.year,
            cover: album.cover_id,
            accent: album.accent.map(rgb),
            tracks: album.tracks.len(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlbumDetail {
    #[serde(flatten)]
    pub summary: AlbumSummary,
    /// The track titles, in album order.
    pub track_titles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueTrack {
    pub title: String,
    /// Zero while the scan has not found the track's album yet.
    #[serde(with = "id")]
    pub album: u64,
}

/// A run of queue tracks from one album: one cover in the cover flow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub start: usize,
    pub end: usize,
    pub album: Option<AlbumHeader>,
}

/// Enough of an album to show it beside something else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlbumHeader {
    #[serde(with = "id")]
    pub id: u64,
    pub title: String,
    pub artist: String,
    #[serde(with = "opt_id")]
    pub cover: Option<u64>,
    pub accent: Option<u32>,
}

impl AlbumHeader {
    fn of(album: AlbumRef) -> Self {
        AlbumHeader {
            id: album.id,
            title: album.title,
            artist: album.artist,
            cover: album.cover,
            accent: album.accent.map(rgb),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playback {
    /// The queue index of the track, or `None` while the queue is empty.
    pub current: Option<usize>,
    pub playing: bool,
    pub position_ms: u64,
    pub length_ms: Option<u64>,
    pub repeat: RepeatMode,
    pub title: String,
    pub album: Option<AlbumHeader>,
    /// The album's cover as a JPEG file, if one has been made.
    pub cover_file: Option<String>,
}

impl Playback {
    fn of(playback: model::Playback) -> Self {
        Playback {
            current: playback.current,
            playing: playback.playing,
            position_ms: playback.position.as_millis() as u64,
            length_ms: playback.length.map(|len| len.as_millis() as u64),
            repeat: playback.repeat.into(),
            title: playback.title,
            album: playback.album.map(AlbumHeader::of),
            cover_file: playback.cover_file.map(|path| path.to_string_lossy().into_owned()),
        }
    }
}

/// [`Repeat`], in the protocol's spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    Off,
    Track,
    Album,
    Playlist,
}

impl From<Repeat> for RepeatMode {
    fn from(repeat: Repeat) -> Self {
        match repeat {
            Repeat::Off => RepeatMode::Off,
            Repeat::Track => RepeatMode::Track,
            Repeat::Album => RepeatMode::Album,
            Repeat::Playlist => RepeatMode::Playlist,
        }
    }
}

impl From<RepeatMode> for Repeat {
    fn from(repeat: RepeatMode) -> Self {
        match repeat {
            RepeatMode::Off => Repeat::Off,
            RepeatMode::Track => Repeat::Track,
            RepeatMode::Album => Repeat::Album,
            RepeatMode::Playlist => Repeat::Playlist,
        }
    }
}

fn rgb(color: Rgb) -> u32 {
    let channel = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u32;
    channel(color.r) << 16 | channel(color.g) << 8 | channel(color.b)
}

static CORE: OnceLock<Core> = OnceLock::new();

/// Decode a request, carry it out, and encode the response.
///
/// Every failure becomes a [`Response::Error`], so the returned bytes always parse on the Kotlin
/// side.
pub fn handle(request: &[u8]) -> Vec<u8> {
    let response = serde_json::from_slice::<Request>(request)
        .context("Failed to parse request")
        .and_then(dispatch)
        .unwrap_or_else(|err| Response::Error { message: format!("{err:?}") });
    encode(&response)
}

pub fn dispatch(request: Request) -> anyhow::Result<Response> {
    let core = match request {
        Request::Version => {
            return Ok(Response::Version {
                core_version: env!("CARGO_PKG_VERSION").to_owned(),
                arch: std::env::consts::ARCH.to_owned(),
                profile: if cfg!(debug_assertions) { "debug" } else { "release" }.to_owned(),
            });
        }
        Request::Start { files, cache } => {
            let mut fresh = false;
            let core = CORE.get_or_init(|| {
                fresh = true;
                Core::new(Paths::new(&PathBuf::from(files), &PathBuf::from(cache)))
            });
            if fresh {
                core.follow_engine();
            }
            return Ok(Response::Done);
        }
        request => (CORE.get().ok_or_else(|| anyhow!("the core has not been started"))?, request),
    };
    Ok(match core {
        (_, Request::Version | Request::Start { .. }) => unreachable!("answered above"),
        (core, Request::NextOp) => Response::Op { op: core.saf().next_op() },
        (core, Request::Answer { id, answer }) => {
            core.saf().answer(id, answer);
            Response::Done
        }
        (core, Request::NextNotices) => {
            let changes = core.wait_notices();
            Response::Notices {
                library: changes.library,
                queue: changes.queue,
                scanning: core.scanning(),
                covers: changes.covers.into_iter().collect(),
                playback: changes.playback.then(|| Playback::of(core.playback())),
            }
        }
        (core, Request::Scan) => {
            core.scan();
            Response::Done
        }
        (core, Request::Albums { query, genre }) => {
            let albums = core.albums(&query, genre.as_deref());
            Response::Albums { albums: albums.iter().map(AlbumSummary::of).collect() }
        }
        (core, Request::Album { id }) => Response::Album {
            album: core.album(id).map(|album| AlbumDetail {
                summary: AlbumSummary::of(&album),
                track_titles: album.tracks.iter().map(|t| t.title.clone()).collect(),
            }),
        },
        (core, Request::Genres) => Response::Genres { genres: core.genres() },
        (core, Request::Sorts) => Response::Sorts {
            labels: SortOrder::ALL.iter().map(|sort| sort.label()).collect(),
            current: SortOrder::ALL.iter().position(|&sort| sort == core.sort()).unwrap_or(0),
        },
        (core, Request::SetSort { index }) => {
            core.set_sort(*SortOrder::ALL.get(index).context("no such sort order")?);
            Response::Done
        }
        (core, Request::Cover { id }) => {
            Response::Cover { path: core.cover(id).map(|path| path.to_string_lossy().into_owned()) }
        }
        (core, Request::Queue) => {
            let (tracks, runs) = core.queue();
            Response::Queue {
                tracks: tracks.into_iter().map(|item| QueueTrack { title: item.title, album: item.album_id }).collect(),
                runs: runs
                    .into_iter()
                    .map(|run| Run { start: run.start, end: run.end, album: run.album.map(AlbumHeader::of) })
                    .collect(),
            }
        }
        (core, Request::Play { albums, track }) => {
            core.play(&albums, track);
            Response::Done
        }
        (core, Request::Enqueue { albums, track }) => {
            core.enqueue(&albums, track);
            Response::Done
        }
        (core, Request::Clear) => {
            core.clear();
            Response::Done
        }
        (core, Request::Shuffle { albums, all }) => {
            let grouping = if albums { Grouping::Albums } else { Grouping::Tracks };
            core.shuffle(grouping, if all { Scope::All } else { Scope::Others });
            Response::Done
        }
        (core, Request::Playback) => Response::Playback { playback: Playback::of(core.playback()) },
        (core, Request::JumpTo { index }) => {
            core.jump_to(index);
            Response::Done
        }
        (core, Request::SetPlaying { playing }) => {
            core.set_playing(playing);
            Response::Done
        }
        (core, Request::TogglePlaying) => {
            core.toggle_playing();
            Response::Done
        }
        (core, Request::Next) => {
            core.next();
            Response::Done
        }
        (core, Request::Prev) => {
            core.prev();
            Response::Done
        }
        (core, Request::Seek { ms }) => {
            core.seek(Duration::from_millis(ms));
            Response::Done
        }
        (core, Request::SetRepeat { repeat }) => {
            core.set_repeat(repeat.into());
            Response::Done
        }
    })
}

/// Encode a bare error. For the JNI layer, whose own failures (a bad array, a panic) never become a
/// [`Request`].
pub fn encode_error(message: impl Into<String>) -> Vec<u8> {
    encode(&Response::Error { message: message.into() })
}

/// Encode a response. If that somehow fails, encode an error about it instead, which cannot fail
/// because an error is a single string field.
fn encode(response: &Response) -> Vec<u8> {
    serde_json::to_vec(response).unwrap_or_else(|e| {
        let fallback = Response::Error { message: format!("failed to encode response: {e}") };
        serde_json::to_vec(&fallback).expect("encoding a plain Error response cannot fail")
    })
}

/// A 64-bit id as the `i64` with the same bits.
mod id {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(id: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_i64(*id as i64)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        i64::deserialize(d).map(|id| id as u64)
    }
}

mod opt_id {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(id: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
        match id {
            Some(id) => s.serialize_some(&(*id as i64)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
        Option::<i64>::deserialize(d).map(|id| id.map(|id| id as u64))
    }
}

mod ids {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(ids: &[u64], s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(ids.iter().map(|&id| id as i64))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u64>, D::Error> {
        Vec::<i64>::deserialize(d).map(|ids| ids.into_iter().map(|id| id as u64).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_request_round_trips() {
        let request = serde_json::to_vec(&Request::Version).unwrap();
        let response: Response = serde_json::from_slice(&handle(&request)).unwrap();
        let Response::Version { core_version, .. } = response else {
            panic!("expected a Version response, got {response:?}");
        };
        assert_eq!(core_version, env!("CARGO_PKG_VERSION"));
    }

    /// Kotlin builds and reads these shapes by hand, so pin them exactly rather than just
    /// round-tripping.
    #[test]
    fn encodings_are_the_tagged_forms_kotlin_expects() {
        let play = Request::Play { albums: vec![u64::MAX], track: 2 };
        assert_eq!(serde_json::to_string(&play).unwrap(), r#"{"kind":"play","albums":[-1],"track":2}"#);
        let answer = Request::Answer { id: 3, answer: Answer::Opened { fd: 7 } };
        assert_eq!(serde_json::to_string(&answer).unwrap(), r#"{"kind":"answer","id":3,"answer":{"kind":"opened","fd":7}}"#);
        let op = Response::Op { op: Op::ReadDir { id: 1, path: "/a".into() } };
        assert_eq!(serde_json::to_string(&op).unwrap(), r#"{"kind":"op","op":{"kind":"read_dir","id":1,"path":"/a"}}"#);
        let repeat = Request::SetRepeat { repeat: RepeatMode::Album };
        assert_eq!(serde_json::to_string(&repeat).unwrap(), r#"{"kind":"set_repeat","repeat":"album"}"#);
        let cover = Response::Cover { path: None };
        assert_eq!(serde_json::to_string(&cover).unwrap(), r#"{"kind":"cover","path":null}"#);
    }

    #[test]
    fn ids_keep_their_bits() {
        let r: Request = serde_json::from_str(r#"{"kind":"album","id":-2}"#).unwrap();
        assert_eq!(r, Request::Album { id: u64::MAX - 1 });
    }

    #[test]
    fn accent_packs_as_rgb() {
        assert_eq!(rgb(Rgb { r: 1.0, g: 0.0, b: 0.5 }), 0xff0080);
    }

    #[test]
    fn a_request_before_start_is_an_error_not_a_panic() {
        let response: Response = serde_json::from_slice(&handle(br#"{"kind":"queue"}"#)).unwrap();
        assert!(matches!(response, Response::Error { .. }), "got {response:?}");
    }

    #[test]
    fn malformed_request_yields_an_error_response() {
        let response: Response = serde_json::from_slice(&handle(b"not json at all")).unwrap();
        assert!(matches!(response, Response::Error { .. }), "got {response:?}");
    }
}
