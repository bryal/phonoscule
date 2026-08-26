//! Where this player keeps its files.
//!
//! Its own directories, not shared ones: the framework takes paths rather than deciding them, so
//! another phonoscule player on the same machine has its own state and caches and cannot overwrite
//! ours. (Pointing two of them at one directory is then a deliberate act, not the default.) Which
//! roots those directories sit under is the platform's business, and
//! [`dirs`](phonoscule::dirs)' - all we bring is the name.

use phonoscule::{dirs, library};

/// The square edge covers are cached at: what an album card in the grid wants, and no more -- every
/// one of them is held in memory at once, decoded, so a card costs `edge`² x 4 bytes for as long as
/// the library has it.
///
/// A card's cover square is 156 to 172 pixels wide (see the album grid's metrics), so this is a
/// little headroom over the largest of them and nothing more. It is on disk in the path, so changing
/// it starts a fresh cache rather than reinterpreting the old one.
pub const THUMB_EDGE: u32 = 220;
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

/// The cover thumbnails, one per album and all of them held decoded.
pub fn covers_dir() -> Option<PathBuf> {
    Some(library::covers_dir(&cache_dir()?, THUMB_EDGE))
}

/// The full-size covers the player view draws, so that showing one is a read and a small decode
/// rather than decoding a whole sleeve. Only ever holds the covers that have actually been looked
/// at, which is why it can afford a resolution the thumbnails cannot.
pub fn full_covers_dir() -> Option<PathBuf> {
    Some(library::covers_dir(&cache_dir()?, library::FULL))
}
