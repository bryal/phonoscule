//! The messages, and how each of them changes the model.

use crate::model::{
    App, QueueItem, ScanState, TRACK_MENU_SCROLL_ID, TrackMenu, View, album_runs, current_album_id, current_glow, flow_target,
    glow_blend, queue_items, run_of,
};
use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};
use phonoscule_gui::library::{self, Album};
use phonoscule_gui::{media, player};
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
    /// own state drops with the view -- see `AlbumGrid::selected`).
    AlbumSelected(Option<usize>),
    /// Open the modal listing an album's tracks (the card's list bubble, a left-click on its
    /// cover, or Enter on the selection), to play or queue tracks individually.
    OpenTrackMenu(usize),
    /// Dismiss the track menu (Escape, or a click outside it).
    CloseTrackMenu,
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
            // Keep the browser sorted; scan order is nondeterministic (directories complete
            // in parallel).
            let key = |a: &Album| (a.artist.to_lowercase(), a.title.to_lowercase());
            let ix = app.albums.partition_point(|a| key(a) <= key(&album));
            app.albums.insert(ix, album);
        }
        Msg::Library(library::ScanEvent::Cover { albums, art }) => {
            for album in app.albums.iter_mut().filter(|a| albums.contains(&a.id)) {
                album.cover = Some(art.clone());
            }
            for item in app.queue.iter_mut().filter(|i| albums.contains(&i.album_id)) {
                item.cover = Some(art.clone());
            }
            // The playing track's cover art may just have arrived.
            if app.queue.get(app.current).is_some_and(|item| albums.contains(&item.album_id)) {
                publish_media(app);
            }
        }
        Msg::Library(library::ScanEvent::Done { album_ids }) => {
            let ids: std::collections::HashSet<u64> = album_ids.into_iter().collect();
            app.albums.retain(|album| ids.contains(&album.id));
            app.scan = ScanState::Complete;
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
        Msg::PlayAlbum(ix) => play_album(app, ix),
        Msg::QueueAlbum(ix) => queue_album(app, ix),
        Msg::PreloadAlbum(ix) => return preload_cover(app, ix),
        Msg::AlbumSelected(selected) => app.selected = selected,
        Msg::OpenTrackMenu(ix) => {
            app.track_menu = Some(TrackMenu { album: ix, selected: 0 });
            // A play from the menu is likely imminent: warm the album's high-res cover.
            return preload_cover(app, ix);
        }
        Msg::CloseTrackMenu => app.track_menu = None,
        Msg::MenuMove(dir) => return menu_step(app, dir),
        // No snap, unlike MenuMove: the cursor is already on the row it selected.
        Msg::MenuHover(track) => {
            if let Some(menu) = &mut app.track_menu {
                menu.selected = track;
            }
        }
        Msg::MenuQueue => {
            if let Some(menu) = app.track_menu
                && let Some(item) = track_item(app, menu.album, menu.selected)
            {
                app.send(player::Cmd::Append { tracks: vec![item.path.clone()] });
                app.queue.push(item);
                // Step onto the next track, so successive presses queue an album run.
                return menu_step(app, MenuDir::Down);
            }
        }
        Msg::MenuPlay => {
            if let Some(menu) = app.track_menu {
                app.track_menu = None;
                if let Some(item) = track_item(app, menu.album, menu.selected) {
                    play_items(app, vec![item]);
                }
            }
        }
        Msg::PlayTrack { album, track } => {
            // Playing switches to the player view; the menu has served its purpose.
            app.track_menu = None;
            if let Some(item) = track_item(app, album, track) {
                play_items(app, vec![item]);
            }
        }
        Msg::QueueTrack { album, track } => {
            // Deliberately keeps the menu open: queueing several tracks in a row is the natural
            // flow, and Escape or a click outside ends it.
            if let Some(item) = track_item(app, album, track) {
                app.send(player::Cmd::Append { tracks: vec![item.path.clone()] });
                app.queue.push(item);
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
                // The playing album moved, so ensure the cover flow's high-res window around it.
                return ensure_hires(app);
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
fn play_items(app: &mut App, items: Vec<QueueItem>) {
    app.send(player::Cmd::SetQueue { tracks: items.iter().map(|i| i.path.clone()).collect(), start: 0 });
    app.queue = items;
    app.current = 0;
    app.anim_pos = 0.0;
    app.view = View::Player;
}

/// Replaces the queue with the album at `ix` and switches to the player view. No-op if the index
/// is out of range -- a rescan can shrink the list under a stale selection.
fn play_album(app: &mut App, ix: usize) {
    let Some(album) = app.albums.get(ix) else { return };
    let items = queue_items(album);
    play_items(app, items);
}

/// Appends the album at `ix` to the queue. No-op if the index is out of range.
fn queue_album(app: &mut App, ix: usize) {
    let Some(album) = app.albums.get(ix) else { return };
    let items = queue_items(album);
    app.send(player::Cmd::Append { tracks: items.iter().map(|i| i.path.clone()).collect() });
    app.queue.extend(items);
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
    let Some(menu) = &mut app.track_menu else { return Task::none() };
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
/// selection actions internally (see `album_grid`), so this covers the global view switching and
/// the player bindings. `repeat` marks auto-repeat from a held key: continuous actions honor it
/// (seek/scrub, walking the queue), while one-shot ones don't (holding Space must not machine-gun
/// play/pause). Alt/Logo always pass through to the window manager.
///
/// Global: Tab / Shift-Tab cycle the view tabs; `l`/`p` jump to Library/Player; Escape returns to
/// the library. With the track menu open (`menu_open`), Escape dismisses it instead and every
/// other binding is suppressed -- it's a modal. In the player: Left/Right seek by [`SEEK_STEP`],
/// Space toggles play/pause, Home restarts the track (or steps back near the start), End steps to
/// the next track, PageUp restarts the album (or steps to the previous one), PageDown jumps to the
/// next album.
pub fn key_to_msg(view: View, menu_open: bool, key: Key, modifiers: Modifiers, repeat: bool) -> Option<Msg> {
    /// How far a single Left/Right tap seeks.
    const SEEK_STEP: Duration = Duration::from_secs(5);
    // Alt/Logo aren't bound anywhere; leave them (and their chords) to the window manager.
    if modifiers.alt() || modifiers.logo() {
        return None;
    }
    let one_shot = |msg| if repeat { None } else { Some(msg) };

    // The track menu is modal: it gets its own bindings and everything else is suppressed.
    // Mirrors the grid's vocabulary one level down -- arrows move the selection, Space queues it,
    // Ctrl+Space (or Enter, which opened the menu) plays it -- and Escape dismisses.
    if menu_open {
        return match (key, modifiers.control()) {
            (Key::Named(Named::Escape), false) => one_shot(Msg::CloseTrackMenu),
            (Key::Named(Named::ArrowUp), false) if modifiers.is_empty() => Some(Msg::MenuMove(MenuDir::Up)),
            (Key::Named(Named::ArrowDown), false) if modifiers.is_empty() => Some(Msg::MenuMove(MenuDir::Down)),
            // One queue per press: holding Space must not machine-gun the queue.
            (Key::Named(Named::Space), false) if modifiers.is_empty() => one_shot(Msg::MenuQueue),
            (Key::Named(Named::Space), true) => one_shot(Msg::MenuPlay),
            (Key::Named(Named::Enter), false) if modifiers.is_empty() => one_shot(Msg::MenuPlay),
            _ => None,
        };
    }

    // View-independent navigation takes precedence over the per-view bindings below.
    match (&key, modifiers.shift(), modifiers.control()) {
        (Key::Named(Named::Tab), false, false) => return one_shot(Msg::Show(view.next())),
        (Key::Named(Named::Tab), true, false) => return one_shot(Msg::Show(view.prev())),
        (Key::Named(Named::Escape), _, false) => return one_shot(Msg::Show(View::Library)),
        (Key::Character(c), false, false) if c.as_str() == "l" => return one_shot(Msg::Show(View::Library)),
        (Key::Character(c), false, false) if c.as_str() == "p" => return one_shot(Msg::Show(View::Player)),
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
