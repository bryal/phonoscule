//! Where this player keeps its files.
//!
//! Its own directories, not shared ones: the framework takes paths rather than deciding them, so
//! another phonoscule player on the same machine has its own state and caches and cannot overwrite
//! ours. (Pointing two of them at one directory is then a deliberate act, not the default.) Which
//! roots those directories sit under is the platform's business, and
//! [`dirs`](phonoscule::dirs)' - all we bring is the name.

use phonoscule::{dirs, library};
use std::path::PathBuf;

/// The name our directories go by under the platform's roots.
const DIR: &str = "phonoscule";

/// State survives a cache wipe. `None` when there is no such directory to be found, which means we
/// don't persist.
fn state_dir() -> Option<PathBuf> {
    dirs::state_dir(DIR)
}

/// Regenerable caches: the tag cache, the album index, and the cover thumbnails.
fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir(DIR)
}

/// The play queue: just its list of tracks.
pub fn playlist_file() -> Option<PathBuf> {
    Some(state_dir()?.join("playlist.json"))
}

/// The session state around the queue: current track, repeat mode, sort order.
pub fn player_file() -> Option<PathBuf> {
    Some(state_dir()?.join("player.json"))
}

/// The tag cache, so a rescan only opens files that changed.
pub fn tag_cache_file() -> Option<PathBuf> {
    Some(cache_dir()?.join("library.json"))
}

/// The album index, so a launch shows the whole library before the scan finishes.
pub fn album_index_file() -> Option<PathBuf> {
    Some(cache_dir()?.join("albums.json"))
}

/// The decoded cover thumbnails.
pub fn covers_dir() -> Option<PathBuf> {
    Some(library::covers_dir(&cache_dir()?))
}
