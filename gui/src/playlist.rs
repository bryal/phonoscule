//! Persistence of the play queue across runs: the tracks (with the metadata needed to render them
//! before the library scan comes back) and which one was current. Saved best-effort on every
//! change and loaded once at boot; the session restores paused at the start of the current track.
//!
//! Lives under the XDG *state* directory, not the cache: unlike the tag or cover caches, a queue
//! can't be regenerated, so it must survive a cache wipe.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Bumped when [`SavedPlaylist`] changes shape; an old or unreadable file restores an empty queue.
const VERSION: u32 = 1;

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct SavedPlaylist {
    version: u32,
    pub items: Vec<SavedItem>,
    pub current: usize,
}

impl SavedPlaylist {
    pub fn new(items: Vec<SavedItem>, current: usize) -> Self {
        SavedPlaylist { version: VERSION, items, current }
    }
}

/// A queue entry: the track and the tags the player view shows. Everything but the cover art,
/// which the library scan re-attaches by album id shortly after boot.
#[derive(Clone, Serialize, Deserialize)]
pub struct SavedItem {
    pub path: PathBuf,
    pub album_id: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
}

/// Where the playlist is saved: `$XDG_STATE_HOME/phonoscule/playlist.json`, falling back to
/// `~/.local/state/phonoscule/playlist.json`.
pub fn default_file() -> Option<PathBuf> {
    let state = std::env::var("XDG_STATE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(std::env::home_dir()?.join(".local/state")))?;
    Some(state.join("phonoscule/playlist.json"))
}

/// Loads the saved playlist, dropping tracks whose files have vanished since the last run (the
/// music directory may have changed in between) while keeping `current` pointed at the same item.
/// A missing, outdated, or unreadable file restores an empty queue.
pub async fn load(path: Option<PathBuf>) -> SavedPlaylist {
    let Some(path) = path else { return SavedPlaylist::default() };
    let src = match smol::fs::read_to_string(&path).await {
        Ok(src) => src,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return SavedPlaylist::default(),
        Err(e) => {
            log::warn!("could not read the playlist at {path:?}: {e}");
            return SavedPlaylist::default();
        }
    };
    let saved = match serde_json::from_str::<SavedPlaylist>(&src) {
        Ok(saved) if saved.version == VERSION => saved,
        Ok(_) => return SavedPlaylist::default(),
        Err(e) => {
            log::warn!("discarding unreadable playlist at {path:?}: {e}");
            return SavedPlaylist::default();
        }
    };

    let mut items = Vec::with_capacity(saved.items.len());
    let mut current = saved.current;
    for (ix, item) in saved.items.into_iter().enumerate() {
        if smol::fs::metadata(&item.path).await.is_ok() {
            items.push(item);
        } else if ix < saved.current {
            // A dropped track before the current one shifts it back one slot.
            current -= 1;
        }
    }
    let current = current.min(items.len().saturating_sub(1));
    SavedPlaylist { version: VERSION, items, current }
}

/// Best-effort atomic write, mirroring the tag cache's idiom; failure only costs this queue on
/// the next launch.
pub async fn save(path: Option<PathBuf>, playlist: SavedPlaylist) {
    let Some(path) = path else { return };
    let write = async {
        if let Some(dir) = path.parent() {
            smol::fs::create_dir_all(dir).await?;
        }
        let json = serde_json::to_string(&playlist).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.partial");
        smol::fs::write(&tmp, json).await?;
        smol::fs::rename(&tmp, &path).await
    };
    if let Err(e) = write.await {
        log::warn!("could not write the playlist to {path:?}: {e}");
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn item(path: PathBuf, title: &str) -> SavedItem {
        SavedItem { path, album_id: 1, title: title.into(), artist: "a".into(), album: "b".into() }
    }

    /// Save and load round-trip, with a track whose file vanished between runs: it is dropped, and
    /// `current` follows its item to the new index.
    #[test]
    fn roundtrip_drops_vanished_tracks_and_keeps_current() {
        let root = std::env::temp_dir().join(format!("phonoscule-playlist-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = Some(root.join("playlist.json"));

        smol::block_on(async {
            // Two real files around one that never exists.
            let (a, b) = (root.join("a.opus"), root.join("b.opus"));
            smol::fs::write(&a, b"x").await.unwrap();
            smol::fs::write(&b, b"x").await.unwrap();
            let items = vec![item(a.clone(), "a"), item(root.join("gone.opus"), "gone"), item(b.clone(), "b")];

            save(file.clone(), SavedPlaylist::new(items, 2)).await;
            let loaded = load(file.clone()).await;

            let titles: Vec<&str> = loaded.items.iter().map(|i| i.title.as_str()).collect();
            assert_eq!(titles, ["a", "b"], "the vanished track is dropped");
            assert_eq!(loaded.current, 1, "current follows its item past the dropped one");
        });

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_file_restores_an_empty_queue() {
        let loaded = smol::block_on(load(Some(std::env::temp_dir().join("phonoscule-playlist-nonexistent.json"))));
        assert!(loaded.items.is_empty());
        assert_eq!(loaded.current, 0);
    }
}
