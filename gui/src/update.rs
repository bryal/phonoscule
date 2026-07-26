//! The messages, and how each of them changes the model.

use crate::model::{
    App, Filter, Modal, ModalKind, PICKER_INPUT_ID, PICKER_SCROLL_ID, Picker, PickerSubject, QueueItem, SEARCH_INPUT_ID,
    SORT_SCROLL_ID, ScanState, SortMenu, TRACK_MENU_SCROLL_ID, TrackMenu, View, album_runs, color, current_album_id,
    current_glow, entries, flow_target, glow_blend, hydrate_queue, picker_matches, queue_items, refresh_filter, run_of,
};
use crate::paths;
use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::mouse::ScrollDelta;
use phonoscule::library::{self, Album};
use phonoscule::queue::{self, Grouping, Scope};
use phonoscule::sort::SortOrder;
use phonoscule::{mpris, player, session};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub enum Msg {
    Library(library::ScanEvent),
    /// Time to poll the music directory for changes.
    Rescan,
    Show(View),
    /// Adjust the live UI scale: Ctrl +/- step it, Ctrl+= resets to the configured value.
    Zoom(Zoom),
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
    /// Ctrl+F, from any view: bring up the library and focus the album search field.
    FocusSearch,
    /// A plain character typed in the library outside any field: append it to the album search
    /// and focus the field, so just starting to type searches (the rest of the keys land in the
    /// field directly, once focused).
    SearchTyped(String),
    /// Clear every library filter (the bar's ✕ button, or Ctrl+W), showing the whole library
    /// again. Also unfocuses any filter input and dismisses an open filter picker, so it works
    /// as a full "out of the filtering business" gesture mid-typing.
    ClearFilters,
    /// Play all albums matching the current filter, in their displayed order, replacing the
    /// queue (the filter bar's ▶ button, or Ctrl+Enter in the library).
    PlayAll,
    /// Append all albums matching the current filter, in their displayed order, to the queue
    /// (the filter bar's ＋ button, or Alt+Enter in the library).
    QueueAll,
    /// Open the searchable filter picker for the given subject (a filter-bar chip), focusing its
    /// search field.
    OpenPicker(PickerSubject),
    /// Tab / Shift+Tab (`backwards`), outside a modal: cycle the views -- unless the album search
    /// field holds focus, in which case it moves between the filter inputs instead. Only the
    /// widget tree knows where focus lives, so the decision is deferred to the handler.
    TabPressed {
        backwards: bool,
    },
    /// Move between the filter inputs -- genre picker, artist picker, sort picker, album search,
    /// following the bar -- from the one currently open/focused (Tab / Shift+Tab in a picker, the
    /// sort menu, or the focused search field).
    CycleFilterInput {
        backwards: bool,
    },
    /// Open the sort-order picker (the filter bar's sort chip).
    OpenSort,
    /// Move the sort picker's keyboard selection one option up or down (arrow keys).
    SortMove(MenuDir),
    /// The cursor entered a sort picker row: move the selection there.
    SortHover(usize),
    /// Pick the sort picker row under the mouse (a click).
    SortChoose(usize),
    /// Pick the sort picker's selected row (Enter).
    SortPick,
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
    /// Cycle the repeat mode: off -> track -> album -> playlist -> off (Alt+r, and the
    /// player bar's repeat button).
    CycleRepeat,
    /// Shuffle the queue, in place and visibly, per the grouping (single tracks, or whole albums
    /// with their tracks kept together) and scope (everything behind the playing item, or
    /// literally everything). Alt+s/Alt+z shuffle the other albums/tracks, Ctrl+s/Ctrl+z all; see
    /// `shuffle_queue`.
    Shuffle {
        grouping: Grouping,
        scope: Scope,
    },
    /// Rest playback at an edge of the queue, paused and ready to play (Ctrl+Home / Ctrl+End).
    RestAt(QueueEdge),
    /// Clear the queue entirely (Ctrl+K, or the actions menu): playback stops, the player bar
    /// disappears, and the session restores queueless -- a fresh start.
    ClearQueue,
    /// Move the track menu's keyboard selection one track up or down (arrow keys).
    MenuMove(MenuDir),
    /// The cursor entered a track menu row: move the selection there, so the mouse and the arrow
    /// keys drive the same highlight.
    MenuHover(usize),
    /// Append the track menu's selected track to the queue (Alt+Space), stepping the selection to the
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
    Media(mpris::Control),
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
    /// The mouse wheel turned over the player body (anywhere but the bars): the vertical component
    /// steps the playing-track selection through the queue -- up towards its start, down towards
    /// its end -- and the horizontal component walks whole albums, like PageUp/PageDown (scroll
    /// left restarts or steps back, scroll right jumps to the next album). Carries the raw delta;
    /// notch accounting lives in the handler.
    PlayerScrolled(ScrollDelta),
    /// Set the volume to an absolute factor of 100% (a click or drag on the volume bar).
    SetVolume(f32),
    /// Nudge the volume by a factor of 100% (Up/Down in the player: ±5%; repeats welcome, so a
    /// held key ramps).
    BumpVolume(f32),
    /// The mouse wheel turned over the volume bar: 5% per notch, up is louder.
    VolumeScrolled(ScrollDelta),
    /// The OS mixer reported our volume: the initial reading, an echo of our own set, or an
    /// external change (some mixer applet). Mirror it -- never set back, or we'd loop.
    VolumeChanged(f32),
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

/// The queue edge a [`Msg::RestAt`] rests playback on.
#[derive(Debug, Clone, Copy)]
pub enum QueueEdge {
    First,
    Last,
}

/// A live change to the UI scale factor (see [`App::scale`](crate::model::App)): Ctrl +/- step it,
/// Ctrl+= resets to the configured value.
#[derive(Debug, Clone, Copy)]
pub enum Zoom {
    In,
    Out,
    Reset,
}

impl Zoom {
    /// This step applied to `scale`, clamped to the configured range; `Reset` returns to `baseline`
    /// (the config's `scaling`, itself already in range).
    fn apply(self, scale: f32, baseline: f32) -> f32 {
        /// Multiplicative factor per zoom step -- a perceptually even ~10% either way.
        const STEP: f32 = 1.1;
        match self {
            Zoom::In => (scale * STEP).min(crate::SCALE_MAX),
            Zoom::Out => (scale / STEP).max(crate::SCALE_MIN),
            Zoom::Reset => baseline,
        }
    }
}

pub fn update(app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Library(library::ScanEvent::Album(mut album)) => {
            // Re-scans (and the boot scan reconciling the persisted index) re-report every album,
            // the overwhelming majority unchanged: detect that first and drop the event on the
            // floor. Thousands arrive in a burst on the main thread, and the per-event work below
            // (hydration, sorted re-insert, a full filter refresh) is what used to hitch the UI
            // for a beat on every rescan.
            match app.albums.iter().position(|a| a.id == album.id) {
                Some(ix) => {
                    // `cover`/`accent` are runtime-only: scan events never carry them.
                    let Album { id: _, title, artist, genre, year, cover_id, cover: _, accent: _, tracks } = &app.albums[ix];
                    if *title == album.title
                        && *artist == album.artist
                        && *genre == album.genre
                        && *year == album.year
                        && *cover_id == album.cover_id
                        && *tracks == album.tracks
                    {
                        return Task::none();
                    }
                    app.index_dirty = true;
                    let old = app.albums.remove(ix);
                    // Keep the already-loaded cover art when the cover is unchanged (the scanner
                    // skips re-decoding and re-sending it); the accent follows the cover.
                    if old.cover_id == album.cover_id {
                        album.cover = old.cover;
                        album.accent = old.accent;
                    }
                }
                None => app.index_dirty = true,
            }
            // A queue restored from disk carries only paths (placeholder tags, provisional album
            // keys) until the library reports its albums: hydrate matching items by path. Covers
            // not yet decoded follow by album id through the Cover events.
            hydrate_queue(&mut app.queue, &album);
            // Keep the browser sorted; scan order is nondeterministic (directories complete
            // in parallel).
            let key = |a: &Album| (a.artist.to_lowercase(), a.title.to_lowercase());
            let ix = app.albums.partition_point(|a| key(a) <= key(&album));
            app.albums.insert(ix, *album);
            refresh_filter(app);
        }
        Msg::Library(library::ScanEvent::Cover { albums, art }) => {
            // Only albums whose current cover choice this art satisfies take it: an album can
            // outgrow a queued cover mid-scan (see `ScanEvent::Cover`), and the stale decode must
            // not overwrite the winner -- neither on the album nor on its queue items.
            let accepted: Vec<u64> =
                app.albums.iter().filter(|a| albums.contains(&a.id) && a.cover_id == Some(art.id)).map(|a| a.id).collect();
            if !accepted.is_empty() {
                // The handle for these pixels, made exactly once (see `App::covers`). It wraps the
                // scan's bitmap rather than copying it, so this costs an id and a refcount.
                let pixels = bytes::Bytes::from_owner(art.pixels.clone());
                app.covers.insert(art.id, iced::widget::image::Handle::from_rgba(library::THUMB, library::THUMB, pixels));
            }
            for album in app.albums.iter_mut().filter(|a| accepted.contains(&a.id)) {
                album.cover = Some(art.clone());
                // Freshly computed, so an accent the index got wrong (or an algorithm change)
                // heals on the next index save.
                app.index_dirty |= album.accent != Some(art.accent);
                album.accent = Some(art.accent);
            }
            for item in app.queue.iter_mut().filter(|i| accepted.contains(&i.album_id)) {
                item.cover = Some(art.clone());
                item.accent = Some(color(art.accent));
            }
            // The playing track's cover art may just have arrived -- notably right after boot,
            // when a restored queue's covers all hydrate through the scan. Re-publish it, and
            // (re)fill the cover flow's high-res window that TrackStarted found coverless.
            if app.queue.get(app.current).is_some_and(|item| accepted.contains(&item.album_id)) {
                publish_media(app);
                return ensure_hires(app);
            }
        }
        Msg::Library(library::ScanEvent::Done { album_ids }) => {
            let ids: std::collections::HashSet<u64> = album_ids.into_iter().collect();
            let before = app.albums.len();
            app.albums.retain(|album| ids.contains(&album.id));
            app.index_dirty |= app.albums.len() != before;
            // Retire the handles of covers nothing shows any more: an album gone from the library,
            // or one whose artwork was replaced. The queue counts as a holder -- it outlives the
            // albums it was filled from.
            let live: std::collections::HashSet<u64> = app
                .albums
                .iter()
                .filter_map(|a| a.cover.as_ref())
                .chain(app.queue.iter().filter_map(|i| i.cover.as_ref()))
                .map(|c| c.id)
                .collect();
            app.covers.retain(|id, _| live.contains(id));
            app.scan = ScanState::Complete;
            refresh_filter(app);
            // Persist the settled album list for the next launch's instant grid -- only when this
            // scan actually changed something, so the quiet periodic rescans don't churn the disk.
            if app.index_dirty {
                app.index_dirty = false;
                return Task::future(library::save_index(paths::album_index_file(), &app.albums)).discard();
            }
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
        Msg::Zoom(zoom) => {
            app.scale = zoom.apply(app.scale, app.scaling);
            log::debug!("UI scale -> {:.2}", app.scale);
        }
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
        Msg::FocusSearch => {
            app.view = View::Library;
            return focus_search();
        }
        Msg::SearchTyped(c) => {
            app.filter.search.push_str(&c);
            app.selected = None;
            refresh_filter(app);
            // Focusing puts the text cursor at the end, right behind the character typed here.
            return focus_search();
        }
        Msg::ClearFilters => {
            // Clearing the filters dismisses an open filter picker or the sort picker (Ctrl+W).
            if matches!(app.modal, Some(Modal::Picker(_) | Modal::Sort(_))) {
                app.modal = None;
            }
            app.filter = Filter::default();
            app.selected = None;
            refresh_filter(app);
            // Unfocus whatever filter input held focus: the next keystrokes are bindings (or a
            // fresh type-to-search), not leftovers into a cleared field.
            use iced::advanced::widget;
            return widget::operate(widget::operation::focusable::unfocus());
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
        Msg::OpenPicker(subject) => return open_picker(app, subject),
        Msg::TabPressed { backwards } => {
            // What Tab means depends on where focus lives, which only the widget tree knows:
            // ask it whether the album search is focused, then dispatch. An empty answer (the
            // field isn't rendered -- another view, an empty library) switches views like an
            // unfocused one.
            use iced::advanced::widget;
            let focused = widget::operate(widget::operation::focusable::is_focused(widget::Id::new(SEARCH_INPUT_ID)));
            let show = Msg::Show(if backwards { app.view.prev() } else { app.view.next() });
            return focused.collect().map(move |answer| match answer.first() {
                Some(true) => Msg::CycleFilterInput { backwards },
                _ => show.clone(),
            });
        }
        Msg::CycleFilterInput { backwards } => {
            // The filter inputs, in bar order; Tab walks this ring. The current stop is read from
            // the open modal (or the focused search field, when none is up -- stop 3).
            use PickerSubject::{Artist, Genre};
            let current = match &app.modal {
                Some(Modal::Sort(_)) => 0,
                Some(Modal::Picker(picker)) if picker.subject == Genre => 1,
                Some(Modal::Picker(picker)) if picker.subject == Artist => 2,
                _ => 3, // the album search field
            };
            let stops = 4;
            let next = if backwards { (current + stops - 1) % stops } else { (current + 1) % stops };
            return match next {
                0 => open_sort(app),
                1 => open_picker(app, Genre),
                2 => open_picker(app, Artist),
                _ => {
                    app.modal = None;
                    focus_search()
                }
            };
        }
        Msg::PickerQuery(query) => {
            let Some(Modal::Picker(picker)) = &app.modal else { return Task::none() };
            let matches = picker_matches(app, picker.subject, &query);
            if let Some(Modal::Picker(picker)) = &mut app.modal {
                picker.query = query;
                picker.matches = matches;
                picker.selected = 0;
            }
            return snap_list(PICKER_SCROLL_ID, 0.0);
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
        Msg::OpenSort => return open_sort(app),
        Msg::SortMove(dir) => return sort_step(app, dir),
        // No snap, unlike SortMove: the cursor is already on the row it selected.
        Msg::SortHover(slot) => {
            if let Some(Modal::Sort(menu)) = &mut app.modal {
                menu.selected = slot;
            }
        }
        Msg::SortChoose(slot) => return pick_sort(app, slot),
        Msg::SortPick => {
            if let Some(Modal::Sort(menu)) = &app.modal {
                return pick_sort(app, menu.selected);
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
        Msg::Shuffle { grouping, scope } => return shuffle_queue(app, grouping, scope),
        Msg::RestAt(edge) => {
            if !app.queue.is_empty() {
                app.current = match edge {
                    QueueEdge::First => 0,
                    QueueEdge::Last => app.queue.len() - 1,
                };
                app.anim_pos = flow_target(app);
                // Replacing the queue with itself reopens the track paused: ready to play, its
                // length on the seek bar -- the same way a restored session comes up.
                app.send(player::Cmd::SetQueue {
                    tracks: entries(&app.queue),
                    start: app.current,
                    play: player::PlayState::Paused,
                });
                return save_player(app);
            }
        }
        Msg::ClearQueue => {
            // Invoked from the actions menu: the action dismisses it.
            if matches!(app.modal, Some(Modal::Actions)) {
                app.modal = None;
            }
            if !app.queue.is_empty() {
                app.queue.clear();
                app.current = 0;
                app.anim_pos = 0.0;
                // The engine goes idle (reporting QueueEnded, which also tells the OS we stopped).
                app.send(player::Cmd::SetQueue { tracks: vec![], start: 0, play: player::PlayState::Paused });
                return Task::batch([save_playlist(app), save_player(app)]);
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
                app.media.publish(mpris::Snapshot { meta: None, state: mpris::Playback::Stopped, position: app.pos });
            }
        },
        Msg::Media(control) => match control {
            mpris::Control::Play => match app.play_state {
                player::PlayState::Paused => app.send(player::Cmd::TogglePlayPause),
                player::PlayState::Playing => (),
            },
            // We have no stopped-with-a-track-open state; pausing is the closest thing.
            mpris::Control::Pause | mpris::Control::Stop => match app.play_state {
                player::PlayState::Playing => app.send(player::Cmd::TogglePlayPause),
                player::PlayState::Paused => (),
            },
            mpris::Control::Toggle => app.send(player::Cmd::TogglePlayPause),
            mpris::Control::Next => app.send(player::Cmd::Next),
            mpris::Control::Prev => app.send(player::Cmd::Prev),
            mpris::Control::Seek(offset) => {
                let dir = if offset >= 0 { SeekDir::Forward } else { SeekDir::Backward };
                do_seek(app, Seek::By(dir, Duration::from_micros(offset.unsigned_abs())));
            }
            mpris::Control::SetPosition(t) => app.send(player::Cmd::Seek(t)),
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
        Msg::PlayerScrolled(delta) => {
            let (_, vertical) = scroll_notches(delta);
            app.list_scroll += vertical;
            let steps = app.list_scroll.trunc();
            app.list_scroll -= steps;
            // Scrolling up moves the selection towards the queue's start, like a list. Jumps
            // rather than the transport commands: Cmd::Prev restarts the track when far enough
            // in, and Cmd::Next past the last track would end the queue -- a selection clamps.
            if steps != 0.0 && !app.queue.is_empty() {
                let target = (app.current as f32 - steps).clamp(0.0, (app.queue.len() - 1) as f32) as usize;
                if target != app.current {
                    app.send(player::Cmd::JumpTo(target));
                }
            }
            scroll_albums(app, delta);
        }
        Msg::SetVolume(volume) => set_volume(app, volume),
        Msg::BumpVolume(delta) => {
            if let Some(volume) = app.volume {
                set_volume(app, volume + delta);
            }
        }
        Msg::VolumeScrolled(delta) => {
            let (_, vertical) = scroll_notches(delta);
            if let Some(volume) = app.volume {
                set_volume(app, volume + 0.05 * vertical);
            }
        }
        Msg::VolumeChanged(volume) => {
            // While our own set is in flight, ignore readings until the mixer reaches (about)
            // the requested value -- see `App::pending_volume`. The tolerance is generous
            // against the mixer's quantization, narrow against a real 5% step.
            match app.pending_volume {
                Some(target) if (volume - target).abs() > 0.005 => (),
                _ => {
                    app.pending_volume = None;
                    app.volume = Some(volume);
                }
            }
        }
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
    Task::future(session::save_playlist(paths::playlist_file(), session::SavedPlaylist::new(tracks))).discard()
}

/// Snapshots the session state around the queue (current track, repeat mode, sort order) to disk;
/// returned by everything that changes any of them. Split from [`save_playlist`]: these changes
/// are frequent and needn't rewrite the whole track list.
fn save_player(app: &App) -> Task<Msg> {
    let saved = session::SavedPlayer::new(app.current, app.repeat, app.sort);
    Task::future(session::save_player(paths::player_file(), saved)).discard()
}

/// Shuffles the queue in place, visibly: the reordering IS the new playlist (persisted like any
/// other queue change, so a restart resumes the same order), and the cover flow snaps to the
/// cursor's new position rather than sweeping. See [`Scope`] for what moves and what keeps
/// playing: Alt+s/Alt+z shuffle the others, Ctrl+s/Ctrl+z everything -- always, regardless of
/// playback state.
fn shuffle_queue(app: &mut App, grouping: Grouping, scope: Scope) -> Task<Msg> {
    if app.queue.is_empty() {
        return Task::none();
    }
    // Shuffling from the actions menu: the action dismisses it.
    if matches!(app.modal, Some(Modal::Actions)) {
        app.modal = None;
    }

    let albums: Vec<u64> = app.queue.iter().map(|item| item.album_id).collect();
    let order = queue::shuffle(&albums, app.current, grouping, scope, queue::seed());

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

/// Resolves a grid message's index (into the filtered list) to a real album index, or `None` for
/// a stale cell (the filter can change under an in-flight message).
fn shown_album(app: &App, cell: usize) -> Option<usize> {
    app.filtered.get(cell).copied()
}

/// Focuses the filter bar's album search field (a no-op while the field isn't rendered, i.e. an
/// empty library).
fn focus_search() -> Task<Msg> {
    use iced::advanced::widget;
    widget::operate(widget::operation::focusable::focus(widget::Id::new(SEARCH_INPUT_ID)))
}

/// Opens the filter picker for `subject` and focuses its search field, so typing starts
/// filtering immediately.
fn open_picker(app: &mut App, subject: PickerSubject) -> Task<Msg> {
    let matches = picker_matches(app, subject, "");
    app.modal = Some(Modal::Picker(Picker { subject, query: String::new(), matches, selected: 0 }));
    use iced::advanced::widget;
    widget::operate(widget::operation::focusable::focus(widget::Id::new(PICKER_INPUT_ID)))
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
    snap_list(PICKER_SCROLL_ID, fraction)
}

/// Opens the sort-order picker, its selection on the current order. No text field to focus (the
/// option set is fixed), so unfocus any filter input, so the modal's keys reach it (see
/// `key_to_msg`).
fn open_sort(app: &mut App) -> Task<Msg> {
    let selected = SortOrder::ALL.iter().position(|&order| order == app.sort).unwrap_or(0);
    app.modal = Some(Modal::Sort(SortMenu { selected }));
    use iced::advanced::widget;
    widget::operate(widget::operation::focusable::unfocus())
}

/// Applies the sort picker's slot `slot`: sets the order, closes the picker, drops the grid
/// selection (it indexes the filtered list, which is about to reorder), refreshes the grid, and
/// persists the new order (it survives restarts).
fn pick_sort(app: &mut App, slot: usize) -> Task<Msg> {
    let Some(&order) = SortOrder::ALL.get(slot) else { return Task::none() };
    app.sort = order;
    app.modal = None;
    app.selected = None;
    refresh_filter(app);
    save_player(app)
}

/// Moves the sort picker's keyboard selection one option, clamped, and snaps the list to it.
fn sort_step(app: &mut App, dir: MenuDir) -> Task<Msg> {
    let Some(Modal::Sort(menu)) = &mut app.modal else { return Task::none() };
    let last = SortOrder::ALL.len() - 1;
    menu.selected = match dir {
        MenuDir::Up => menu.selected.saturating_sub(1),
        MenuDir::Down => (menu.selected + 1).min(last),
    };
    snap_list(SORT_SCROLL_ID, menu.selected as f32 / last as f32)
}

/// Snaps the scrollable with the given id to the relative vertical position `fraction`.
fn snap_list(id: &'static str, fraction: f32) -> Task<Msg> {
    use iced::advanced::widget;
    let to = widget::operation::scrollable::RelativeOffset { x: None, y: Some(fraction) };
    widget::operate(widget::operation::scrollable::snap_to(widget::Id::new(id), to))
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

/// The wheel delta as (horizontal, vertical) notches: line deltas are notches already; trackpad
/// pixel deltas convert at a typical line height. Fractions accumulate across events (a trackpad
/// flick arrives as many small deltas), one step firing per whole notch.
fn scroll_notches(delta: ScrollDelta) -> (f32, f32) {
    match delta {
        ScrollDelta::Lines { x, y } => (x, y),
        ScrollDelta::Pixels { x, y } => (x / 20.0, y / 20.0),
    }
}

/// Requests `volume` (clamped) from the OS mixer and mirrors it optimistically, so the bar
/// tracks a drag or a wheel burst instantly while the mixer's echo trails behind.
fn set_volume(app: &mut App, volume: f32) {
    let volume = volume.clamp(0.0, 1.0);
    app.volume = Some(volume);
    app.pending_volume = Some(volume);
    app.mixer.set(volume);
}

/// Applies a wheel delta's horizontal component to the queue's albums: scroll left restarts or
/// steps back like PageUp, scroll right jumps to the next album like PageDown. At most one album
/// step per event -- the album helpers read `app.current`, which only advances once the engine
/// reports the jump, so a burst must not stack stale jumps.
fn scroll_albums(app: &mut App, delta: ScrollDelta) {
    let (horizontal, _) = scroll_notches(delta);
    app.album_scroll += horizontal;
    let steps = app.album_scroll.trunc();
    app.album_scroll -= steps;
    if steps > 0.0 {
        prev_album(app);
    } else if steps < 0.0 {
        next_album(app);
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

/// Options for a periodic re-scan: skip re-decoding all the cover art we already hold. No cover
/// priority -- the covers that matter are long loaded by now.
fn rescan_options(app: &App) -> library::ScanOptions {
    library::ScanOptions {
        root: app.conf.music_dir.clone(),
        priority: vec![],
        known_covers: app.albums.iter().filter_map(|a| a.cover.as_ref().map(|c| c.id)).collect(),
        cache_file: paths::tag_cache_file(),
        covers_dir: paths::covers_dir(),
    }
}

/// Publishes the current now-playing state to the OS media integration. Fire-and-forget: the
/// media worker coalesces a burst of these down to the latest and rate-limits the actual pushes,
/// so callers need not throttle.
fn publish_media(app: &App) {
    let meta = app.queue.get(app.current).map(|item| mpris::Meta {
        title: item.title.clone(),
        album: item.album.clone(),
        artist: item.artist.clone(),
        // Absolute file:// URL so the OS can load the cover regardless of our working directory.
        cover_url: item.cover.as_ref().and_then(|c| url::Url::from_file_path(&*c.file).ok()).map(String::from),
        duration: app.len,
    });
    let state = match app.play_state {
        player::PlayState::Playing => mpris::Playback::Playing,
        player::PlayState::Paused => mpris::Playback::Paused,
    };
    app.media.publish(mpris::Snapshot { meta, state, position: app.pos });
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
/// Space must not machine-gun play/pause). Alt/Logo pass through to the window manager, except
/// Alt+Space: that is the queue-the-selection binding, freeing bare Space for the toggle.
///
/// Global: Space toggles play/pause (too central a function to belong to one view); Tab /
/// Shift-Tab cycle the view tabs -- unless a filter input holds focus (the album search, or an
/// open filter picker), in which case they move between the filter inputs instead, following the
/// bar; Escape returns to the library; Alt+r cycles the repeat mode;
/// Alt+s/Alt+z shuffle the other albums/tracks in behind
/// the playing one (Ctrl instead of Alt shuffles literally everything); Ctrl+Home / Ctrl+End rest
/// playback at the queue's first / last track, paused. With a modal open, Escape dismisses it, the track menu gets its
/// own bindings, and everything else is suppressed. Ctrl+F brings up the library with the album
/// search focused; Ctrl+W clears every filter (unfocusing its inputs). In the library: Ctrl+Enter plays and
/// Alt+Enter queues everything the filter shows, and typing any plain character starts the album
/// search. In the player: Left/Right seek by
/// [`SEEK_STEP`], Up/Down nudge the volume by 5%, Home
/// restarts the track (or steps back near the start), End steps to the next track, PageUp restarts
/// the album (or steps to the previous one), PageDown jumps to the next album.
pub fn key_to_msg(
    view: View,
    modal: Option<ModalKind>,
    key: Key,
    modified_key: Key,
    modifiers: Modifiers,
    repeat: bool,
) -> Option<Msg> {
    /// How far a single Left/Right tap seeks.
    const SEEK_STEP: Duration = Duration::from_secs(5);
    // Alt/Logo chords belong to the window manager -- all but the few we bind: Alt+Space, the
    // Alt+s/Alt+z shuffles, and the Alt+r repeat cycle. Letters and typing keys carry no bare
    // bindings, so plain typing stays free for a future type-to-search.
    let alt_ours = modifiers.alt()
        && (matches!(key, Key::Named(Named::Space) | Key::Named(Named::Enter))
            || matches!(&key, Key::Character(c) if matches!(c.as_str(), "s" | "z" | "r")));
    if (modifiers.alt() && !alt_ours) || modifiers.logo() {
        return None;
    }
    let one_shot = |msg| if repeat { None } else { Some(msg) };

    match modal {
        // The track menu is modal: it gets its own bindings and everything else is suppressed.
        // Mirrors the grid's vocabulary one level down -- arrows move the selection, Alt+Space
        // queues it, Ctrl+Space (or Enter, which opened the menu) plays it -- and Escape
        // dismisses. Bare Space stays the global play/pause toggle even here.
        Some(ModalKind::Tracks) => {
            return match (key, modifiers.control()) {
                (Key::Named(Named::Escape), false) => one_shot(Msg::CloseModal),
                (Key::Named(Named::ArrowUp), false) if modifiers.is_empty() => Some(Msg::MenuMove(MenuDir::Up)),
                (Key::Named(Named::ArrowDown), false) if modifiers.is_empty() => Some(Msg::MenuMove(MenuDir::Down)),
                // One queue per press: holding Alt+Space must not machine-gun the queue.
                (Key::Named(Named::Space), false) if modifiers.alt() => one_shot(Msg::MenuQueue),
                (Key::Named(Named::Space), false) if modifiers.is_empty() => one_shot(Msg::Toggle),
                (Key::Named(Named::Space), true) => one_shot(Msg::MenuPlay),
                (Key::Named(Named::Enter), false) if modifiers.is_empty() => one_shot(Msg::MenuPlay),
                _ => None,
            };
        }
        // The actions menu: Escape dismisses; its actions keep their global keys (the entries
        // display them as hints, and the handlers dismiss the menu themselves), and so does the
        // play/pause toggle.
        Some(ModalKind::Actions) => {
            return match (key, modifiers.control()) {
                (Key::Named(Named::Escape), false) => one_shot(Msg::CloseModal),
                (Key::Named(Named::Space), false) if modifiers.is_empty() => one_shot(Msg::Toggle),
                (Key::Character(c), false) if modifiers.alt() && c.as_str() == "r" => one_shot(Msg::CycleRepeat),
                // Exactly one modifier, like the global bindings: Alt = others, Ctrl = all.
                (Key::Character(c), ctrl) if modifiers.alt() != ctrl && c.as_str() == "s" => {
                    one_shot(Msg::Shuffle { grouping: Grouping::Albums, scope: if ctrl { Scope::All } else { Scope::Others } })
                }
                (Key::Character(c), ctrl) if modifiers.alt() != ctrl && c.as_str() == "z" => {
                    one_shot(Msg::Shuffle { grouping: Grouping::Tracks, scope: if ctrl { Scope::All } else { Scope::Others } })
                }
                (Key::Character(c), true) if c.as_str() == "k" => one_shot(Msg::ClearQueue),
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
                // Clearing every filter includes the one being picked: the handler dismisses us.
                Key::Character(c) if modifiers.control() && !modifiers.alt() && c.as_str() == "w" => {
                    one_shot(Msg::ClearFilters)
                }
                // Tab moves along the filter inputs, not between the views, while one is open.
                Key::Named(Named::Tab) if !modifiers.control() && !modifiers.alt() => {
                    one_shot(Msg::CycleFilterInput { backwards: modifiers.shift() })
                }
                _ => None,
            };
        }
        // The sort picker: a fixed list, so no search field -- arrows move, Enter picks, and Tab
        // walks the filter inputs like the other pickers. Escape and Ctrl+W dismiss.
        Some(ModalKind::Sort) => {
            return match key {
                Key::Named(Named::Escape) if !modifiers.control() => one_shot(Msg::CloseModal),
                Key::Named(Named::ArrowUp) if modifiers.is_empty() => Some(Msg::SortMove(MenuDir::Up)),
                Key::Named(Named::ArrowDown) if modifiers.is_empty() => Some(Msg::SortMove(MenuDir::Down)),
                Key::Named(Named::Enter) if modifiers.is_empty() => one_shot(Msg::SortPick),
                Key::Character(c) if modifiers.control() && !modifiers.alt() && c.as_str() == "w" => {
                    one_shot(Msg::ClearFilters)
                }
                Key::Named(Named::Tab) if !modifiers.control() && !modifiers.alt() => {
                    one_shot(Msg::CycleFilterInput { backwards: modifiers.shift() })
                }
                _ => None,
            };
        }
        None => {}
    }

    // Live UI zoom (see `App::scale`), matched on the resolved character `modified_key` -- so `+`,
    // `-` and `=` land as actually typed on any layout, unlike the base `key` the bindings below
    // read. Ctrl+plus zooms in, Ctrl+minus out, Ctrl+= resets. View-independent, so it works in the
    // player too (which drops its own Ctrl chords); Alt combinations were already filtered above.
    if modifiers.control()
        && let Key::Character(c) = &modified_key
    {
        match c.as_str() {
            "+" => return one_shot(Msg::Zoom(Zoom::In)),
            "-" => return one_shot(Msg::Zoom(Zoom::Out)),
            "=" => return one_shot(Msg::Zoom(Zoom::Reset)),
            _ => {}
        }
    }

    // View-independent navigation takes precedence over the per-view bindings below. Bare Space
    // toggles play/pause in every view (an uncaptured Alt+Space -- no grid selection -- is a
    // missed enqueue, not a toggle request).
    match (&key, modifiers.shift(), modifiers.control()) {
        (Key::Named(Named::Space), false, false) if modifiers.is_empty() => return one_shot(Msg::Toggle),
        (Key::Named(Named::Tab), backwards, false) => return one_shot(Msg::TabPressed { backwards }),
        (Key::Named(Named::Escape), _, false) => return one_shot(Msg::Show(View::Library)),
        (Key::Character(c), false, false) if modifiers.alt() && c.as_str() == "r" => return one_shot(Msg::CycleRepeat),
        // Exactly one modifier: Alt shuffles the others, Ctrl shuffles all -- Ctrl+Alt is nothing.
        (Key::Character(c), false, ctrl) if modifiers.alt() != ctrl && c.as_str() == "s" => {
            return one_shot(Msg::Shuffle { grouping: Grouping::Albums, scope: if ctrl { Scope::All } else { Scope::Others } });
        }
        (Key::Character(c), false, ctrl) if modifiers.alt() != ctrl && c.as_str() == "z" => {
            return one_shot(Msg::Shuffle { grouping: Grouping::Tracks, scope: if ctrl { Scope::All } else { Scope::Others } });
        }
        (Key::Named(Named::Home), false, true) => return one_shot(Msg::RestAt(QueueEdge::First)),
        (Key::Named(Named::End), false, true) => return one_shot(Msg::RestAt(QueueEdge::Last)),
        (Key::Character(c), false, true) if c.as_str() == "k" => return one_shot(Msg::ClearQueue),
        (Key::Character(c), false, true) if c.as_str() == "f" => return one_shot(Msg::FocusSearch),
        (Key::Character(c), false, true) if c.as_str() == "w" => return one_shot(Msg::ClearFilters),
        _ => {}
    }

    match view {
        // Play or queue every album the current filter lets through (the whole library when it
        // is empty), in displayed order -- the keyboard forms of the filter bar's ▶/＋ buttons.
        View::Library => match (key, modifiers.alt(), modifiers.control()) {
            (Key::Named(Named::Enter), false, true) => one_shot(Msg::PlayAll),
            (Key::Named(Named::Enter), true, false) => one_shot(Msg::QueueAll),
            // Starting to type searches: the first character rides along to the field, which
            // takes over (as the focused widget) for the rest. Letters can carry bindings only
            // with a modifier, exactly so that this works. Repeats welcome -- held keys repeat
            // in a text field.
            (Key::Character(c), false, false) => Some(Msg::SearchTyped(c.to_string())),
            _ => None,
        },
        // The player view binds no Ctrl chords of its own.
        View::Player if modifiers.control() => None,
        View::Player => match key {
            Key::Named(Named::ArrowLeft) => Some(Msg::Seek(Seek::By(SeekDir::Backward, SEEK_STEP))),
            Key::Named(Named::ArrowRight) => Some(Msg::Seek(Seek::By(SeekDir::Forward, SEEK_STEP))),
            Key::Named(Named::ArrowUp) => Some(Msg::BumpVolume(0.05)),
            Key::Named(Named::ArrowDown) => Some(Msg::BumpVolume(-0.05)),
            Key::Named(Named::Home) => Some(Msg::PrevOrRestart { repeat }),
            Key::Named(Named::End) => Some(Msg::Next { repeat }),
            // One album per press: a held key mustn't fly through the whole queue.
            Key::Named(Named::PageUp) => one_shot(Msg::PrevAlbum),
            Key::Named(Named::PageDown) => one_shot(Msg::NextAlbum),
            _ => None,
        },
    }
}
