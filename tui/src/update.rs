//! The messages, and how each of them changes the model.

use crate::model::{Model, ScanState, View, refresh};
use crate::{covers, keys, logger, paths};
use phonoscule::library::{self, Album};

/// Everything the event loop reacts to, whatever source it came from.
#[derive(Debug)]
pub enum Msg {
    /// A key press, which the keys module resolves to whatever it is bound to in the current view.
    Key(crossterm::event::KeyEvent),
    /// The terminal was resized: nothing to change, but the frame must be redrawn.
    Resize,
    Library(library::ScanEvent),
    Log(logger::Entry),
    Show(View),
    /// Move the browser's selection by this many rows, clamped.
    Select(isize),
    /// Move it to the first or last album.
    SelectEdge(Edge),
    Quit,
}

#[derive(Debug, Clone, Copy)]
pub enum Edge {
    First,
    Last,
}

/// What the loop must do once a message has been applied.
#[must_use]
pub enum After {
    /// Redraw the frame.
    Redraw,
    /// Nothing on screen changed.
    Idle,
    /// Save the album index, which has drifted from what is on disk.
    SaveIndex,
}

pub fn update(model: &mut Model, msg: Msg) -> After {
    match msg {
        Msg::Key(key) => match keys::key_to_msg(model.view, key) {
            Some(msg) => update(model, msg),
            None => After::Idle,
        },
        Msg::Resize => After::Redraw,
        Msg::Log(entry) => {
            model.push_log(entry);
            After::Redraw
        }
        Msg::Quit => {
            model.quit = true;
            After::Idle
        }
        Msg::Show(view) => {
            model.view = view;
            After::Redraw
        }
        Msg::Select(delta) => {
            let last = model.shown.len().saturating_sub(1);
            model.selected = model.selected.saturating_add_signed(delta).min(last);
            sync_cover(model);
            After::Redraw
        }
        Msg::SelectEdge(edge) => {
            model.selected = match edge {
                Edge::First => 0,
                Edge::Last => model.shown.len().saturating_sub(1),
            };
            sync_cover(model);
            After::Redraw
        }
        Msg::Library(library::ScanEvent::Album(album)) => absorb_album(model, *album),
        Msg::Library(library::ScanEvent::Cover { albums, art }) => {
            // Only albums whose current cover choice this art satisfies take it: an album can
            // outgrow a queued cover mid-scan, and the stale decode must not overwrite the winner.
            let mut applied = false;
            for album in model.albums.iter_mut().filter(|a| albums.contains(&a.id) && a.cover_id == Some(art.id)) {
                album.cover = Some(art.clone());
                model.index_dirty |= album.accent != Some(art.accent);
                album.accent = Some(art.accent);
                applied = true;
            }
            if applied {
                sync_cover(model);
                After::Redraw
            } else {
                After::Idle
            }
        }
        Msg::Library(library::ScanEvent::Done { album_ids }) => {
            let ids: std::collections::HashSet<u64> = album_ids.into_iter().collect();
            let before = model.albums.len();
            model.albums.retain(|album| ids.contains(&album.id));
            model.index_dirty |= model.albums.len() != before;
            model.scan = ScanState::Complete;
            refresh(model);
            sync_cover(model);
            if std::mem::take(&mut model.index_dirty) { After::SaveIndex } else { After::Redraw }
        }
    }
}

/// Upserts a scanned album by id. Rescans re-report every album, the overwhelming majority
/// unchanged, so an unchanged one is dropped before doing any of the work below.
fn absorb_album(model: &mut Model, mut album: Album) -> After {
    match model.albums.iter().position(|a| a.id == album.id) {
        Some(ix) => {
            // `cover` and `accent` are runtime-only: scan events never carry them.
            let Album { id: _, title, artist, genre, year, cover_id, cover: _, accent: _, tracks } = &model.albums[ix];
            if *title == album.title
                && *artist == album.artist
                && *genre == album.genre
                && *year == album.year
                && *cover_id == album.cover_id
                && *tracks == album.tracks
            {
                return After::Idle;
            }
            model.index_dirty = true;
            let old = model.albums.remove(ix);
            // Keep the loaded cover art when the cover is unchanged (the scanner skips re-sending
            // it); the accent follows the cover.
            if old.cover_id == album.cover_id {
                album.cover = old.cover;
                album.accent = old.accent;
            }
        }
        None => model.index_dirty = true,
    }
    let key = |a: &Album| (a.artist.to_lowercase(), a.title.to_lowercase());
    let ix = model.albums.partition_point(|a| key(a) <= key(&album));
    model.albums.insert(ix, album);
    refresh(model);
    sync_cover(model);
    After::Redraw
}

/// Points the one held cover at the selected album, building it if that album's art has arrived and
/// dropping it if the selection has none. Cheap and idempotent, so callers fire it after anything
/// that could have moved the selection or delivered art.
fn sync_cover(model: &mut Model) {
    let art = model.selected_album().and_then(|album| album.cover.as_ref().map(|art| (album.id, art.clone())));
    match art {
        // Already the right one: leave it be, or its resize and encode would be thrown away.
        Some((id, _)) if model.cover.as_ref().is_some_and(|cover| cover.album == id) => (),
        Some((id, art)) => model.cover = covers::build(&model.picker, id, &art),
        None => model.cover = None,
    }
}

/// Options for the boot scan.
pub fn scan_options(model: &Model) -> library::ScanOptions {
    library::ScanOptions {
        root: model.conf.music_dir.clone(),
        priority: vec![],
        known_covers: model.albums.iter().filter_map(|a| a.cover.as_ref().map(|c| c.id)).collect(),
        cache_file: paths::tag_cache_file(),
        covers_dir: paths::covers_dir(),
    }
}
