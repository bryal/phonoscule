//! The application model: all state, and the queue/album-run bookkeeping around it.

use crate::update::Msg;
use futures::StreamExt;
use iced::Task;
use phonoscule_gui::conf::Conf;
use phonoscule_gui::library::{self, Album};
use phonoscule_gui::{media, player, playlist, watcher};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    Library,
    Player,
}

impl View {
    /// The next/previous view in tab order, wrapping around -- what Tab / Shift-Tab select.
    pub fn next(self) -> Self {
        match self {
            View::Library => View::Player,
            View::Player => View::Library,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            View::Library => View::Player,
            View::Player => View::Library,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanState {
    Scanning,
    Complete,
}

/// A modal open over the current view; at most one at a time, by construction.
#[derive(Debug, Clone)]
pub enum Modal {
    /// An album's track menu (see [`TrackMenu`]).
    Tracks(TrackMenu),
    /// The player actions menu (shuffle, and eventually export and friends), opened from the
    /// player bar's ellipsis button.
    Actions,
    /// A searchable filter picker (see [`Picker`]), opened from the library's filter bar.
    Picker(Picker),
}

impl Modal {
    /// The payload-free kind, for contexts that only care which modal is up (the keyboard
    /// subscription identity, which must not churn as a menu's selection moves).
    pub fn kind(&self) -> ModalKind {
        match self {
            Modal::Tracks(_) => ModalKind::Tracks,
            Modal::Actions => ModalKind::Actions,
            Modal::Picker(_) => ModalKind::Picker,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModalKind {
    Tracks,
    Actions,
    Picker,
}

/// What a [`Picker`] picks a filter value for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerSubject {
    Genre,
    Artist,
}

/// An open filter picker: a search query over the subject's values, the values currently matching
/// it (recomputed as the query changes, ranked like the album search), and the keyboard selection
/// as a slot index -- slot 0 is the standing "(all)" entry that clears the filter, slot `n + 1` is
/// `matches[n]`.
#[derive(Debug, Clone)]
pub struct Picker {
    pub subject: PickerSubject,
    pub query: String,
    pub matches: Vec<String>,
    pub selected: usize,
}

/// Widget ids of the picker's search input (so opening it can focus the field) and its list's
/// scrollable (so keyboard navigation can snap it to the selection).
pub const PICKER_INPUT_ID: &str = "picker-input";
pub const PICKER_SCROLL_ID: &str = "picker-list";

/// The open track menu: which album (an index into [`App::albums`]) and which of its tracks the
/// keyboard selection sits on (Up/Down move it; Alt+Space queues, Ctrl+Space or Enter plays).
#[derive(Debug, Clone, Copy)]
pub struct TrackMenu {
    pub album: usize,
    pub selected: usize,
}

/// Widget id of the track menu's scrollable, so keyboard navigation can snap it to the selection.
pub const TRACK_MENU_SCROLL_ID: &str = "track-menu";

#[derive(Debug, Clone)]
pub struct QueueItem {
    pub path: PathBuf,
    pub album_id: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover: Option<library::CoverArt>,
    /// The album's accent color, carried independently of the cover pixels (the index knows it at
    /// boot, long before thumbnails load) so the backdrop glow lights up immediately.
    pub accent: Option<iced::Color>,
}

pub struct App {
    pub engine: player::Engine,
    /// Handle to the OS media integration. State changes are published to it (see
    /// [`publish_media`](crate::update)); a worker coalesces and pushes them, so the update loop
    /// holds no rate-limiting state of its own.
    pub media: media::Media,
    pub watcher: watcher::Watcher,
    pub conf: Conf,
    pub scan: ScanState,
    pub albums: Vec<Album>,
    /// Whether `albums` has drifted from the persisted album index since it was last written:
    /// set by scan events that actually change something, cleared when `Done` saves the index.
    /// Keeps the quiet periodic rescans from rewriting megabytes every time.
    pub index_dirty: bool,
    /// The library filter: which albums the grid shows, of everything in `albums`.
    pub filter: Filter,
    /// The filtered view of `albums` the grid displays: indices into it, in display order
    /// (alphabetical like `albums`; a search re-ranks by match quality). Derived state -- kept
    /// fresh by [`refresh_filter`] on every filter change and scan event. Grid messages carry
    /// indices into THIS list.
    pub filtered: Vec<usize>,
    pub view: View,
    /// The library grid's selection, externalized from the grid widget (whose own state drops
    /// with the view) so it survives switching views. Purely a persistence mirror: the widget
    /// syncs from it each render and reports changes back (see `AlbumGrid::selected`); nothing
    /// here reads it.
    pub selected: Option<usize>,
    /// The modal open over the current view, if any: an album's track menu, or the player
    /// actions menu.
    pub modal: Option<Modal>,
    pub queue: Vec<QueueItem>,
    pub current: usize,
    /// The repeat mode, mirrored here for the UI and persistence; the engine holds its own copy
    /// (synced via [`player::Cmd::SetRepeat`]) since auto-advance happens there.
    pub repeat: player::Repeat,
    pub play_state: player::PlayState,
    pub pos: Duration,
    pub len: Option<Duration>,
    /// Seek-bar fraction while the user is dragging it.
    pub seek_drag: Option<f32>,
    /// The position last requested of the player, held until playback actually reaches it. Lets a
    /// burst of relative seeks (a held arrow key) accumulate from the requested position rather
    /// than the round-trip-lagged reported one, and stops stale in-flight reports from yanking the
    /// bar back to the pre-seek spot.
    pub pending_seek: Option<Duration>,
    /// When the last track skip from a held Home/End key fired, used to rate-limit its auto-repeat
    /// (see `skip_ready`). `None` until the first such skip.
    pub last_skip: Option<Instant>,
    /// When the current held Home/End press began, so its auto-repeat can accelerate the longer
    /// it's held (see `skip_interval`). Reset on each fresh press.
    pub hold_start: Option<Instant>,
    /// Fractional wheel notches accumulated over the player's track list, so trackpad pixel
    /// deltas add up to whole selection steps (see `Msg::TrackListScrolled`).
    pub list_scroll: f32,
    /// Like `list_scroll`, for the horizontal wheel axis: whole notches walk the queue's albums
    /// (see `Msg::PlayerScrolled`).
    pub album_scroll: f32,
    /// High-resolution cover art (FULL² RGBA) for the now-playing cover flow. The flow keeps a
    /// small window around `current` resident (see `ensure_hires`), but this cache outlives that
    /// window: it retains recently-played covers under an LRU bound, so hopping back to an album
    /// played moments ago shows its full-res cover instantly instead of decoding again.
    pub hires: HiResCache,
    /// Animated Cover Flow position, chasing `current`.
    pub anim_pos: f32,
    /// The backdrop glow transitions between two album states: `glow_from` -> `glow_to` as
    /// `glow_p` runs 0 -> 1 (1 = settled on `glow_to`). `glow_album` is the album `glow_to`
    /// represents, so a change is detected when the current album differs.
    pub glow_from: GlowState,
    pub glow_to: GlowState,
    pub glow_album: u64,
    pub glow_p: f32,
    pub last_frame: Instant,
}

/// A backdrop glow: its color and its center as a fraction of the viewport. Album changes
/// animate one of these into another (see [`glow_blend`]).
#[derive(Clone, Copy, PartialEq)]
pub struct GlowState {
    pub color: iced::Color,
    pub center: (f32, f32),
}

/// How many decoded high-res covers [`HiResCache`] keeps. At FULL² RGBA (~3 MiB each) this bounds
/// its footprint near 240 MiB -- and only a session that plays that many *distinct* albums reaches
/// it; a typical one holds far fewer. Enough to blanket a favorite genre or playlist, so bouncing
/// among its albums never re-decodes a cover.
pub const HIRES_CAP: usize = 80;

/// A least-recently-used cache of decoded high-res covers (FULL² RGBA), shared across every album
/// that plays. Demand-driven in the style of a query-compilation cache: callers [`query`] a cover
/// and the cache fetches-or-decodes behind the scenes, memoizing the result; there is no manual
/// get-then-insert. It holds the covers around the current album and a good many recently-played
/// others, up to [`HIRES_CAP`], so revisiting an album is instant. Bitmaps are `Arc<[u8]>`, so a
/// cached cover and the cover flow's copy of it are one allocation, not two.
///
/// [`query`]: HiResCache::query
pub struct HiResCache {
    /// `id -> (pixels, tick when last used)`. A monotonic tick, not a wall clock, orders entries
    /// for eviction -- the update loop's state transitions are pure and have no clock to read.
    entries: HashMap<u64, (Arc<[u8]>, u64)>,
    /// Cover ids whose decode is in flight, so a repeated [`query`](Self::query) doesn't launch a
    /// second decode of the same cover.
    pending: HashSet<u64>,
    tick: u64,
}

impl HiResCache {
    pub fn new() -> Self {
        HiResCache { entries: HashMap::new(), pending: HashSet::new(), tick: 0 }
    }

    /// Demands the high-res cover for `id`, decoded from `file`. Query-compilation style: if it's
    /// already cached this just promotes it in the LRU (the view reads the pixels via [`peek`]);
    /// otherwise the decode runs behind the scenes and lands back through [`complete`], which
    /// memoizes it. Deduplicated -- a cover already resident or already decoding yields
    /// [`Task::none`] -- so callers can query their whole window every album move without tracking
    /// what's loaded or in flight.
    ///
    /// [`peek`]: HiResCache::peek
    /// [`complete`]: HiResCache::complete
    pub fn query(&mut self, id: u64, file: Arc<PathBuf>) -> Task<Msg> {
        // Resident: promote and done. Already decoding: let the in-flight decode land. (`touch`
        // short-circuits the `insert`, so a resident cover is never marked pending.)
        if self.touch(id) || !self.pending.insert(id) {
            return Task::none();
        }
        let file = (*file).clone();
        Task::perform(library::full_res(file), move |pixels| Msg::HiResLoaded { id, pixels })
    }

    /// Absorbs the result of a [`query`](Self::query)'s decode: clears the in-flight mark and, on
    /// success, memoizes the cover (evicting the least-recently-used if now over [`HIRES_CAP`]). A
    /// failed decode (`None`) simply leaves the cover on its thumbnail.
    pub fn complete(&mut self, id: u64, pixels: Option<Arc<[u8]>>) {
        self.pending.remove(&id);
        let Some(pixels) = pixels else { return };
        self.tick += 1;
        self.entries.insert(id, (pixels, self.tick));
        while self.entries.len() > HIRES_CAP {
            // The cap is small and inserts are infrequent (one per album entering the window), so a
            // linear scan for the oldest beats maintaining a separate ordered index.
            let Some((&oldest, _)) = self.entries.iter().min_by_key(|(_, (_, used))| *used) else { break };
            self.entries.remove(&oldest);
        }
    }

    /// The cover for `id`, if resident, as a cheap ref-counted clone. A non-promoting probe for the
    /// view, which calls it every frame and must render *something* (the thumbnail) when a cover
    /// isn't loaded yet -- promotion is [`query`](Self::query)'s job, once per album move.
    pub fn peek(&self, id: u64) -> Option<Arc<[u8]>> {
        self.entries.get(&id).map(|(pixels, _)| pixels.clone())
    }

    /// Marks a resident cover as just-used, moving it to the front of the eviction order, and
    /// reports whether it was resident at all. Keeping the on-screen window touched on every query
    /// keeps eviction falling on colder, off-screen covers.
    fn touch(&mut self, id: u64) -> bool {
        if let Some((_, used)) = self.entries.get_mut(&id) {
            self.tick += 1;
            *used = self.tick;
            true
        } else {
            false
        }
    }
}

impl Default for HiResCache {
    fn default() -> Self {
        Self::new()
    }
}

pub fn boot(conf: Conf, restored: playlist::Restored, index: Vec<Album>) -> impl Fn() -> (App, Task<Msg>) {
    move || {
        let (media, media_worker) = media::start();
        let mut app = App {
            engine: player::start(),
            media,
            watcher: watcher::start(&conf.music_dir),
            conf: conf.clone(),
            scan: ScanState::Scanning,
            albums: index.clone(),
            index_dirty: false,
            filter: Filter::default(),
            filtered: vec![],
            view: View::Library,
            selected: None,
            modal: None,
            queue: vec![],
            current: 0,
            repeat: restored.repeat,
            play_state: player::PlayState::Paused,
            pos: Duration::ZERO,
            len: None,
            seek_drag: None,
            pending_seek: None,
            last_skip: None,
            hold_start: None,
            list_scroll: 0.0,
            album_scroll: 0.0,
            hires: HiResCache::new(),
            anim_pos: 0.0,
            glow_from: GlowState { color: iced::Color::BLACK, center: glow_center(0) },
            glow_to: GlowState { color: iced::Color::BLACK, center: glow_center(0) },
            glow_album: 0,
            glow_p: 1.0,
            last_frame: Instant::now(),
        };
        // Restore the previous session's queue, paused at its current track: the engine opens the
        // track (reporting its length for the seek bar) and waits. Only paths were persisted --
        // items start as placeholders (the file stem for a title, the parent directory hashed as a
        // provisional album key) and the scan below hydrates real tags and album ids by path, with
        // covers following by album id. The engine keeps the provisional keys until the next queue
        // command; they group identically except where the directory layout and the tags disagree
        // (a directory pooling several albums, or an album spread over several directories).
        app.queue = restored
            .tracks
            .iter()
            .map(|path| QueueItem {
                title: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                artist: String::new(),
                album: String::new(),
                album_id: dir_key(path),
                cover: None,
                accent: None,
                path: path.clone(),
            })
            .collect();
        // The persisted album index makes the whole library visible immediately (covers stream in
        // from the thumbnail cache); the scan below reconciles it like any rescan. It also
        // hydrates the restored queue right here -- real tags and album ids from the first frame,
        // and the engine gets real album grouping keys below instead of the provisional ones.
        for album in &app.albums {
            hydrate_queue(&mut app.queue, album);
        }
        refresh_filter(&mut app);
        app.current = restored.current;
        // Rest the cover flow on the current album rather than sweeping to it from the far end.
        app.anim_pos = flow_target(&app);
        app.send(player::Cmd::SetRepeat(app.repeat));
        if !app.queue.is_empty() {
            app.send(player::Cmd::SetQueue {
                tracks: entries(&app.queue),
                start: app.current,
                play: player::PlayState::Paused,
            });
            // A restored session opens where it left off: on the player, ready to resume.
            app.view = View::Player;
        }
        let options = library::ScanOptions {
            root: conf.music_dir.clone(),
            // Dress the restored queue's covers first, outward from the playing album: they're
            // what a session booting into the player is looking at.
            priority: cover_priority(&app.queue, app.current),
            known_covers: Default::default(),
            cache_file: library::default_cache_file(),
            covers_dir: library::default_covers_dir(),
        };
        let scan = Task::run(library::scan(options), Msg::Library);
        // Run the media worker for the whole session, on iced's executor; it pushes to the OS and
        // yields no messages, so wrap its never-ending future as a stream that produces nothing.
        let media = Task::stream(futures::stream::once(media_worker.run()).filter_map(|()| async { None }));
        (app, Task::batch([scan, media]))
    }
}

impl App {
    pub fn send(&self, cmd: player::Cmd) {
        // The command channel is unbounded, so this only fails when the engine is gone.
        if self.engine.cmd.try_send(cmd).is_err() {
            log::error!("player engine is gone");
        }
    }

    /// The open track menu, if that is the modal that's up.
    pub fn track_menu(&self) -> Option<TrackMenu> {
        match &self.modal {
            Some(Modal::Tracks(menu)) => Some(*menu),
            _ => None,
        }
    }
}

/// The library filter's inputs: an exact genre and artist (each picked from a searchable picker)
/// and a fuzzy album-title search, ANDed together.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub genre: Option<String>,
    pub artist: Option<String>,
    pub search: String,
}

impl Filter {
    /// Whether the filter lets everything through (nothing to clear).
    pub fn is_empty(&self) -> bool {
        self.genre.is_none() && self.artist.is_none() && self.search.is_empty()
    }
}

/// Recomputes [`App::filtered`] from the filter inputs: albums by the picked artist (if any)
/// whose titles contain every whitespace-split word of the search (case-insensitively). Album
/// order (alphabetical) is kept, except that a non-empty search re-ranks by [`search_rank`] --
/// the sort is stable, so ties stay alphabetical.
pub fn refresh_filter(app: &mut App) {
    let mut scored: Vec<(usize, usize)> = app
        .albums
        .iter()
        .enumerate()
        .filter(|(_, album)| app.filter.genre.as_ref().is_none_or(|genre| album.genre == *genre))
        .filter(|(_, album)| app.filter.artist.as_ref().is_none_or(|artist| album.artist == *artist))
        .filter_map(|(ix, album)| Some((ix, search_rank(&album.title, &app.filter.search)?)))
        .collect();
    scored.sort_by(|(_, a), (_, b)| b.cmp(a));
    app.filtered = scored.into_iter().map(|(ix, _)| ix).collect();
}

/// Ranks `candidate` against a fuzzy search: `None` unless it contains every whitespace-split
/// word of `query` (case-insensitively); otherwise the length of the longest common substring
/// with the full query, so contiguous hits ("dark side" as a phrase) outrank scattered ones. An
/// empty query matches everything at rank 0.
pub fn search_rank(candidate: &str, query: &str) -> Option<usize> {
    let query = query.to_lowercase();
    if query.split_whitespace().next().is_none() {
        return Some(0);
    }
    let candidate = candidate.to_lowercase();
    query.split_whitespace().all(|word| candidate.contains(word)).then(|| lcs_len(&candidate, &query))
}

/// The length in bytes of the longest common substring of `a` and `b`: the classic quadratic
/// table, one rolling row. Both inputs are short (titles and queries), so this is microseconds.
fn lcs_len(a: &str, b: &str) -> usize {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut row = vec![0usize; b.len() + 1];
    let mut best = 0;
    for &ca in a {
        // Walk right-to-left so `row[j - 1]` still holds the previous row's value.
        for j in (1..=b.len()).rev() {
            row[j] = if ca == b[j - 1] { row[j - 1] + 1 } else { 0 };
            best = best.max(row[j]);
        }
    }
    best
}

/// The values the picker for `subject` searches over: every distinct value in the library,
/// sorted. Untagged albums contribute no genre; they show only under "(all)".
pub fn picker_options(app: &App, subject: PickerSubject) -> Vec<String> {
    let values: std::collections::BTreeSet<&String> = match subject {
        PickerSubject::Genre => app.albums.iter().map(|album| &album.genre).filter(|genre| !genre.is_empty()).collect(),
        PickerSubject::Artist => app.albums.iter().map(|album| &album.artist).collect(),
    };
    values.into_iter().cloned().collect()
}

/// The picker's matches for its current query: the subject's values ranked like the album search
/// (every word contained; longest common substring breaks ties, stably).
pub fn picker_matches(app: &App, subject: PickerSubject, query: &str) -> Vec<String> {
    let mut scored: Vec<(String, usize)> = picker_options(app, subject)
        .into_iter()
        .filter_map(|value| {
            let rank = search_rank(&value, query)?;
            Some((value, rank))
        })
        .collect();
    scored.sort_by(|(_, a), (_, b)| b.cmp(a));
    scored.into_iter().map(|(value, _)| value).collect()
}

/// The engine-facing form of the queue: each track's path plus its album grouping key, which
/// repeat-album advancement walks (see [`player::Entry`]).
pub fn entries(items: &[QueueItem]) -> Vec<player::Entry> {
    items.iter().map(|item| player::Entry { path: item.path.clone(), album: item.album_id }).collect()
}

/// Fills the queue items belonging to `album` with its tags, id, and cover art -- how a restored,
/// paths-only queue hydrates as the library reports albums (from the persisted index at boot, and
/// from every scan event thereafter).
pub fn hydrate_queue(queue: &mut [QueueItem], album: &Album) {
    let paths: HashSet<&PathBuf> = album.tracks.iter().map(|t| &t.path).collect();
    for item in queue.iter_mut().filter(|item| paths.contains(&item.path)) {
        item.album_id = album.id;
        item.artist = album.artist.clone();
        item.album = album.title.clone();
        item.cover = album.cover.clone();
        item.accent = album.accent;
        if let Some(track) = album.tracks.iter().find(|t| t.path == item.path) {
            item.title = track.title.clone();
        }
    }
}

/// The queue's albums as a cover-loading priority: distinct album ids ordered circularly outward
/// from the playing track -- current, next, previous, next-but-one, ... -- so a session restored
/// into the player dresses the covers nearest the flow's center first.
fn cover_priority(queue: &[QueueItem], current: usize) -> Vec<u64> {
    let n = queue.len();
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    if n == 0 {
        return ids;
    }
    let current = current.min(n - 1);
    for d in 0..n {
        let mut consider = |ix: usize| {
            let id = queue[ix].album_id;
            if seen.insert(id) {
                ids.push(id);
            }
        };
        if current + d < n {
            consider(current + d);
        }
        if d > 0 && current >= d {
            consider(current - d);
        }
    }
    ids
}

/// A provisional album grouping key for a track not yet matched to the library: its parent
/// directory (a serviceable stand-in for the tag-derived album until the tags arrive), hashed
/// into the album-id key space. Replaced by the real album id when the scan hydrates the item.
fn dir_key(path: &std::path::Path) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    path.parent().hash(&mut hasher);
    hasher.finish()
}

/// The queue's contiguous runs of tracks from the same album, as ranges into the queue. The
/// Cover Flow shows one cover per run rather than one per track.
pub fn album_runs(queue: &[QueueItem]) -> Vec<std::ops::Range<usize>> {
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    for (ix, item) in queue.iter().enumerate() {
        match runs.last_mut() {
            Some(run) if queue[run.start].album_id == item.album_id => run.end = ix + 1,
            _ => runs.push(ix..ix + 1),
        }
    }
    runs
}

/// The index of the run containing the given track index.
pub fn run_of(runs: &[std::ops::Range<usize>], track: usize) -> usize {
    runs.iter().position(|run| run.contains(&track)).unwrap_or(0)
}

/// The Cover Flow target position for the currently playing track.
pub fn flow_target(app: &App) -> f32 {
    run_of(&album_runs(&app.queue), app.current) as f32
}

/// The currently playing album's id (a stable, near-unique per-album value); 0 when nothing is
/// playing.
pub fn current_album_id(app: &App) -> u64 {
    app.queue.get(app.current).map_or(0, |item| item.album_id)
}

/// Where the currently playing album's glow sits, as a fraction of the viewport size. Scattered
/// per album by indexing fixed position tables with the album id (already a well-mixed hash);
/// the low digit picks the column, a higher one the row, so the two axes don't correlate.
pub fn glow_center(album_id: u64) -> (f32, f32) {
    const CENTERS_X: [f32; 7] = [0.20, 0.30, 0.40, 0.50, 0.60, 0.70, 0.80];
    const CENTERS_Y: [f32; 4] = [0.10, 0.20, 0.30, 0.40];
    let (nx, ny) = (CENTERS_X.len() as u64, CENTERS_Y.len() as u64);
    (CENTERS_X[(album_id % nx) as usize], CENTERS_Y[(album_id / nx % ny) as usize])
}

/// The glow the currently playing album should ultimately show: its accent, saturated to full
/// brightness in proportion to how much color it actually has, placed at its scattered position;
/// black when nothing is playing. The backdrop shader decides how much of the color to show.
pub fn current_glow(app: &App) -> GlowState {
    // Read from the item's own accent (known from the index at boot), not through the loaded
    // cover: the glow must not wait for thumbnails.
    let color = match app.queue.get(app.current).and_then(|item| item.accent) {
        Some(accent) => {
            let max = accent.r.max(accent.g).max(accent.b);
            let min = accent.r.min(accent.g).min(accent.b);
            if max <= f32::EPSILON {
                iced::Color::BLACK
            } else {
                // Saturate the hue only in proportion to the accent's chroma: a grayscale
                // cover's accent is its dominant shade, whose channel ratios are noise --
                // normalizing them outright turned black covers vivid blue. With little chroma
                // the glow stays the shade itself (black art glows dark, white art white).
                let trust = ((max - min) / 0.15).clamp(0.0, 1.0);
                let channel = |c: f32| c + (c / max - c) * trust;
                iced::Color { r: channel(accent.r), g: channel(accent.g), b: channel(accent.b), a: 1.0 }
            }
        }
        None => iced::Color::BLACK,
    };
    GlowState { color, center: glow_center(current_album_id(app)) }
}

/// Blend between two glow states by progress `p` (0 = `from`, 1 = `to`): the center glides from one position to the
/// other while the color cross-fades, both smoothstep-eased. The color is interpolated in linear light so the midpoint
/// stays correctly bright, rather than dipping dark the way a gamma-space lerp of the sRGB values would.
pub fn glow_blend(from: GlowState, to: GlowState, p: f32) -> GlowState {
    let ease = p * p * (3.0 - 2.0 * p); // smoothstep
    let center = (from.center.0 + (to.center.0 - from.center.0) * ease, from.center.1 + (to.center.1 - from.center.1) * ease);
    let from_c = from.color.into_linear();
    let to_c = to.color.into_linear();
    let [r, g, b] = std::array::from_fn(|i| (1.0 - ease) * from_c[i] + ease * to_c[i]);
    let color = iced::Color::from_linear_rgba(r, g, b, 1.0);
    GlowState { color, center }
}

/// Whether the backdrop glow is mid-transition (or needs to start one), i.e. not settled on the
/// current album's target glow.
pub fn glow_animating(app: &App) -> bool {
    app.glow_p < 1.0 || app.glow_album != current_album_id(app) || app.glow_to != current_glow(app)
}

/// The glow to render this frame: the current point of the `glow_from` -> `glow_to` blend.
pub fn glow_now(app: &App) -> GlowState {
    glow_blend(app.glow_from, app.glow_to, app.glow_p)
}

pub fn queue_items(album: &Album) -> Vec<QueueItem> {
    album
        .tracks
        .iter()
        .map(|t| QueueItem {
            path: t.path.clone(),
            album_id: album.id,
            title: t.title.clone(),
            artist: album.artist.clone(),
            album: album.title.clone(),
            cover: album.cover.clone(),
            accent: album.accent,
        })
        .collect()
}

#[cfg(test)]
mod test {
    use super::*;

    fn pixels(byte: u8) -> Arc<[u8]> {
        Arc::from(vec![byte])
    }

    /// The oldest entry is evicted when the cache overflows -- but a `touch` (as every on-screen
    /// query does) makes an entry the freshest, so the window survives while colder covers go.
    #[test]
    fn evicts_the_least_recently_used() {
        let mut cache = HiResCache::new();
        for id in 0..HIRES_CAP as u64 {
            cache.complete(id, Some(pixels(0)));
        }
        // Refresh the oldest entry, then overflow by one.
        assert!(cache.touch(0));
        cache.complete(HIRES_CAP as u64, Some(pixels(1)));

        assert_eq!(cache.entries.len(), HIRES_CAP);
        assert!(cache.peek(0).is_some(), "the touched entry must survive");
        assert!(cache.peek(1).is_none(), "the now-oldest entry must be evicted");
        assert!(cache.peek(HIRES_CAP as u64).is_some(), "the newcomer must be resident");
    }

    /// `peek` is a probe, not a use: hammering it must not protect an entry from eviction (only
    /// `query`, via `touch`, promotes -- once per album move, not once per frame).
    #[test]
    fn peek_does_not_promote() {
        let mut cache = HiResCache::new();
        for id in 0..HIRES_CAP as u64 {
            cache.complete(id, Some(pixels(0)));
        }
        for _ in 0..5 {
            assert!(cache.peek(0).is_some());
        }
        cache.complete(HIRES_CAP as u64, Some(pixels(1)));
        assert!(cache.peek(0).is_none(), "peek must not have promoted the oldest entry");
    }

    /// A failed decode clears the in-flight mark but stores nothing, leaving the cover on its
    /// thumbnail -- and lets a later query retry it rather than being deduplicated forever.
    #[test]
    fn failed_decode_stores_nothing_and_reopens_the_query() {
        let mut cache = HiResCache::new();
        cache.pending.insert(9);
        cache.complete(9, None);
        assert!(cache.peek(9).is_none());
        assert!(!cache.pending.contains(&9), "a failed decode must clear the pending mark");
    }

    #[test]
    fn search_ranks_contiguous_matches_higher() {
        assert_eq!(search_rank("The Dark Side of the Moon", ""), Some(0), "an empty query matches everything");
        assert_eq!(search_rank("The Dark Side of the Moon", "dark side"), Some(9), "a phrase hit scores its full length");
        assert_eq!(search_rank("Darkness on the Far Side", "dark side"), Some(5), "scattered words score the longest run");
        assert_eq!(search_rank("The Wall", "dark side"), None, "every word must be contained");
        assert_eq!(search_rank("MONO no aware", "mono"), Some(4), "matching is case-insensitive");
    }
}
