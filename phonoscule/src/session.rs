//! Persistence of a player's session across runs, split over two files in the XDG state directory:
//! `playlist.json` is the queue itself -- essentially just the list of tracks -- while `player.json`
//! holds the state around it (current index, repeat mode, and the library's sort order). The split
//! keeps the playlist a plain track list, one step away from a future M3U export/import, while the
//! volatile state churns in its own small file.
//!
//! Only paths are stored: tags and album grouping rehydrate from the library scan at boot. Both
//! files are saved best-effort on every change and loaded once at boot; the session restores paused
//! at the start of the current track.
//!
//! State directory, not cache: unlike the tag or cover caches, a queue can't be regenerated, so it
//! must survive a cache wipe.
//!
//! Wants std and a filesystem.

use crate::player::Repeat;
use crate::sort::SortOrder;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Bumped when [`SavedPlaylist`] changes shape; old or unreadable files restore an empty queue.
/// (v1 was the pre-split format carrying per-track tags.)
const PLAYLIST_VERSION: u32 = 2;
/// Bumped when [`SavedPlayer`] changes shape. (v2 added the library sort order.)
const PLAYER_VERSION: u32 = 2;

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct SavedPlaylist {
    version: u32,
    pub tracks: Vec<PathBuf>,
}

impl SavedPlaylist {
    pub fn new(tracks: Vec<PathBuf>) -> Self {
        SavedPlaylist { version: PLAYLIST_VERSION, tracks }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, Default)]
pub struct SavedPlayer {
    version: u32,
    pub current: usize,
    pub repeat: Repeat,
    pub sort: SortOrder,
}

impl SavedPlayer {
    pub fn new(current: usize, repeat: Repeat, sort: SortOrder) -> Self {
        SavedPlayer { version: PLAYER_VERSION, current, repeat, sort }
    }
}

/// A restored session, [`load`]ed and reconciled from both files.
#[derive(Clone, Default)]
pub struct Restored {
    pub tracks: Vec<PathBuf>,
    pub current: usize,
    pub repeat: Repeat,
    pub sort: SortOrder,
}

/// Loads and reconciles both files: tracks whose files have vanished since the last run are
/// dropped (the music directory may have changed in between) with `current` following its item to
/// its new index, and `current` is clamped to the queue. Missing, outdated, or unreadable files
/// degrade to their defaults (an empty queue; index 0, repeat off).
pub async fn load(playlist: Option<PathBuf>, player: Option<PathBuf>) -> Restored {
    let saved: SavedPlaylist =
        read_json(playlist, "playlist").await.filter(|p: &SavedPlaylist| p.version == PLAYLIST_VERSION).unwrap_or_default();
    let state: SavedPlayer =
        read_json(player, "player state").await.filter(|p: &SavedPlayer| p.version == PLAYER_VERSION).unwrap_or_default();

    let mut tracks = Vec::with_capacity(saved.tracks.len());
    let mut current = state.current;
    for (ix, track) in saved.tracks.into_iter().enumerate() {
        if smol::fs::metadata(&track).await.is_ok() {
            tracks.push(track);
        } else if ix < state.current {
            // A dropped track before the current one shifts it back one slot.
            current -= 1;
        }
    }
    let current = current.min(tracks.len().saturating_sub(1));
    Restored { tracks, current, repeat: state.repeat, sort: state.sort }
}

pub async fn save_playlist(path: Option<PathBuf>, playlist: SavedPlaylist) {
    save_json(path, &playlist).await;
}

pub async fn save_player(path: Option<PathBuf>, player: SavedPlayer) {
    save_json(path, &player).await;
}

async fn read_json<T: serde::de::DeserializeOwned>(path: Option<PathBuf>, what: &str) -> Option<T> {
    let path = path?;
    let src = match smol::fs::read_to_string(&path).await {
        Ok(src) => src,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::warn!("could not read the {what} at {path:?}: {e}");
            return None;
        }
    };
    match serde_json::from_str::<T>(&src) {
        Ok(value) => Some(value),
        Err(e) => {
            log::warn!("discarding unreadable {what} at {path:?}: {e}");
            None
        }
    }
}

/// Best-effort atomic write, mirroring the tag cache's idiom; failure only costs this state on
/// the next launch.
async fn save_json<T: Serialize>(path: Option<PathBuf>, value: &T) {
    let Some(path) = path else { return };
    let write = async {
        if let Some(dir) = path.parent() {
            smol::fs::create_dir_all(dir).await?;
        }
        let json = serde_json::to_string(value).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.partial");
        smol::fs::write(&tmp, json).await?;
        smol::fs::rename(&tmp, &path).await
    };
    if let Err(e) = write.await {
        log::warn!("could not write {path:?}: {e}");
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Save and load round-trip, with a track whose file vanished between runs: it is dropped, and
    /// `current` follows its item to the new index.
    #[test]
    fn roundtrip_drops_vanished_tracks_and_keeps_current() {
        let root = std::env::temp_dir().join(format!("phonoscule-playlist-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let (playlist, player) = (Some(root.join("playlist.json")), Some(root.join("player.json")));

        smol::block_on(async {
            // Two real files around one that never exists.
            let (a, b) = (root.join("a.opus"), root.join("b.opus"));
            smol::fs::write(&a, b"x").await.unwrap();
            smol::fs::write(&b, b"x").await.unwrap();

            let sort = SortOrder { field: crate::sort::SortField::Year, ..Default::default() };
            save_playlist(playlist.clone(), SavedPlaylist::new(vec![a.clone(), root.join("gone.opus"), b.clone()])).await;
            save_player(player.clone(), SavedPlayer::new(2, Repeat::Album, sort)).await;
            let restored = load(playlist.clone(), player.clone()).await;

            assert_eq!(restored.tracks, [a, b], "the vanished track is dropped");
            assert_eq!(restored.current, 1, "current follows its item past the dropped one");
            assert_eq!(restored.repeat, Repeat::Album, "the repeat mode round-trips");
            assert_eq!(restored.sort, sort, "the sort order round-trips");
        });

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_files_restore_an_empty_session() {
        let missing = std::env::temp_dir().join("phonoscule-playlist-nonexistent");
        let restored = smol::block_on(load(Some(missing.join("playlist.json")), Some(missing.join("player.json"))));
        assert!(restored.tracks.is_empty());
        assert_eq!(restored.current, 0);
        assert_eq!(restored.repeat, Repeat::Off);
    }
}
