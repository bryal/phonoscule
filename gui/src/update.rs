//! The messages, and how each of them changes the model.

use crate::model::{
    App, Filter, Modal, ModalKind, PICKER_INPUT_ID, PICKER_SCROLL_ID, Picker, PickerSubject, QueueItem, ScanState,
    TRACK_MENU_SCROLL_ID, TrackMenu, View, album_runs, current_album_id, current_glow, entries, flow_target, glow_blend,
    picker_matches, queue_items, refresh_filter, run_of,
};
use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};
use phonoscule_gui::library::{self, Album};
use phonoscule_gui::{media, player, playlist};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub enum Msg {
    Library(library::ScanEvent),
    /// Time to poll the music directory for changes.
    Rescan,
    Show(View),
    PlayAlbum(usize),
    QueueAlbum(usize),
    /// Warm the global cache with an album's high-res cover ahead of a likely play: the library
    /// grid fires this when the cursor enters an album's play bubble, so the decode overlaps the
    /// moment between hover and click and the cover is ready (or nearly) by the time the flow shows
    /// it. Idempotent -- a hover that never becomes a click just ages back out of the LRU.
    PreloadAlbum(usize),
    /// The library grid's selection changed; store it so it survives view switches (the grid's
    /// own state drops with the view -- see `AlbumGrid::selected`). Like every grid message, the
    /// index is into the *filtered* album list.
    AlbumSelected(Option<usize>),
    /// The album-title search field changed: refresh the filtered grid.
    SearchChanged(String),
    /// Clear every library filter (the bar's ✕ button), showing the whole library again.
    ClearFilters,
    /// Play all albums matching the current filter, in their displayed order, replacing the queue.
    PlayAll,
    /// Append all albums matching the current filter, in their displayed order, to the queue.
    QueueAll,
    /// Open the searchable filter picker for the given subject (a filter-bar chip), focusing its
    /// search field.
    OpenPicker(PickerSubject),
    /// The picker's search field changed: re-rank its matches and reset the selection to the top.
    PickerQuery(String),
    /// Move the picker's keyboard selection one slot up or down (arrow keys -- they pass through
    /// the focused search field, so this works while typing).
    PickerMove(MenuDir),
    /// The cursor entered a picker row: move the selection there.
    PickerHover(usize),
    /// Pick the picker row under the mouse (a click).
    PickerChoose(usize),
    /// Pick the picker's selected row (Enter -- via the search field's on_submit while it has
    /// focus, or the key binding otherwise).
    PickerPick,
    /// Open the modal listing an album's tracks (the card's list bubble, a left-click on its
    /// cover, or Enter on the selection), to play or queue tracks individually.
    OpenTrackMenu(usize),
    /// Open the player actions menu (the player bar's ellipsis button): shuffle and friends.
    OpenActionsMenu,
    /// Dismiss whatever modal is up (Escape, or a click outside it).
    CloseModal,
    /// Cycle the repeat mode: off -> track -> album -> playlist -> off (the `r` key, and the
    /// player bar's repeat button).
    CycleRepeat,
    /// Shuffle the queue, in place and visibly, per the grouping (single tracks, or whole albums
    /// with their tracks kept together) and scope (everything behind the playing item, or
    /// literally everything). `s`/`z` shuffle the other albums/tracks, Ctrl promotes to all; see
    /// `shuffle_queue`.
    Shuffle {
        grouping: Grouping,
        scope: Scope,
        promotion: Promotion,
    },
    /// Reset playback: jump to the first track of the queue, paused (Backspace).
    ResetPlayback,
    /// Move the track menu's keyboard selection one track up or down (arrow keys).
    MenuMove(MenuDir),
    /// The cursor entered a track menu row: move the selection there, so the mouse and the arrow
    /// keys drive the same highlight.
    MenuHover(usize),
    /// Append the track menu's selected track to the queue (Space), stepping the selection to the
    /// next track so successive presses queue an album run.
    MenuQueue,
    /// Play the track menu's selected track, replacing the queue (Ctrl+Space or Enter).
    MenuPlay,
    /// Play a single track, replacing the queue with it (a play bubble in the track menu).
    PlayTrack {
        album: usize,
        track: usize,
    },
    /// Append a single track to the queue (an enqueue bubble in the track menu).
    QueueTrack {
        album: usize,
        track: usize,
    },
    Player(player::Event),
    Media(media::Control),
    Toggle,
    /// Step to the next track (End, and the on-screen button). `repeat` marks a held-key
    /// auto-repeat, which the handler rate-limits; a fresh press or button click passes `false`.
    Next {
        repeat: bool,
    },
    Prev,
    /// Restart the current track, or step to the previous one if playback is already near the
    /// start (Home). Position-dependent, so it's resolved here rather than in the key mapping.
    /// `repeat` marks a held-key auto-repeat, rate-limited like [`Next`](Msg::Next).
    PrevOrRestart {
        repeat: bool,
    },
    /// Jump between albums in the queue (PageUp/PageDown). PageUp restarts the current album, or
    /// steps to the previous one if already at its first track; PageDown jumps to the next album.
    PrevAlbum,
    NextAlbum,
    CoverClicked(usize),
    TrackClicked(usize),
    SeekChanged(f32),
    SeekReleased,
    /// A relative or absolute seek, from the keyboard or the OS media keys (the seek *bar* uses
    /// SeekChanged/SeekReleased instead, since a drag is a stream of absolute fractions).
    Seek(Seek),
    /// A high-resolution cover finished decoding (`None` if the decode failed), to be stored in the
    /// global high-res cache (see `ensure_hires`).
    HiResLoaded {
        id: u64,
        pixels: Option<Arc<[u8]>>,
    },
    Frame(Instant),
}

/// A seek that isn't a drag of the bar: a jump relative to the current position, or to the start
/// of the track.
#[derive(Debug, Clone, Copy)]
pub enum Seek {
    By(SeekDir, Duration),
    ToStart,
}

#[derive(Debug, Clone, Copy)]
pub enum SeekDir {
    Forward,
    Backward,
}

/// A keyboard move of the track menu's selection.
#[derive(Debug, Clone, Copy)]
pub enum MenuDir {
    Up,
    Down,
}

pub fn update(app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Library(library::ScanEvent::Album(mut album)) => {
            // Re-scans re-report albums we already have: upsert by the stable id, keeping the
            // already-loaded cover art when the cover is unchanged (the scanner skips
            // re-decoding and re-sending it).
            if let Some(ix) = app.albums.iter().position(|a| a.id == album.id) {
                let old = app.albums.remove(ix);
                if old.cover_id == album.cover_id {
                    album.cover = old.cover;
                }
            }
            // A queue restored from disk carries only paths (placeholder tags, provisional album
            // keys) until the scan reports its albums: hydrate matching items by path. Covers not
            // yet decoded follow by album id through the Cover events.
            let paths: std::collections::HashSet<&std::path::PathBuf> = album.tracks.iter().map(|t| &t.path).collect();
            for item in app.queue.iter_mut().filter(|item| paths.contains(&item.path)) {
                item.album_id = album.id;
                item.artist = album.artist.clone();
                item.album = album.title.clone();
                item.cover = album.cover.clone();
                if let Some(track) = album.tracks.iter().find(|t| t.path == item.path) {
                    item.title = track.title.clone();
                }
            }
            // Keep the browser sorted; scan order is nondeterministic (directories complete
            // in parallel).
            let key = |a: &Album| (a.artist.to_lowercase(), a.title.to_lowercase());
            let ix = app.albums.partition_point(|a| key(a) <= key(&album));
            app.albums.insert(ix, album);
            refresh_filter(app);
        }
        Msg::Library(library::ScanEvent::Cover { albums, art }) => {
            for album in app.albums.iter_mut().filter(|a| albums.contains(&a.id)) {
                album.cover = Some(art.clone());
            }
            for item in app.queue.iter_mut().filter(|i| albums.contains(&i.album_id)) {
                item.cover = Some(art.clone());
            }
            // The playing track's cover art may just have arrived -- notably right after boot,
            // when a restored queue's covers all hydrate through the scan. Re-publish it, and
            // (re)fill the cover flow's high-res window that TrackStarted found coverless.
            if app.queue.get(app.current).is_some_and(|item| albums.contains(&item.album_id)) {
                publish_media(app);
                return ensure_hires(app);
            }
        }
        Msg::Library(library::ScanEvent::Done { album_ids }) => {
            let ids: std::collections::HashSet<u64> = album_ids.into_iter().collect();
            app.albums.retain(|album| ids.contains(&album.id));
            app.scan = ScanState::Complete;
            refresh_filter(app);
        }
        Msg::Rescan => match app.scan {
            // The running scan will pick changes up anyway.
            ScanState::Scanning => (),
            ScanState::Complete => {
                app.scan = ScanState::Scanning;
                return Task::run(library::scan(rescan_options(app)), Msg::Library);
            }
        },
        Msg::Show(v) => app.view = v,
        // Grid messages carry indices into the filtered list: resolve them to real album indices
        // here, at the boundary, so everything downstream (the track menu included) speaks real
        // indices.
        Msg::PlayAlbum(cell) => {
            if let Some(ix) = shown_album(app, cell) {
                return play_album(app, ix);
            }
        }
        Msg::QueueAlbum(cell) => {
            if let Some(ix) = shown_album(app, cell) {
                return queue_album(app, ix);
            }
        }
        Msg::PreloadAlbum(cell) => {
            if let Some(ix) = shown_album(app, cell) {
                return preload_cover(app, ix);
            }
        }
        Msg::AlbumSelected(selected) => app.selected = selected,
        Msg::OpenTrackMenu(cell) => {
            if let Some(ix) = shown_album(app, cell) {
                app.modal = Some(Modal::Tracks(TrackMenu { album: ix, selected: 0 }));
                // A play from the menu is likely imminent: warm the album's high-res cover.
                return preload_cover(app, ix);
            }
        }
        Msg::SearchChanged(search) => {
            app.filter.search = search;
            // The old selection would silently point at a different album in the new list.
            app.selected = None;
            refresh_filter(app);
        }
        Msg::ClearFilters => {
            app.filter = Filter::default();
            app.selected = None;
            refresh_filter(app);
        }
        Msg::PlayAll => {
            let items: Vec<QueueItem> = app.filtered.iter().flat_map(|&ix| queue_items(&app.albums[ix])).collect();
            if !items.is_empty() {
                return play_items(app, items);
            }
        }
        Msg::QueueAll => {
            let items: Vec<QueueItem> = app.filtered.iter().flat_map(|&ix| queue_items(&app.albums[ix])).collect();
            if !items.is_empty() {
                app.send(player::Cmd::Append { tracks: entries(&items) });
                app.queue.extend(items);
                return save_playlist(app);
            }
        }
        Msg::OpenPicker(subject) => {
            let matches = picker_matches(app, subject, "");
            app.modal = Some(Modal::Picker(Picker { subject, query: String::new(), matches, selected: 0 }));
            // Focus the search field, so typing starts filtering immediately.
            use iced::advanced::widget;
            return widget::operate(widget::operation::focusable::focus(widget::Id::new(PICKER_INPUT_ID)));
        }
        Msg::PickerQuery(query) => {
            let Some(Modal::Picker(picker)) = &app.modal else { return Task::none() };
            let matches = picker_matches(app, picker.subject, &query);
            if let Some(Modal::Picker(picker)) = &mut app.modal {
                picker.query = query;
                picker.matches = matches;
                picker.selected = 0;
            }
            return snap_picker(0.0);
        }
        Msg::PickerMove(dir) => return picker_step(app, dir),
        // No snap, unlike PickerMove: the cursor is already on the row it selected.
        Msg::PickerHover(slot) => {
            if let Some(Modal::Picker(picker)) = &mut app.modal {
                picker.selected = slot;
            }
        }
        Msg::PickerChoose(slot) => pick_filter(app, slot),
        Msg::PickerPick => {
            if let Some(Modal::Picker(picker)) = &app.modal {
                pick_filter(app, picker.selected);
            }
        }
        Msg::OpenActionsMenu => app.modal = Some(Modal::Actions),
        Msg::CloseModal => app.modal = None,
        Msg::MenuMove(dir) => return menu_step(app, dir),
        // No snap, unlike MenuMove: the cursor is already on the row it selected.
        Msg::MenuHover(track) => {
            if let Some(Modal::Tracks(menu)) = &mut app.modal {
                menu.selected = track;
            }
        }
        Msg::MenuQueue => {
            if let Some(menu) = app.track_menu()
                && let Some(item) = track_item(app, menu.album, menu.selected)
            {
                app.send(player::Cmd::Append { tracks: entries(std::slice::from_ref(&item)) });
                app.queue.push(item);
                // Step onto the next track, so successive presses queue an album run.
                let step = menu_step(app, MenuDir::Down);
                return Task::batch([step, save_playlist(app)]);
            }
        }
        Msg::MenuPlay => {
            if let Some(menu) = app.track_menu() {
                app.modal = None;
                if let Some(item) = track_item(app, menu.album, menu.selected) {
                    return play_items(app, vec![item]);
                }
            }
        }
        Msg::CycleRepeat => {
            app.repeat = app.repeat.cycled();
            app.send(player::Cmd::SetRepeat(app.repeat));
            return save_player(app);
        }
        Msg::Shuffle { grouping, scope, promotion } => return shuffle_queue(app, grouping, scope, promotion),
        Msg::ResetPlayback => {
            if !app.queue.is_empty() {
                app.current = 0;
                app.anim_pos = flow_target(app);
                // Replacing the queue with itself reopens the first track paused: ready to play,
                // its length on the seek bar -- the same way a restored session comes up.
                app.send(player::Cmd::SetQueue { tracks: entries(&app.queue), start: 0, play: player::PlayState::Paused });
                return save_player(app);
            }
        }
        Msg::PlayTrack { album, track } => {
            // Playing switches to the player view; the menu has served its purpose.
            app.modal = None;
            if let Some(item) = track_item(app, album, track) {
                return play_items(app, vec![item]);
            }
        }
        Msg::QueueTrack { album, track } => {
            // Deliberately keeps the menu open: queueing several tracks in a row is the natural
            // flow, and Escape or a click outside ends it.
            if let Some(item) = track_item(app, album, track) {
                app.send(player::Cmd::Append { tracks: entries(std::slice::from_ref(&item)) });
                app.queue.push(item);
                return save_playlist(app);
            }
        }
        Msg::Player(event) => match event {
            player::Event::TrackStarted { ix, len } => {
                app.current = ix;
                app.len = len;
                app.pos = Duration::ZERO;
                // A new track invalidates any seek still settling against the old one.
                app.pending_seek = None;
                // A new album (re)starts the glow/cover-flow animation after a possibly long
                // idle stretch; reset the frame clock so the first frame's dt is one frame, not
                // the whole idle gap (which would jump the animation far in a single step).
                app.last_frame = Instant::now();
                publish_media(app);
                // The playing album moved: ensure the cover flow's high-res window around it, and
                // remember the new position for the next restore.
                return Task::batch([ensure_hires(app), save_player(app)]);
            }
            player::Event::Progress(t) => {
                // While a seek is settling, ignore reports until playback reaches (roughly) the
                // requested position: earlier reports still in flight would otherwise yank the
                // optimistically-placed bar back to where playback was before the seek.
                if matches!(app.pending_seek, Some(target) if t.abs_diff(target) > SEEK_SETTLE) {
                    return Task::none();
                }
                app.pending_seek = None;
                app.pos = t;
                // Publish freely -- the media worker coalesces a burst down to the latest, so the
                // position stream doubles as the heartbeat that flushes a pending track change.
                publish_media(app);
            }
            player::Event::PlayState(state) => {
                app.play_state = state;
                publish_media(app);
            }
            player::Event::QueueEnded => {
                app.play_state = player::PlayState::Paused;
                // The queue may have ended through a skip: rest the bar at the end rather than
                // wherever the last track happened to be.
                app.pos = app.len.unwrap_or(Duration::ZERO);
                app.pending_seek = None;
                // Report Stopped to the OS (the bar still shows the last track, but nothing plays).
                app.media.publish(media::Snapshot { meta: None, state: media::Playback::Stopped, position: app.pos });
            }
        },
        Msg::Media(control) => match control {
            media::Control::Play => match app.play_state {
                player::PlayState::Paused => app.send(player::Cmd::TogglePlayPause),
                player::PlayState::Playing => (),
            },
            // We have no stopped-with-a-track-open state; pausing is the closest thing.
            media::Control::Pause | media::Control::Stop => match app.play_state {
                player::PlayState::Playing => app.send(player::Cmd::TogglePlayPause),
                player::PlayState::Paused => (),
            },
            media::Control::Toggle => app.send(player::Cmd::TogglePlayPause),
            media::Control::Next => app.send(player::Cmd::Next),
            media::Control::Prev => app.send(player::Cmd::Prev),
            media::Control::Seek(offset) => {
                let dir = if offset >= 0 { SeekDir::Forward } else { SeekDir::Backward };
                do_seek(app, Seek::By(dir, Duration::from_micros(offset.unsigned_abs())));
            }
            media::Control::SetPosition(t) => app.send(player::Cmd::Seek(t)),
        },
        Msg::Toggle => app.send(player::Cmd::TogglePlayPause),
        Msg::Next { repeat } => {
            if skip_ready(app, repeat) {
                app.send(player::Cmd::Next);
            }
        }
        Msg::Prev => app.send(player::Cmd::Prev),
        Msg::PrevAlbum => prev_album(app),
        Msg::NextAlbum => next_album(app),
        Msg::PrevOrRestart { repeat } => {
            /// Below this, Home steps back a track rather than restarting the current one.
            const NEAR_START: Duration = Duration::from_millis(1500);
            if !skip_ready(app, repeat) {
                return Task::none();
            }
            if app.pos < NEAR_START {
                // Cmd::Prev steps to the previous track when playback is near the start. On a
                // double-press the second press lands here after the first has queued a seek to
                // zero, and the engine applies that seek before this Prev -- so it reliably sees
                // playback at the start and steps back rather than restarting again.
                app.send(player::Cmd::Prev);
            } else {
                do_seek(app, Seek::ToStart);
            }
        }
        Msg::CoverClicked(ix) => {
            let runs = album_runs(&app.queue);
            if ix == run_of(&runs, app.current) {
                app.send(player::Cmd::TogglePlayPause);
            } else if let Some(run) = runs.get(ix) {
                app.send(player::Cmd::JumpTo(run.start));
            }
        }
        Msg::TrackClicked(ix) => app.send(player::Cmd::JumpTo(ix)),
        Msg::SeekChanged(frac) => app.seek_drag = Some(frac),
        Msg::SeekReleased => {
            if let (Some(frac), Some(len)) = (app.seek_drag.take(), app.len) {
                let t = len.mul_f32(frac.clamp(0.0, 1.0));
                // No optimistic position update: the player reports its position right after
                // seeking (stale in-flight reports can't yank the bar around this way, at the
                // price of the bar resting at the old position for the seek's few ms).
                app.send(player::Cmd::Seek(t));
            }
        }
        Msg::Seek(seek) => do_seek(app, seek),
        // The cache absorbs its own query's result: memoized if it decoded, forgotten if it
        // failed. Kept even if the window has moved past this album -- a later hop back is then
        // instant, and the LRU bound retires it in time regardless.
        Msg::HiResLoaded { id, pixels } => app.hires.complete(id, pixels),
        Msg::Frame(now) => {
            // Clamp to ~one frame: after an idle stretch (frames only run while animating) the
            // gap since the last frame would otherwise lurch every animation forward at once.
            let dt = (now - app.last_frame).as_secs_f32().min(1.0 / 30.0);
            app.last_frame = now;

            // Exponential ease of the cover flow towards the current album run.
            let target = flow_target(app);
            app.anim_pos += (target - app.anim_pos) * (1.0 - (-9.0 * dt).exp());
            if (target - app.anim_pos).abs() < 0.002 {
                app.anim_pos = target;
            }

            // The backdrop glow blends from the album we're leaving into the current one over
            // glow_p (see glow_blend). Start a fresh blend whenever the target changes (album
            // change, or a cover finishing loading), freezing the current on-screen glow as the
            // new starting point so an interruption mid-blend continues smoothly.
            const GLOW_RATE: f32 = 1.5; // blends per second (1/x s per transition)
            let target = current_glow(app);
            if current_album_id(app) != app.glow_album || target != app.glow_to {
                app.glow_from = glow_blend(app.glow_from, app.glow_to, app.glow_p);
                app.glow_to = target;
                app.glow_album = current_album_id(app);
                app.glow_p = 0.0;
            }
            app.glow_p = (app.glow_p + GLOW_RATE * dt).min(1.0);
        }
    }
    Task::none()
}

/// Replaces the queue with the given items and switches to the player view, playing from the top.
/// Returns the playlist save task.
fn play_items(app: &mut App, items: Vec<QueueItem>) -> Task<Msg> {
    let play = player::PlayState::Playing;
    app.send(player::Cmd::SetQueue { tracks: entries(&items), start: 0, play });
    app.queue = items;
    app.current = 0;
    app.anim_pos = 0.0;
    app.view = View::Player;
    Task::batch([save_playlist(app), save_player(app)])
}

/// Replaces the queue with the album at `ix` and switches to the player view. No-op if the index
/// is out of range -- a rescan can shrink the list under a stale selection.
fn play_album(app: &mut App, ix: usize) -> Task<Msg> {
    let Some(album) = app.albums.get(ix) else { return Task::none() };
    let items = queue_items(album);
    play_items(app, items)
}

/// Appends the album at `ix` to the queue. No-op if the index is out of range.
fn queue_album(app: &mut App, ix: usize) -> Task<Msg> {
    let Some(album) = app.albums.get(ix) else { return Task::none() };
    let items = queue_items(album);
    app.send(player::Cmd::Append { tracks: entries(&items) });
    app.queue.extend(items);
    save_playlist(app)
}

/// Snapshots the queue's tracks to disk, fire-and-forget (see the playlist module). Returned by
/// everything that changes the queue, so a crash or an exit at any point loses nothing.
fn save_playlist(app: &App) -> Task<Msg> {
    let tracks = app.queue.iter().map(|item| item.path.clone()).collect();
    Task::future(playlist::save_playlist(playlist::playlist_file(), playlist::SavedPlaylist::new(tracks))).discard()
}

/// Snapshots the session state around the queue (current track, repeat mode) to disk; returned by
/// everything that changes either. Split from [`save_playlist`]: track changes are frequent and
/// needn't rewrite the whole track list.
fn save_player(app: &App) -> Task<Msg> {
    Task::future(playlist::save_player(playlist::player_file(), playlist::SavedPlayer::new(app.current, app.repeat))).discard()
}

/// What a shuffle permutes: single tracks, or whole albums (each album's tracks stay together, in
/// their queue order, while the albums land in random order).
#[derive(Debug, Clone, Copy)]
pub enum Grouping {
    Tracks,
    Albums,
}

/// Whether a [`Scope::Others`] shuffle may promote to [`Scope::All`] when playback sits paused on
/// the queue's first track, nothing begun -- a state that reads "shuffle me a fresh playlist",
/// not "keep my current track first". The bare `s`/`z` keys promote; the actions menu's entries
/// say exactly what they do, so they stay literal.
#[derive(Debug, Clone, Copy)]
pub enum Promotion {
    Auto,
    Literal,
}

/// How much of the queue a shuffle reorders.
#[derive(Debug, Clone, Copy)]
pub enum Scope {
    /// The playing track (or its whole album) moves to the front of the queue and everything else
    /// shuffles in behind it, so nothing lands unreachably behind the cursor; playback continues
    /// undisturbed.
    Others,
    /// Literally everything shuffles: playback is interrupted and the cursor rests, paused, on
    /// the queue's new first track.
    All,
}

/// Shuffles the queue in place, visibly: the reordering IS the new playlist (persisted like any
/// other queue change, so a restart resumes the same order), and the cover flow snaps to the
/// cursor's new position rather than sweeping. See [`Scope`] for what moves and what keeps
/// playing, and [`Promotion`] for when a shuffle of the others becomes a shuffle of everything
/// (it's the state Backspace resets to, making Ctrl+s and Backspace-then-s equivalent).
fn shuffle_queue(app: &mut App, grouping: Grouping, scope: Scope, promotion: Promotion) -> Task<Msg> {
    if app.queue.is_empty() {
        return Task::none();
    }
    // Shuffling from the actions menu: the action dismisses it.
    if matches!(app.modal, Some(Modal::Actions)) {
        app.modal = None;
    }
    let scope = match (scope, promotion, app.current, app.play_state) {
        (Scope::Others, Promotion::Auto, 0, player::PlayState::Paused) => Scope::All,
        (scope, ..) => scope,
    };

    // Build the new order as a permutation of indices, so the current track can be followed by
    // identity (queue items need not be unique -- an album can be queued twice). Tracks are
    // singleton groups; albums group all of an album's tracks, wherever they sit, in their queue
    // order.
    let mut groups: Vec<Vec<usize>> = match grouping {
        Grouping::Tracks => (0..app.queue.len()).map(|ix| vec![ix]).collect(),
        Grouping::Albums => {
            let mut groups: Vec<(u64, Vec<usize>)> = Vec::new();
            for (ix, item) in app.queue.iter().enumerate() {
                match groups.iter_mut().find(|(album, _)| *album == item.album_id) {
                    Some((_, ixs)) => ixs.push(ix),
                    None => groups.push((item.album_id, vec![ix])),
                }
            }
            groups.into_iter().map(|(_, ixs)| ixs).collect()
        }
    };
    match scope {
        Scope::All => shuffle(&mut groups),
        Scope::Others => {
            // Pin the playing group to the front; only the rest shuffles.
            let playing = groups.iter().position(|group| group.contains(&app.current)).unwrap_or(0);
            groups.swap(0, playing);
            shuffle(&mut groups[1..]);
        }
    }
    let order: Vec<usize> = groups.into_iter().flatten().collect();

    let mut old: Vec<Option<QueueItem>> = std::mem::take(&mut app.queue).into_iter().map(Some).collect();
    app.queue = order.iter().map(|&ix| old[ix].take().expect("a permutation visits each index once")).collect();
    match scope {
        // The engine is handed the same tracks in the new order and only the cursor follows the
        // playing track (see `Cmd::Reorder`).
        Scope::Others => {
            app.current = order.iter().position(|&ix| ix == app.current).unwrap_or(0);
            app.send(player::Cmd::Reorder { tracks: entries(&app.queue), current: app.current });
        }
        // A fresh start: the new first track opens paused, ready to play.
        Scope::All => {
            app.current = 0;
            app.send(player::Cmd::SetQueue { tracks: entries(&app.queue), start: 0, play: player::PlayState::Paused });
        }
    }
    app.anim_pos = flow_target(app);
    Task::batch([save_playlist(app), save_player(app)])
}

/// Fisher-Yates over a splitmix64 stream seeded from the clock: not cryptographic, plenty for
/// shuffling a music queue, and spares a randomness dependency.
fn shuffle<T>(items: &mut [T]) {
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let mut state = seed.map_or(0x9E37_79B9_7F4A_7C15, |d| d.as_nanos() as u64);
    let mut next = move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    for i in (1..items.len()).rev() {
        // The modulo bias is immaterial at queue sizes.
        items.swap(i, (next() % (i as u64 + 1)) as usize);
    }
}

/// Resolves a grid message's index (into the filtered list) to a real album index, or `None` for
/// a stale cell (the filter can change under an in-flight message).
fn shown_album(app: &App, cell: usize) -> Option<usize> {
    app.filtered.get(cell).copied()
}

/// Applies the picker's slot `slot` to the filter: slot 0 (the standing "(all)" entry) clears it,
/// slot `n + 1` picks `matches[n]`. Closes the picker, drops the grid selection (it indexes the
/// filtered list, which is about to change), and refreshes the grid.
fn pick_filter(app: &mut App, slot: usize) {
    let Some(Modal::Picker(picker)) = &app.modal else { return };
    let value = match slot.checked_sub(1) {
        None => None,
        Some(n) => match picker.matches.get(n) {
            Some(value) => Some(value.clone()),
            None => return, // a stale slot; keep the picker open
        },
    };
    match picker.subject {
        PickerSubject::Genre => app.filter.genre = value,
        PickerSubject::Artist => app.filter.artist = value,
    }
    app.modal = None;
    app.selected = None;
    refresh_filter(app);
}

/// Moves the picker's keyboard selection one slot, clamped to its entries ("(all)" plus the
/// matches), and snaps the list so the selection stays in view -- proportional, like the track
/// menu (see `menu_step`).
fn picker_step(app: &mut App, dir: MenuDir) -> Task<Msg> {
    let Some(Modal::Picker(picker)) = &mut app.modal else { return Task::none() };
    let last = picker.matches.len(); // slots run 0..=len
    picker.selected = match dir {
        MenuDir::Up => picker.selected.saturating_sub(1),
        MenuDir::Down => (picker.selected + 1).min(last),
    };
    let fraction = if last > 0 { picker.selected as f32 / last as f32 } else { 0.0 };
    snap_picker(fraction)
}

/// Snaps the picker's list to the given relative position.
fn snap_picker(fraction: f32) -> Task<Msg> {
    use iced::advanced::widget;
    let to = widget::operation::scrollable::RelativeOffset { x: None, y: Some(fraction) };
    widget::operate(widget::operation::scrollable::snap_to(widget::Id::new(PICKER_SCROLL_ID), to))
}

/// The queue item for one track of the album at `album`, or `None` if either index is out of
/// range (a rescan can reshuffle the list under an open track menu).
fn track_item(app: &App, album: usize, track: usize) -> Option<QueueItem> {
    let album = app.albums.get(album)?;
    queue_items(album).into_iter().nth(track)
}

/// Moves the track menu's keyboard selection one step, clamped to the album's tracks, and snaps
/// the menu's scrollable so the selection stays in view. Snapping is proportional (selection at
/// fraction f of the list scrolls to fraction f), which keeps the selected row visible at every
/// position without knowing the list's pixel geometry.
fn menu_step(app: &mut App, dir: MenuDir) -> Task<Msg> {
    let Some(Modal::Tracks(menu)) = &mut app.modal else { return Task::none() };
    let Some(n) = app.albums.get(menu.album).map(|a| a.tracks.len()).filter(|&n| n > 0) else {
        return Task::none();
    };
    menu.selected = match dir {
        MenuDir::Up => menu.selected.saturating_sub(1),
        MenuDir::Down => (menu.selected + 1).min(n - 1),
    };
    let fraction = if n > 1 { menu.selected as f32 / (n - 1) as f32 } else { 0.0 };
    use iced::advanced::widget;
    let to = widget::operation::scrollable::RelativeOffset { x: None, y: Some(fraction) };
    widget::operate(widget::operation::scrollable::snap_to(widget::Id::new(TRACK_MENU_SCROLL_ID), to))
}

/// Warms the global high-res cache with the cover of the album at `ix` (see `HiResCache::query`);
/// idempotent, so callers fire it on any hint that the album is about to play.
fn preload_cover(app: &mut App, ix: usize) -> Task<Msg> {
    match app.albums.get(ix).and_then(|a| a.cover.as_ref()).map(|c| (c.id, c.file.clone())) {
        Some((id, file)) => app.hires.query(id, file),
        None => Task::none(),
    }
}

/// PageUp: restart the current album, or -- if playback is already at its first track -- step to
/// the previous album. Mirrors Home's restart-or-step behavior at album granularity. No-op before
/// the first album.
fn prev_album(app: &App) {
    let runs = album_runs(&app.queue);
    let cur = run_of(&runs, app.current);
    let target = match runs.get(cur) {
        Some(run) if run.start == app.current => cur.checked_sub(1),
        _ => Some(cur),
    };
    if let Some(run) = target.and_then(|ix| runs.get(ix)) {
        app.send(player::Cmd::JumpTo(run.start));
    }
}

/// PageDown: jump to the start of the next album. No-op past the last album.
fn next_album(app: &App) {
    let runs = album_runs(&app.queue);
    if let Some(run) = runs.get(run_of(&runs, app.current) + 1) {
        app.send(player::Cmd::JumpTo(run.start));
    }
}

/// Options for a periodic re-scan: skip re-decoding all the cover art we already hold.
fn rescan_options(app: &App) -> library::ScanOptions {
    library::ScanOptions {
        root: app.conf.music_dir.clone(),
        known_covers: app.albums.iter().filter_map(|a| a.cover.as_ref().map(|c| c.id)).collect(),
        cache_file: library::default_cache_file(),
        covers_dir: library::default_covers_dir(),
    }
}

/// Publishes the current now-playing state to the OS media integration. Fire-and-forget: the
/// media worker coalesces a burst of these down to the latest and rate-limits the actual pushes,
/// so callers need not throttle.
fn publish_media(app: &App) {
    let meta = app.queue.get(app.current).map(|item| media::Meta {
        title: item.title.clone(),
        album: item.album.clone(),
        artist: item.artist.clone(),
        // Absolute file:// URL so the OS can load the cover regardless of our working directory.
        cover_url: item.cover.as_ref().and_then(|c| url::Url::from_file_path(&*c.file).ok()).map(String::from),
        duration: app.len,
    });
    let state = match app.play_state {
        player::PlayState::Playing => media::Playback::Playing,
        player::PlayState::Paused => media::Playback::Paused,
    };
    app.media.publish(media::Snapshot { meta, state, position: app.pos });
}

/// How many album runs on each side of the playing one the cover flow ensures are held in the
/// high-res cache (plus the current run itself). Asymmetric because skipping forward is more common
/// than back. There is no separate eviction here: covers that fall outside this span stay in the
/// global cache until its LRU bound retires them (see [`HiResCache`](crate::model::HiResCache)), so
/// a short hop back finds them still resident and instant.
const ENSURE_PREV: usize = 8;
const ENSURE_NEXT: usize = 10;

/// Queries the high-res cache for the covers around the playing album, so it decodes the ones it
/// doesn't already hold and keeps the on-screen window hot in its LRU. The cache owns all the
/// fetch-or-decode bookkeeping (see [`HiResCache::query`](crate::model::HiResCache::query)); this
/// just declares the window. Returns the batch of resulting decode tasks (empty if all resident).
fn ensure_hires(app: &mut App) -> Task<Msg> {
    let runs = album_runs(&app.queue);
    if runs.is_empty() {
        return Task::none();
    }
    let center = run_of(&runs, app.current);
    let last = runs.len() - 1;
    let lo = center.saturating_sub(ENSURE_PREV);
    let hi = (center + ENSURE_NEXT).min(last);

    let mut tasks = Vec::new();
    for run in &runs[lo..=hi] {
        // Copy the id and ref-count the path out of the queue borrow before querying the cache.
        if let Some((id, file)) = app.queue.get(run.start).and_then(|it| it.cover.as_ref()).map(|c| (c.id, c.file.clone())) {
            tasks.push(app.hires.query(id, file));
        }
    }
    Task::batch(tasks)
}

/// How close a reported position must be to a pending seek's target to count as "arrived", after
/// which live reports drive the bar again. Wide enough that the first report once playback catches
/// up clears it, narrow relative to a seek step so it doesn't clear mid-scrub while holding a key.
const SEEK_SETTLE: Duration = Duration::from_secs(1);

/// Performs a [`Seek`]. The target is taken relative to [`pos`](App::pos) -- which is itself moved
/// optimistically below -- so a burst of relative seeks accumulates instead of all reading the
/// same round-trip-lagged position. Relative seeks saturate at zero and clamp to the track length.
fn do_seek(app: &mut App, seek: Seek) {
    let target = match seek {
        Seek::By(SeekDir::Forward, dt) => app.pos.saturating_add(dt),
        Seek::By(SeekDir::Backward, dt) => app.pos.saturating_sub(dt),
        Seek::ToStart => Duration::ZERO,
    };
    let target = app.len.map_or(target, |len| target.min(len));
    // Move the bar now and remember the target: the next relative seek reads this position, and
    // reports still in flight are ignored until playback reaches it (see the Progress handler).
    app.pos = target;
    app.pending_seek = Some(target);
    app.send(player::Cmd::Seek(target));
}

/// Whether a Home/End track skip should fire now, with staged auto-repeat acceleration. A fresh
/// press (`repeat` false) always fires and starts a new hold; while the key stays down the skip
/// rate ramps up the longer it's held (see [`skip_interval`]), so a quick tap stays precise but a
/// sustained hold races through a long queue.
fn skip_ready(app: &mut App, repeat: bool) -> bool {
    let now = Instant::now();
    if !repeat {
        app.hold_start = Some(now);
        app.last_skip = Some(now);
        return true;
    }
    let held = app.hold_start.map_or(Duration::ZERO, |t| now.duration_since(t));
    if app.last_skip.is_some_and(|t| now.duration_since(t) < skip_interval(held)) {
        return false;
    }
    app.last_skip = Some(now);
    true
}

/// The minimum spacing between held-key skips, as a function of how long the key has been held:
/// 8/s for the first 2s, then 20/s. Holding accelerates through a long queue while a short press
/// keeps fine, one-at-a-time control.
fn skip_interval(held: Duration) -> Duration {
    let per_sec: u32 = if held >= Duration::from_secs(2) { 20 } else { 8 };
    Duration::from_secs(1) / per_sec
}

/// Translates a key press into a message for the current `view`, or `None` for keys we don't bind.
/// Only keys no widget captured arrive here: the library grid handles its own navigation and
/// selection actions internally (see `album_grid`), so this covers the global view switching, the
/// playback-mode keys, and the player bindings. `repeat` marks auto-repeat from a held key:
/// continuous actions honor it (seek/scrub, walking the queue), while one-shot ones don't (holding
/// Space must not machine-gun play/pause). Alt/Logo always pass through to the window manager.
///
/// Global: Tab / Shift-Tab cycle the view tabs; `l`/`p` jump to Library/Player; Escape returns to
/// the library; `r` cycles the repeat mode; `s`/`z` shuffle the other albums/tracks in behind the
/// playing one (Ctrl promotes to shuffling literally everything); Backspace resets playback to the
/// queue's first track, paused. With a modal open, Escape dismisses it, the track menu gets its
/// own bindings, and everything else is suppressed. In the player: Left/Right seek by
/// [`SEEK_STEP`], Space toggles play/pause, Home
/// restarts the track (or steps back near the start), End steps to the next track, PageUp restarts
/// the album (or steps to the previous one), PageDown jumps to the next album.
pub fn key_to_msg(view: View, modal: Option<ModalKind>, key: Key, modifiers: Modifiers, repeat: bool) -> Option<Msg> {
    /// How far a single Left/Right tap seeks.
    const SEEK_STEP: Duration = Duration::from_secs(5);
    // Alt/Logo aren't bound anywhere; leave them (and their chords) to the window manager.
    if modifiers.alt() || modifiers.logo() {
        return None;
    }
    let one_shot = |msg| if repeat { None } else { Some(msg) };

    match modal {
        // The track menu is modal: it gets its own bindings and everything else is suppressed.
        // Mirrors the grid's vocabulary one level down -- arrows move the selection, Space queues
        // it, Ctrl+Space (or Enter, which opened the menu) plays it -- and Escape dismisses.
        Some(ModalKind::Tracks) => {
            return match (key, modifiers.control()) {
                (Key::Named(Named::Escape), false) => one_shot(Msg::CloseModal),
                (Key::Named(Named::ArrowUp), false) if modifiers.is_empty() => Some(Msg::MenuMove(MenuDir::Up)),
                (Key::Named(Named::ArrowDown), false) if modifiers.is_empty() => Some(Msg::MenuMove(MenuDir::Down)),
                // One queue per press: holding Space must not machine-gun the queue.
                (Key::Named(Named::Space), false) if modifiers.is_empty() => one_shot(Msg::MenuQueue),
                (Key::Named(Named::Space), true) => one_shot(Msg::MenuPlay),
                (Key::Named(Named::Enter), false) if modifiers.is_empty() => one_shot(Msg::MenuPlay),
                _ => None,
            };
        }
        // The actions menu: Escape dismisses; its actions keep their global keys (the entries
        // display them as hints, and the handlers dismiss the menu themselves).
        Some(ModalKind::Actions) => {
            return match (key, modifiers.control()) {
                (Key::Named(Named::Escape), false) => one_shot(Msg::CloseModal),
                (Key::Character(c), false) if c.as_str() == "r" => one_shot(Msg::CycleRepeat),
                (Key::Character(c), ctrl) if c.as_str() == "s" => one_shot(Msg::Shuffle {
                    grouping: Grouping::Albums,
                    scope: if ctrl { Scope::All } else { Scope::Others },
                    promotion: Promotion::Auto,
                }),
                (Key::Character(c), ctrl) if c.as_str() == "z" => one_shot(Msg::Shuffle {
                    grouping: Grouping::Tracks,
                    scope: if ctrl { Scope::All } else { Scope::Others },
                    promotion: Promotion::Auto,
                }),
                _ => None,
            };
        }
        // The filter picker: its focused search field captures typing (and Enter, via on_submit);
        // arrows pass through it, so the list navigates while typing. Escape reaches here once the
        // field is unfocused (the field captures the first press to unfocus itself).
        Some(ModalKind::Picker) => {
            return match key {
                Key::Named(Named::Escape) if !modifiers.control() => one_shot(Msg::CloseModal),
                Key::Named(Named::ArrowUp) if modifiers.is_empty() => Some(Msg::PickerMove(MenuDir::Up)),
                Key::Named(Named::ArrowDown) if modifiers.is_empty() => Some(Msg::PickerMove(MenuDir::Down)),
                Key::Named(Named::Enter) if modifiers.is_empty() => one_shot(Msg::PickerPick),
                _ => None,
            };
        }
        None => {}
    }

    // View-independent navigation takes precedence over the per-view bindings below.
    match (&key, modifiers.shift(), modifiers.control()) {
        (Key::Named(Named::Tab), false, false) => return one_shot(Msg::Show(view.next())),
        (Key::Named(Named::Tab), true, false) => return one_shot(Msg::Show(view.prev())),
        (Key::Named(Named::Escape), _, false) => return one_shot(Msg::Show(View::Library)),
        (Key::Character(c), false, false) if c.as_str() == "l" => return one_shot(Msg::Show(View::Library)),
        (Key::Character(c), false, false) if c.as_str() == "p" => return one_shot(Msg::Show(View::Player)),
        (Key::Character(c), false, false) if c.as_str() == "r" => return one_shot(Msg::CycleRepeat),
        (Key::Character(c), false, ctrl) if c.as_str() == "s" => {
            return one_shot(Msg::Shuffle {
                grouping: Grouping::Albums,
                scope: if ctrl { Scope::All } else { Scope::Others },
                promotion: Promotion::Auto,
            });
        }
        (Key::Character(c), false, ctrl) if c.as_str() == "z" => {
            return one_shot(Msg::Shuffle {
                grouping: Grouping::Tracks,
                scope: if ctrl { Scope::All } else { Scope::Others },
                promotion: Promotion::Auto,
            });
        }
        (Key::Named(Named::Backspace), false, false) => return one_shot(Msg::ResetPlayback),
        _ => {}
    }

    match view {
        View::Library => None,
        // The player view binds no Ctrl chords of its own.
        View::Player if modifiers.control() => None,
        View::Player => match key {
            Key::Named(Named::ArrowLeft) => Some(Msg::Seek(Seek::By(SeekDir::Backward, SEEK_STEP))),
            Key::Named(Named::ArrowRight) => Some(Msg::Seek(Seek::By(SeekDir::Forward, SEEK_STEP))),
            Key::Named(Named::Space) => one_shot(Msg::Toggle),
            Key::Named(Named::Home) => Some(Msg::PrevOrRestart { repeat }),
            Key::Named(Named::End) => Some(Msg::Next { repeat }),
            // One album per press: a held key mustn't fly through the whole queue.
            Key::Named(Named::PageUp) => one_shot(Msg::PrevAlbum),
            Key::Named(Named::PageDown) => one_shot(Msg::NextAlbum),
            _ => None,
        },
    }
}
