//! The messages, and how each of them changes the model.

use crate::model::{Focus, Model, QueueItem, ScanState, Subject, TrackMenu, View, open_picker, refresh};
use crate::{covers, keys, logger, paths};
use phonoscule::library::{self, Album};
use phonoscule::media;
use phonoscule::player;
use phonoscule::queue::{self, Grouping, Scope};
use phonoscule::session;
use phonoscule::sort::SortOrder;
use std::future::Future;
use std::pin::Pin;
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
    /// The OS mixer reported the application's volume.
    VolumeChanged(f32),
    /// Step the volume by this much, 1.0 being the whole range.
    BumpVolume(f32),
    /// A control request from the OS: a media key, `playerctl`, a desktop widget.
    Media(media::Control),
    /// Time to look over the music directory again.
    Rescan,
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
    /// Start typing an album search, beginning with this character if typing is what opened it.
    Search(Option<char>),
    /// A character typed into whatever has focus: the album search, or a picker's own search.
    Typed(char),
    /// Rub out the last character typed.
    Rubout,
    /// Open a picker over this subject.
    OpenPicker(Subject),
    /// Move a picker's selection by this many rows, clamped.
    PickerMove(isize),
    /// Open the selected album's tracks, to play or queue them one at a time.
    OpenTracks,
    /// Move the track menu's selection by this many rows, clamped.
    TrackMove(isize),
    /// Play the track the menu's selection sits on, replacing the queue.
    PlayTrack,
    /// Append it to the queue, stepping onto the next so successive presses queue a run.
    QueueTrack,
    /// Take what the picker's selection sits on: a filter value, or an order.
    Pick,
    /// Leave whatever has focus, keeping what it has narrowed to.
    Done,
    /// Clear every filter, and leave the search or picker that was setting one.
    ClearFilters,
    /// Play, or queue, every album the filter lets through, in the order shown.
    PlayShown,
    QueueShown,
    /// Shuffle the queue, in place: see [`Grouping`] for what moves as a unit and [`Scope`] for how
    /// much of the queue is touched.
    Shuffle {
        grouping: Grouping,
        scope: Scope,
    },
    /// Empty the queue: playback stops and the player has nothing to show.
    ClearQueue,
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
    /// Look over the music directory again.
    Rescan,
}

pub fn update(model: &mut Model, msg: Msg) -> After {
    match msg {
        Msg::Key(key) => match keys::key_to_msg(model.view, &model.focus, key) {
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
                model.focus = Focus::Albums;
                model.dirty_playlist = true;
                model.dirty_player = true;
                After::Redraw
            }
            None => After::Idle,
        },
        Msg::QueueSelected => match album_items(model) {
            Some(items) => {
                model.send(player::Cmd::Append { tracks: entries(&items) });
                model.queue.extend(items);
                model.dirty_playlist = true;
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
            model.dirty_player = true;
            After::Redraw
        }
        Msg::Cover(load) => {
            model.covers.absorb(load);
            After::Redraw
        }
        Msg::Search(first) => {
            model.focus = Focus::Search;
            if let Some(c) = first {
                model.filter.search.push(c);
                model.shown_dirty = true;
            }
            After::Redraw
        }
        Msg::Typed(c) => match &mut model.focus {
            Focus::Search => {
                model.filter.search.push(c);
                model.shown_dirty = true;
                After::Redraw
            }
            Focus::Picker(picker) => {
                picker.query.push(c);
                requery(model);
                After::Redraw
            }
            // Typing is for searching, and the track menu has nothing to search.
            Focus::Albums | Focus::Tracks(_) => After::Idle,
        },
        Msg::Rubout => match &mut model.focus {
            Focus::Search => {
                model.filter.search.pop();
                model.shown_dirty = true;
                After::Redraw
            }
            Focus::Picker(picker) => {
                picker.query.pop();
                requery(model);
                After::Redraw
            }
            Focus::Tracks(_) => After::Idle,
            // Rubbing out is editing the search, so it takes the keys there as typing does -- for
            // correcting a search after the keys have gone back to the list.
            Focus::Albums => {
                model.focus = Focus::Search;
                model.filter.search.pop();
                model.shown_dirty = true;
                After::Redraw
            }
        },
        Msg::OpenPicker(subject) => {
            model.focus = Focus::Picker(open_picker(model, subject));
            After::Redraw
        }
        Msg::PickerMove(delta) => {
            let Focus::Picker(picker) = &mut model.focus else { return After::Idle };
            let last = picker.rows().saturating_sub(1);
            picker.selected = picker.selected.saturating_add_signed(delta).min(last);
            After::Redraw
        }
        Msg::OpenTracks => match model.selected_album() {
            Some(album) => {
                model.focus = Focus::Tracks(TrackMenu { album: album.id, selected: 0 });
                model.menu_list = Default::default();
                After::Redraw
            }
            None => After::Idle,
        },
        Msg::TrackMove(delta) => {
            let Some(menu) = model.track_menu() else { return After::Idle };
            let last = model.acting_album().map_or(0, |album| album.tracks.len().saturating_sub(1));
            let selected = menu.selected.saturating_add_signed(delta).min(last);
            model.focus = Focus::Tracks(TrackMenu { selected, ..menu });
            After::Redraw
        }
        Msg::PlayTrack => match menu_track(model) {
            Some(item) => {
                model.send(player::Cmd::SetQueue {
                    tracks: entries(std::slice::from_ref(&item)),
                    start: 0,
                    play: player::PlayState::Playing,
                });
                model.queue = vec![item];
                model.current = 0;
                model.focus = Focus::Albums;
                model.view = View::Player;
                model.dirty_playlist = true;
                model.dirty_player = true;
                After::Redraw
            }
            None => After::Idle,
        },
        Msg::QueueTrack => match menu_track(model) {
            Some(item) => {
                model.send(player::Cmd::Append { tracks: entries(std::slice::from_ref(&item)) });
                model.queue.push(item);
                model.dirty_playlist = true;
                // Onto the next, so holding it down queues an album a track at a time.
                update(model, Msg::TrackMove(1))
            }
            None => After::Idle,
        },
        Msg::Pick => pick(model),
        Msg::Done => {
            model.focus = Focus::Albums;
            After::Redraw
        }
        Msg::ClearFilters => {
            model.filter = Default::default();
            model.focus = Focus::Albums;
            model.shown_dirty = true;
            After::Redraw
        }
        Msg::PlayShown => match shown_items(model) {
            items if items.is_empty() => After::Idle,
            items => {
                model.send(player::Cmd::SetQueue { tracks: entries(&items), start: 0, play: player::PlayState::Playing });
                model.queue = items;
                model.current = 0;
                model.view = View::Player;
                model.dirty_playlist = true;
                model.dirty_player = true;
                After::Redraw
            }
        },
        Msg::QueueShown => match shown_items(model) {
            items if items.is_empty() => After::Idle,
            items => {
                model.send(player::Cmd::Append { tracks: entries(&items) });
                model.queue.extend(items);
                model.dirty_playlist = true;
                After::Redraw
            }
        },
        Msg::Shuffle { grouping, scope } => shuffle(model, grouping, scope),
        Msg::ClearQueue => {
            model.queue.clear();
            model.current = 0;
            model.pos = Duration::ZERO;
            model.len = None;
            model.pending_seek = None;
            model.play_state = player::PlayState::Paused;
            model.send(player::Cmd::SetQueue { tracks: vec![], start: 0, play: player::PlayState::Paused });
            model.dirty_playlist = true;
            model.dirty_player = true;
            After::Redraw
        }
        Msg::BumpVolume(delta) => match model.volume {
            // Nothing to step from until the mixer has said where it is.
            None => After::Idle,
            Some(volume) => {
                let wanted = (volume + delta).clamp(0.0, 1.0);
                // Moved at once and remembered, so a burst of steps accumulates from here rather than
                // from the reading that is still catching up.
                model.volume = Some(wanted);
                model.pending_volume = Some(wanted);
                model.mixer.set(wanted);
                After::Redraw
            }
        },
        Msg::VolumeChanged(volume) => {
            // While a set of ours is in flight, ignore readings until the mixer reaches (about) what
            // was asked: earlier ones still on their way would drag the bar back. The tolerance is
            // generous against the mixer's own rounding and narrow against a real step.
            match model.pending_volume {
                Some(wanted) if (volume - wanted).abs() > 0.005 => After::Idle,
                _ => {
                    model.pending_volume = None;
                    model.volume = Some(volume);
                    After::Redraw
                }
            }
        }
        Msg::Media(control) => match control {
            media::Control::Play | media::Control::Pause | media::Control::Toggle | media::Control::Stop => {
                // Play, pause and stop all mean the same thing here, since the engine has one toggle
                // and the desktop only ever asks for the state we are not in.
                let wanted = match control {
                    media::Control::Play => player::PlayState::Playing,
                    _ => player::PlayState::Paused,
                };
                if control == media::Control::Toggle || wanted != model.play_state {
                    model.send(player::Cmd::TogglePlayPause);
                }
                After::Idle
            }
            media::Control::Next => update(model, Msg::Next),
            media::Control::Prev => update(model, Msg::Prev),
            media::Control::Seek(micros) => update(model, Msg::Seek(micros / 1_000_000)),
            media::Control::SetPosition(pos) => {
                model.pos = pos;
                model.pending_seek = Some(pos);
                model.send(player::Cmd::Seek(pos));
                After::Redraw
            }
        },
        Msg::Rescan => match model.scan {
            // The scan already running will pick up whatever changed.
            ScanState::Scanning => After::Idle,
            ScanState::Complete => {
                model.scan = ScanState::Scanning;
                After::Rescan
            }
        },
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
            model.dirty_player = true;
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

/// Shuffles the queue in place: the new order *is* the queue, not a shuffled way of playing it, so
/// what the player lists is what will play. Which slot leads and whether playback carries on depend
/// on the scope (see [`Scope`]).
fn shuffle(model: &mut Model, grouping: Grouping, scope: Scope) -> After {
    if model.queue.is_empty() {
        return After::Idle;
    }
    let albums: Vec<u64> = model.queue.iter().map(|item| item.album_id).collect();
    let order = queue::shuffle(&albums, model.current, grouping, scope, queue::seed());

    let mut old: Vec<Option<QueueItem>> = std::mem::take(&mut model.queue).into_iter().map(Some).collect();
    model.queue = order.iter().map(|&ix| old[ix].take().expect("a permutation visits each slot once")).collect();
    model.dirty_playlist = true;
    model.dirty_player = true;
    match scope {
        // Same tracks in a new order: only the cursor follows the playing one, and it keeps playing.
        Scope::Others => {
            model.current = order.iter().position(|&ix| ix == model.current).unwrap_or(0);
            model.send(player::Cmd::Reorder { tracks: entries(&model.queue), current: model.current });
        }
        // A fresh start, resting paused on whatever came out first.
        Scope::All => {
            model.current = 0;
            model.pos = Duration::ZERO;
            model.pending_seek = None;
            model.send(player::Cmd::SetQueue { tracks: entries(&model.queue), start: 0, play: player::PlayState::Paused });
        }
    }
    After::Redraw
}

/// Re-ranks an open picker's matches against its query, putting the selection back at the top.
fn requery(model: &mut Model) {
    let Focus::Picker(picker) = &model.focus else { return };
    let (subject, query) = (picker.subject, picker.query.clone());
    let matches = phonoscule::search::matches(crate::model::picker_options(model, subject), &query);
    let Focus::Picker(picker) = &mut model.focus else { return };
    picker.matches = matches;
    // Row 0 is the "any" entry where there is one, so the first match is the row below it.
    picker.selected = usize::from(picker.has_any_row());
}

/// Applies what an open picker's selection sits on, and closes it.
fn pick(model: &mut Model) -> After {
    let Focus::Picker(picker) = &model.focus else { return After::Idle };
    let picked = picker.picked().map(str::to_owned);
    match picker.subject {
        Subject::Genre => model.filter.genre = picked,
        Subject::Artist => model.filter.artist = picked,
        Subject::Sort => {
            let Some(&order) = SortOrder::ALL.get(picker.selected) else { return After::Idle };
            model.sort = order;
            model.dirty_player = true;
        }
    }
    model.focus = Focus::Albums;
    model.shown_dirty = true;
    After::Redraw
}

/// The queue item for the track the menu's selection sits on.
fn menu_track(model: &Model) -> Option<QueueItem> {
    let menu = model.track_menu()?;
    let album = model.acting_album()?;
    let track = album.tracks.get(menu.selected)?;
    Some(QueueItem { path: track.path.clone(), album_id: album.id, title: track.title.clone() })
}

/// Every shown album's tracks as queue items, in the order shown.
fn shown_items(model: &Model) -> Vec<QueueItem> {
    model
        .shown
        .iter()
        .filter_map(|&ix| model.albums.get(ix))
        .flat_map(|album| {
            album.tracks.iter().map(|track| QueueItem {
                path: track.path.clone(),
                album_id: album.id,
                title: track.title.clone(),
            })
        })
        .collect()
}

/// The selected album's tracks as queue items, or `None` if nothing is selected.
fn album_items(model: &Model) -> Option<Vec<QueueItem>> {
    let album = model.acting_album()?;
    let items = album
        .tracks
        .iter()
        .map(|track| QueueItem { path: track.path.clone(), album_id: album.id, title: track.title.clone() })
        .collect();
    Some(items)
}

/// The engine-facing form of queue items: the path, and the album key that repeat-album walks.
pub fn entries(items: &[QueueItem]) -> Vec<player::Entry> {
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
    // The queue is re-matched against the library once the burst has landed, not here.
    model.queue_stale = true;
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
    if std::mem::take(&mut model.queue_stale) {
        // Destructured, so the library and the queue are borrowed as the separate fields they are.
        let Model { albums, queue, .. } = model;
        crate::model::hydrate(albums, queue);
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

/// Tells the OS what is playing. Fire and forget: the media worker coalesces a burst of these down
/// to the latest, so this can be called after every round of messages without thought.
pub fn publish_media(model: &Model, media: &media::Media) {
    let meta = model.playing().map(|item| {
        let album = model.album_of(item);
        media::Meta {
            title: item.title.clone(),
            album: album.map(|a| a.title.clone()).unwrap_or_default(),
            artist: album.map(|a| a.artist.clone()).unwrap_or_default(),
            // Absolute, so the desktop can find it whatever our working directory is.
            cover_url: album
                .and_then(|album| album.cover_id)
                .and_then(|cover| model.covers.file_of(cover))
                .and_then(|file| url_of(&file)),
            duration: model.len,
        }
    });
    let state = match (model.playing().is_some(), model.play_state) {
        (false, _) => media::Playback::Stopped,
        (true, player::PlayState::Playing) => media::Playback::Playing,
        (true, player::PlayState::Paused) => media::Playback::Paused,
    };
    media.publish(media::Snapshot { meta, state, position: model.pos });
}

/// A `file://` URL for a path, which is what MPRIS wants of cover art.
fn url_of(path: &std::path::Path) -> Option<String> {
    let path = path.to_str()?;
    Some(format!("file://{path}"))
}

/// Writes out whatever part of the session has changed, and forgets that it had. Called once per
/// burst of messages: a held seek key reports many times a second, and none of that belongs on disk.
///
/// Boxed, because the two writes have different types and there are at most two of them a burst.
pub fn save_session(model: &mut Model) -> Vec<Pin<Box<dyn Future<Output = ()> + Send>>> {
    let mut writes: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = Vec::new();
    if std::mem::take(&mut model.dirty_playlist) {
        let tracks = model.queue.iter().map(|item| item.path.clone()).collect();
        writes.push(Box::pin(session::save_playlist(paths::playlist_file(), session::SavedPlaylist::new(tracks))));
    }
    if std::mem::take(&mut model.dirty_player) {
        let saved = session::SavedPlayer::new(model.current, model.repeat, model.sort);
        writes.push(Box::pin(session::save_player(paths::player_file(), saved)));
    }
    writes
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

    /// A picked genre and artist narrow the browser, and clearing brings everything back.
    #[test]
    fn filters_narrow_what_the_browser_shows() {
        let mut model = browser(6);
        let all = model.shown.len();
        assert_eq!(all, 6);

        send(&mut model, Msg::OpenPicker(Subject::Genre));
        send(&mut model, Msg::Typed('m'));
        send(&mut model, Msg::Pick);
        reconcile(&mut model);
        assert_eq!(model.filter.genre.as_deref(), Some("Metal"));
        assert!(model.shown.len() < all, "a genre narrows the list");
        assert!(model.shown.iter().all(|&ix| model.albums[ix].genre == "Metal"), "and everything shown is of that genre");

        send(&mut model, Msg::ClearFilters);
        reconcile(&mut model);
        assert_eq!(model.shown.len(), all, "clearing brings the rest back");
        assert!(model.filter.is_empty());
    }

    /// The picker's "any" row clears the filter it was setting.
    #[test]
    fn the_any_row_clears_a_filter() {
        let mut model = browser(6);
        send(&mut model, Msg::OpenPicker(Subject::Genre));
        send(&mut model, Msg::Typed('j'));
        send(&mut model, Msg::Pick);
        reconcile(&mut model);
        assert_eq!(model.filter.genre.as_deref(), Some("Jazz"));

        send(&mut model, Msg::OpenPicker(Subject::Genre));
        // Row 0 is "any genre", which is where a freshly opened picker sits.
        send(&mut model, Msg::Pick);
        reconcile(&mut model);
        assert_eq!(model.filter.genre, None, "picking `any` clears it");
    }

    /// Typing searches album titles, and what is typed survives leaving the field.
    #[test]
    fn typing_searches_album_titles() {
        let mut model = browser(20);
        send(&mut model, Msg::Search(Some('0')));
        send(&mut model, Msg::Typed('0'));
        send(&mut model, Msg::Typed('3'));
        reconcile(&mut model);
        assert_eq!(model.filter.search, "003");
        assert_eq!(model.shown.len(), 1, "one album is titled Album 003");

        send(&mut model, Msg::Rubout);
        reconcile(&mut model);
        assert_eq!(model.filter.search, "00");
        assert!(model.shown.len() > 1, "rubbing out widens the search again");

        send(&mut model, Msg::Done);
        assert!(matches!(model.focus, Focus::Albums), "the keys go back to the list");
        assert_eq!(model.filter.search, "00", "and the search it narrowed to stands");
    }

    /// Rubbing out from the album list edits the search, rather than doing nothing: having typed a
    /// search and gone back to the list, backspace is how it gets corrected.
    #[test]
    fn rubbing_out_from_the_list_edits_the_search() {
        let mut model = browser(20);
        send(&mut model, Msg::Search(Some('0')));
        send(&mut model, Msg::Typed('0'));
        send(&mut model, Msg::Typed('3'));
        send(&mut model, Msg::Done);
        assert!(matches!(model.focus, Focus::Albums));

        send(&mut model, Msg::Rubout);
        reconcile(&mut model);
        assert!(matches!(model.focus, Focus::Search), "the keys go to the search");
        assert_eq!(model.filter.search, "00", "and the last character is gone");
    }

    /// The sort picker changes the order, and opens on the order in use.
    #[test]
    fn the_sort_picker_changes_the_order() {
        let mut model = browser(6);
        let before = model.shown.clone();
        send(&mut model, Msg::OpenPicker(Subject::Sort));
        let Focus::Picker(picker) = &model.focus else { panic!("a picker should be open") };
        assert_eq!(SortOrder::ALL[picker.selected], model.sort, "the picker opens on the order in use");

        send(&mut model, Msg::PickerMove(1));
        send(&mut model, Msg::Pick);
        reconcile(&mut model);
        assert_ne!(model.sort, SortOrder::default());
        assert_ne!(model.shown, before, "the browser is reordered");
    }

    /// Playing everything shown queues the filtered albums, not the whole library.
    #[test]
    fn playing_everything_shown_respects_the_filter() {
        let mut model = browser(6);
        send(&mut model, Msg::OpenPicker(Subject::Genre));
        send(&mut model, Msg::Typed('m'));
        send(&mut model, Msg::Pick);
        reconcile(&mut model);
        let shown: Vec<u64> = model.shown.iter().map(|&ix| model.albums[ix].id).collect();

        send(&mut model, Msg::PlayShown);
        let queued: std::collections::HashSet<u64> = model.queue.iter().map(|item| item.album_id).collect();
        assert_eq!(queued.len(), shown.len(), "every shown album is queued and nothing else");
        assert!(queued.iter().all(|id| shown.contains(id)));
    }

    /// Shuffling the rest keeps the playing track playing and where it is, so the reordering never
    /// interrupts anything.
    #[test]
    fn shuffling_the_others_keeps_the_playing_track() {
        let mut model = browser(4);
        send(&mut model, Msg::PlayShown);
        let queued = model.queue.len();
        assert!(queued > 4, "several albums' tracks are queued");
        send(&mut model, Msg::Player(player::Event::TrackStarted { ix: 3, len: None }));
        let playing = model.queue[3].path.clone();

        send(&mut model, Msg::Shuffle { grouping: Grouping::Tracks, scope: Scope::Others });
        assert_eq!(model.queue.len(), queued, "no track is lost or duplicated");
        assert_eq!(model.current, 0, "the playing track leads the queue");
        assert_eq!(model.queue[0].path, playing, "and it is the same track");
    }

    /// Shuffling everything rests paused on whatever came out first.
    #[test]
    fn shuffling_everything_rests_at_the_front() {
        let mut model = browser(4);
        send(&mut model, Msg::PlayShown);
        send(&mut model, Msg::Player(player::Event::TrackStarted { ix: 5, len: None }));
        send(&mut model, Msg::Player(player::Event::Progress(Duration::from_secs(20))));

        send(&mut model, Msg::Shuffle { grouping: Grouping::Albums, scope: Scope::All });
        assert_eq!(model.current, 0);
        assert_eq!(model.pos, Duration::ZERO, "the bar rests at the start of the new first track");
    }

    /// Clearing the queue leaves nothing playing and nothing to show.
    #[test]
    fn clearing_the_queue_empties_it() {
        let mut model = browser(3);
        send(&mut model, Msg::PlaySelected);
        assert!(!model.queue.is_empty());

        send(&mut model, Msg::ClearQueue);
        assert!(model.queue.is_empty());
        assert!(model.playing().is_none());
        assert_eq!(model.play_state, player::PlayState::Paused);
    }

    /// A queue restored from paths alone gets its titles and album ids from the library, and gets them
    /// again when the library is rescanned under it.
    #[test]
    fn a_restored_queue_takes_its_tags_from_the_library() {
        use phonoscule::session::Restored;

        let known = browser(4);
        let paths: Vec<std::path::PathBuf> = known.albums[1].tracks.iter().map(|t| t.path.clone()).collect();
        let restored = Restored { tracks: paths.clone(), current: 0, ..Default::default() };

        // Restored with no library at all: the paths are all there is to show.
        let conf = phonoscule::config::Conf::new("tui", "/music".into());
        let covers = crate::covers::Covers::new(ratatui_image::picker::Picker::halfblocks(), None);
        let engine = player::start(player::Client { name: "restore-test".into(), description: String::new() });
        let mut model = Model::restored(conf, covers, engine, vec![], restored);
        assert_eq!(model.queue.len(), paths.len());
        assert!(model.queue.iter().all(|item| item.album_id == 0), "no album is known yet");

        // The library arrives, and the queue takes its tags from it.
        for album in known.albums.iter().cloned() {
            let _ = update(&mut model, Msg::Library(library::ScanEvent::Album(Box::new(album))));
        }
        reconcile(&mut model);
        assert!(model.queue.iter().all(|item| item.album_id == known.albums[1].id), "every item found its album");
        assert_eq!(model.queue[0].title, known.albums[1].tracks[0].title, "and its title");
    }

    /// Volume within a hair of `wanted`. Stepping accumulates floats, so exact equality is the wrong
    /// question to ask of it.
    fn assert_volume(model: &Model, wanted: f32) {
        let got = model.volume.expect("a volume should be known");
        assert!((got - wanted).abs() < 0.001, "volume {got} should be about {wanted}");
    }

    /// The volume is not stepped before the mixer has said where it is: there is nothing to step from,
    /// and guessing at full would jump someone's volume on the first key press.
    #[test]
    fn volume_does_nothing_until_the_mixer_reports() {
        let mut model = browser(2);
        assert_eq!(model.volume, None);
        send(&mut model, Msg::BumpVolume(0.05));
        assert_eq!(model.volume, None, "still nothing to show");

        send(&mut model, Msg::VolumeChanged(0.4));
        assert_volume(&model, 0.4);
        send(&mut model, Msg::BumpVolume(0.05));
        assert_volume(&model, 0.45);
    }

    /// Volume steps accumulate, and the mixer's echoes of earlier values do not drag the bar back --
    /// the same fault the seek bar had.
    #[test]
    fn stale_volume_readings_do_not_drag_the_bar_back() {
        let mut model = browser(2);
        send(&mut model, Msg::VolumeChanged(0.5));

        for _ in 0..4 {
            send(&mut model, Msg::BumpVolume(0.05));
        }
        assert_volume(&model, 0.7);

        for stale in [0.5, 0.55, 0.6] {
            send(&mut model, Msg::VolumeChanged(stale));
            assert_volume(&model, 0.7);
        }

        // Once the mixer arrives at what was asked, its readings drive the bar again -- which is how
        // a change made in some other mixer still shows up here.
        send(&mut model, Msg::VolumeChanged(0.7));
        send(&mut model, Msg::VolumeChanged(0.72));
        assert_volume(&model, 0.72);
    }

    /// Stepping past either end stops there rather than wrapping or asking the mixer for nonsense.
    #[test]
    fn volume_clamps_at_both_ends() {
        let mut model = browser(2);
        send(&mut model, Msg::VolumeChanged(0.95));
        for _ in 0..4 {
            send(&mut model, Msg::BumpVolume(0.05));
        }
        assert_volume(&model, 1.0);

        // Settle the set before pretending the mixer went elsewhere, or the reading is ignored as the
        // stale echo it would otherwise look like.
        send(&mut model, Msg::VolumeChanged(1.0));
        send(&mut model, Msg::VolumeChanged(0.05));
        for _ in 0..4 {
            send(&mut model, Msg::BumpVolume(-0.05));
        }
        assert_volume(&model, 0.0);
    }

    /// Enter opens the album's tracks rather than playing it, and Enter on a track plays that track
    /// alone -- which is the whole point of the menu.
    #[test]
    fn the_track_menu_plays_one_track() {
        let mut model = browser(4);
        send(&mut model, Msg::Select(1));
        let album = model.selected_album().expect("an album").clone();
        assert!(album.tracks.len() > 1);

        send(&mut model, Msg::OpenTracks);
        assert!(model.queue.is_empty(), "opening the menu plays nothing");
        assert_eq!(model.track_menu().map(|m| m.album), Some(album.id));

        send(&mut model, Msg::TrackMove(1));
        send(&mut model, Msg::PlayTrack);
        assert_eq!(model.queue.len(), 1, "one track, not the album");
        assert_eq!(model.queue[0].title, album.tracks[1].title);
        assert!(matches!(model.focus, Focus::Albums), "the menu closes behind it");
    }

    /// Queueing a track steps onto the next, so holding it down queues a run rather than the same
    /// track over and over.
    #[test]
    fn queueing_tracks_walks_down_the_album() {
        let mut model = browser(4);
        send(&mut model, Msg::OpenTracks);
        let album = model.acting_album().expect("an album").clone();

        for _ in 0..album.tracks.len() {
            send(&mut model, Msg::QueueTrack);
        }
        assert_eq!(model.queue.len(), album.tracks.len(), "each press queued the next track");
        let titles: Vec<&str> = model.queue.iter().map(|item| item.title.as_str()).collect();
        let expected: Vec<&str> = album.tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, expected, "in album order");
    }

    /// With the menu open, playing and queueing "everything in the list" means the album, not every
    /// album the filter lets through.
    #[test]
    fn the_menu_scopes_play_all_to_its_album() {
        let mut model = browser(4);
        send(&mut model, Msg::Select(2));
        let album = model.selected_album().expect("an album").clone();
        send(&mut model, Msg::OpenTracks);

        send(&mut model, Msg::PlaySelected);
        assert_eq!(model.queue.len(), album.tracks.len(), "the album, not the library");
        assert!(model.queue.iter().all(|item| item.album_id == album.id));
    }

    /// The menu selection cannot walk off either end of the album.
    #[test]
    fn the_track_selection_stays_within_the_album() {
        let mut model = browser(2);
        send(&mut model, Msg::OpenTracks);
        let tracks = model.acting_album().expect("an album").tracks.len();

        send(&mut model, Msg::TrackMove(isize::MAX));
        assert_eq!(model.track_menu().map(|m| m.selected), Some(tracks - 1));
        send(&mut model, Msg::TrackMove(isize::MIN));
        assert_eq!(model.track_menu().map(|m| m.selected), Some(0));
    }
}
