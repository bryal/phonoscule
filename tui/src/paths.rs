//! Where this player keeps its files: its own directories, not shared with any other phonoscule
//! player. Point two of them at one directory deliberately if you want that. Which roots they sit
//! under is [`dirs`](phonoscule::dirs)' business; all we bring is the name.

use phonoscule::{dirs, library};
use std::path::PathBuf;

/// The name our directories go by under the platform's roots.
const DIR: &str = "phonoscule-tui";

/// State survives a cache wipe. `None` when there is no such directory to be found, which means the
/// session is simply not kept.
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

/// The state around the queue: current track, repeat mode, sort order.
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
