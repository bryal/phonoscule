//! The messages, and how each of them changes the model.

use crate::model::{Model, QueueItem, ScanState, View, refresh};
use crate::{covers, keys, logger, paths};
use phonoscule::library::{self, Album};
use phonoscule::player;
use std::time::Duration;

/// Everything the event loop reacts to, whatever source it came from.
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
    Player(player::Event),
    /// A cover finished loading and encoding (see the covers module).
    Cover(covers::Load),
    /// Play the selected album, replacing the queue.
    PlaySelected,
    /// Append the selected album to the queue.
    QueueSelected,
    Toggle,
    Next,
    Prev,
    /// Seek by this much, forwards or back, within the playing track.
    Seek(i64),
    /// Cycle the repeat mode.
    CycleRepeat,
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
        Msg::Resize => {
            // Every cover was encoded for the area it was drawn in, and none of those areas survive
            // a resize.
            model.covers.clear();
            After::Redraw
        }
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
            let row = model.selected_row().saturating_add_signed(delta);
            model.select_row(row);
            After::Redraw
        }
        Msg::SelectEdge(edge) => {
            let row = match edge {
                Edge::First => 0,
                Edge::Last => model.shown.len().saturating_sub(1),
            };
            model.select_row(row);
            After::Redraw
        }
        Msg::PlaySelected => match album_items(model) {
            Some(items) => {
                model.send(player::Cmd::SetQueue { tracks: entries(&items), start: 0, play: player::PlayState::Playing });
                model.queue = items;
                model.current = 0;
                model.view = View::Player;
                After::Redraw
            }
            None => After::Idle,
        },
        Msg::QueueSelected => match album_items(model) {
            Some(items) => {
                model.send(player::Cmd::Append { tracks: entries(&items) });
                model.queue.extend(items);
                After::Redraw
            }
            None => After::Idle,
        },
        Msg::Toggle => {
            model.send(player::Cmd::TogglePlayPause);
            After::Idle
        }
        Msg::Next => {
            model.send(player::Cmd::Next);
            After::Idle
        }
        Msg::Prev => {
            model.send(player::Cmd::Prev);
            After::Idle
        }
        Msg::Seek(by) => {
            seek(model, by);
            After::Redraw
        }
        Msg::CycleRepeat => {
            model.repeat = model.repeat.cycled();
            model.send(player::Cmd::SetRepeat(model.repeat));
            After::Redraw
        }
        Msg::Cover(load) => {
            model.covers.absorb(load);
            After::Redraw
        }
        Msg::Player(event) => player_event(model, event),
        Msg::Library(library::ScanEvent::Album(album)) => absorb_album(model, *album),
        Msg::Library(library::ScanEvent::Cover { albums, art }) => {
            // Only albums whose current cover choice this art satisfies take it: an album can
            // outgrow a queued cover mid-scan, and the stale decode must not overwrite the winner.
            // The pixels are not kept: a library's worth of them is hundreds of megabytes, and they
            // are on disk in the thumbnail cache, to be read back a few at a time as covers are
            // shown. What is worth keeping is the accent colour, which stands in for artwork that
            // has not been loaded yet and costs twelve bytes.
            model.covers.learn_file(art.id, art.file.clone());
            let mut applied = false;
            for album in model.albums.iter_mut().filter(|a| albums.contains(&a.id) && a.cover_id == Some(art.id)) {
                model.index_dirty |= album.accent != Some(art.accent);
                album.accent = Some(art.accent);
                applied = true;
            }
            if applied { After::Redraw } else { After::Idle }
        }
        Msg::Library(library::ScanEvent::Done { album_ids }) => {
            let ids: std::collections::HashSet<u64> = album_ids.into_iter().collect();
            let before = model.albums.len();
            model.albums.retain(|album| ids.contains(&album.id));
            model.index_dirty |= model.albums.len() != before;
            model.scan = ScanState::Complete;
            model.shown_dirty = true;
            if std::mem::take(&mut model.index_dirty) { After::SaveIndex } else { After::Redraw }
        }
    }
}

/// How close a reported position must be to a pending seek's target to count as arrived, after
/// which live reports drive the bar again. Wide enough that the first report once playback catches up
/// clears it, narrow relative to a seek step so it does not clear mid-scrub while a key is held.
const SEEK_SETTLE: Duration = Duration::from_secs(1);

/// Seeks `by` seconds, forwards or back. Taken relative to where the bar already is -- which this
/// moves at once -- so a burst accumulates instead of every press starting from the same lagged
/// position. Saturates at zero and clamps to the track's length.
fn seek(model: &mut Model, by: i64) {
    let target = match by.is_negative() {
        true => model.pos.saturating_sub(Duration::from_secs(by.unsigned_abs())),
        false => model.pos.saturating_add(Duration::from_secs(by.unsigned_abs())),
    };
    let target = model.len.map_or(target, |len| target.min(len));
    model.pos = target;
    model.pending_seek = Some(target);
    model.send(player::Cmd::Seek(target));
}

fn player_event(model: &mut Model, event: player::Event) -> After {
    match event {
        player::Event::TrackStarted { ix, len } => {
            model.current = ix;
            model.len = len;
            model.pos = Duration::ZERO;
            // A new track invalidates any seek still settling against the old one.
            model.pending_seek = None;
            After::Redraw
        }
        player::Event::Progress(pos) => {
            // While a seek settles, ignore reports until playback reaches (roughly) where it was
            // asked to go: earlier ones, still in flight, would drag the bar back to where playback
            // was before the seek. Slow decoding makes that window wide, which is why holding a seek
            // key in a debug build made the bar rubberband.
            if matches!(model.pending_seek, Some(target) if pos.abs_diff(target) > SEEK_SETTLE) {
                return After::Idle;
            }
            model.pending_seek = None;
            model.pos = pos;
            After::Redraw
        }
        player::Event::PlayState(state) => {
            model.play_state = state;
            After::Redraw
        }
        player::Event::QueueEnded => {
            model.play_state = player::PlayState::Paused;
            // The queue may have ended through a skip: rest the bar at the end rather than wherever
            // the last track happened to have reached.
            model.pos = model.len.unwrap_or(Duration::ZERO);
            model.pending_seek = None;
            After::Redraw
        }
    }
}

/// The selected album's tracks as queue items, or `None` if nothing is selected.
fn album_items(model: &Model) -> Option<Vec<QueueItem>> {
    let album = model.selected_album()?;
    let items = album
        .tracks
        .iter()
        .map(|track| QueueItem { path: track.path.clone(), album_id: album.id, title: track.title.clone() })
        .collect();
    Some(items)
}

/// The engine-facing form of queue items: the path, and the album key that repeat-album walks.
fn entries(items: &[QueueItem]) -> Vec<player::Entry> {
    items.iter().map(|item| player::Entry { path: item.path.clone(), album: item.album_id }).collect()
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
    // Not re-sorted here: `reconcile` does that once for the whole burst.
    model.shown_dirty = true;
    After::Redraw
}

/// Brings the derived state back in line with the albums and the selection, once per burst of
/// messages rather than once per message (see [`Model::shown_dirty`]).
pub fn reconcile(model: &mut Model) {
    if model.shown_dirty {
        refresh(model);
    }
    pin_covers(model);
}

/// Names the covers that must stay cached: around the browser's cursor for thumbnails, and around
/// the playing album in the queue for the high-resolution ones. Loading them is the view's business,
/// which is where the size they must be encoded for is known.
fn pin_covers(model: &mut Model) {
    let row = model.selected_row();
    let first = row.saturating_sub(covers::PIN_RADIUS);
    let thumbs: Vec<u64> = (first..=row + covers::PIN_RADIUS).filter_map(|row| model.album_at(row)?.cover_id).collect();
    model.covers.pin(thumbs, full_window(model));
}

/// The albums whose high-resolution covers are worth having ready: the playing one, and its
/// neighbours in the queue.
pub fn full_window(model: &Model) -> Vec<u64> {
    let albums = model.queue_albums();
    let Some(playing) = model.playing().map(|item| item.album_id) else { return vec![] };
    let Some(at) = albums.iter().position(|&id| id == playing) else { return vec![] };
    let first = at.saturating_sub(covers::FULL_BEHIND);
    albums[first..(at + covers::FULL_AHEAD + 1).min(albums.len())]
        .iter()
        .filter_map(|&id| model.albums.iter().find(|album| album.id == id)?.cover_id)
        .collect()
}

/// Options for the boot scan.
pub fn scan_options(model: &Model) -> library::ScanOptions {
    library::ScanOptions {
        root: model.conf.music_dir.clone(),
        priority: vec![],
        // Nothing is claimed as known: the scan reads each thumbnail back from its own cache in a
        // few tens of microseconds, and its accent is worth having even when the pixels are not.
        known_covers: Default::default(),
        cache_file: paths::tag_cache_file(),
        covers_dir: paths::covers_dir(),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::model::browser;

    fn send(model: &mut Model, msg: Msg) {
        let _ = update(model, msg);
    }

    /// Playing the selection replaces the queue with that album's tracks and shows the player.
    #[test]
    fn playing_an_album_fills_the_queue_and_shows_the_player() {
        let mut model = browser(5);
        send(&mut model, Msg::Select(2));
        let album = model.selected_album().expect("an album is selected").clone();

        send(&mut model, Msg::PlaySelected);
        assert_eq!(model.view, View::Player);
        assert_eq!(model.queue.len(), album.tracks.len());
        assert!(model.queue.iter().all(|item| item.album_id == album.id), "every item is from the played album");
        assert_eq!(model.current, 0, "playback starts at the album's first track");
    }

    /// Queueing appends and leaves the browser where it is, so several albums can be lined up.
    #[test]
    fn queueing_albums_appends_without_leaving_the_browser() {
        let mut model = browser(5);
        send(&mut model, Msg::PlaySelected);
        let first = model.queue.len();

        send(&mut model, Msg::Show(View::Library));
        send(&mut model, Msg::Select(1));
        send(&mut model, Msg::QueueSelected);
        assert_eq!(model.view, View::Library, "queueing does not switch views");
        assert!(model.queue.len() > first, "the second album was appended");
        let albums: std::collections::HashSet<u64> = model.queue.iter().map(|item| item.album_id).collect();
        assert_eq!(albums.len(), 2, "the queue holds both albums");
    }

    /// The engine says which track is playing and how long it is; the model follows it.
    #[test]
    fn player_events_drive_the_now_playing_state() {
        let mut model = browser(3);
        send(&mut model, Msg::PlaySelected);

        let len = Some(Duration::from_secs(200));
        send(&mut model, Msg::Player(player::Event::TrackStarted { ix: 1, len }));
        assert_eq!(model.current, 1);
        assert_eq!(model.len, len);
        assert_eq!(model.pos, Duration::ZERO, "a fresh track starts at the beginning");

        send(&mut model, Msg::Player(player::Event::Progress(Duration::from_secs(12))));
        assert_eq!(model.pos, Duration::from_secs(12));

        send(&mut model, Msg::Player(player::Event::PlayState(player::PlayState::Paused)));
        assert_eq!(model.play_state, player::PlayState::Paused);
    }

    /// Seeking accumulates from where the bar is, so a held key scrubs instead of every press
    /// starting over from the last position the engine reported.
    #[test]
    fn seeks_accumulate_and_stop_at_the_start() {
        let mut model = browser(3);
        send(&mut model, Msg::Player(player::Event::Progress(Duration::from_secs(30))));
        send(&mut model, Msg::Seek(5));
        send(&mut model, Msg::Seek(5));
        assert_eq!(model.pos, Duration::from_secs(40));

        send(&mut model, Msg::Seek(-600));
        assert_eq!(model.pos, Duration::ZERO, "seeking back past the start stops there");
    }

    /// Seeking forward stops at the end of the track rather than running past it.
    #[test]
    fn seeks_clamp_to_the_track_length() {
        let mut model = browser(3);
        send(&mut model, Msg::Player(player::Event::TrackStarted { ix: 0, len: Some(Duration::from_secs(100)) }));
        send(&mut model, Msg::Seek(600));
        assert_eq!(model.pos, Duration::from_secs(100));
    }

    /// The bar must not rubberband: reports the engine sent before a seek are still on their way
    /// afterwards, and applying them would drag the bar back to where playback was.
    #[test]
    fn stale_progress_reports_do_not_drag_the_bar_back() {
        let mut model = browser(3);
        send(&mut model, Msg::Player(player::Event::TrackStarted { ix: 0, len: Some(Duration::from_secs(300)) }));
        send(&mut model, Msg::Player(player::Event::Progress(Duration::from_secs(10))));

        // Scrub forward a minute, as a held key does.
        for _ in 0..12 {
            send(&mut model, Msg::Seek(5));
        }
        assert_eq!(model.pos, Duration::from_secs(70), "the bar follows the keys immediately");

        // Reports from before the seek, arriving late because decoding lagged behind.
        for stale in [11, 12, 13] {
            send(&mut model, Msg::Player(player::Event::Progress(Duration::from_secs(stale))));
            assert_eq!(model.pos, Duration::from_secs(70), "a stale report must not move the bar");
        }

        // Once playback actually arrives, reports drive the bar again.
        send(&mut model, Msg::Player(player::Event::Progress(Duration::from_secs(70))));
        assert_eq!(model.pos, Duration::from_secs(70));
        send(&mut model, Msg::Player(player::Event::Progress(Duration::from_secs(71))));
        assert_eq!(model.pos, Duration::from_secs(71), "live reports drive the bar once the seek settled");
    }

    /// A track change abandons a seek that never settled, so the new track's reports are believed.
    #[test]
    fn a_new_track_abandons_an_unsettled_seek() {
        let mut model = browser(3);
        send(&mut model, Msg::Player(player::Event::TrackStarted { ix: 0, len: Some(Duration::from_secs(300)) }));
        send(&mut model, Msg::Seek(120));
        send(&mut model, Msg::Player(player::Event::TrackStarted { ix: 1, len: Some(Duration::from_secs(300)) }));
        assert_eq!(model.pos, Duration::ZERO);

        send(&mut model, Msg::Player(player::Event::Progress(Duration::from_secs(2))));
        assert_eq!(model.pos, Duration::from_secs(2), "the new track's reports are not held off");
    }
}
