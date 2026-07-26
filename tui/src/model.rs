//! The application state.

use crate::covers::Covers;
use crate::logger;
use phonoscule::config::Conf;
use phonoscule::library::Album;
use phonoscule::player;
use phonoscule::search;
use phonoscule::session;
use phonoscule::sort::{Dir, SortField, SortOrder};
use ratatui::widgets::ListState;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Library,
    Player,
}

impl View {
    /// The next/previous view in tab order, wrapping around.
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

    pub fn label(self) -> &'static str {
        match self {
            View::Library => "Library",
            View::Player => "Player",
        }
    }

    pub const ALL: [View; 2] = [View::Library, View::Player];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanState {
    Scanning,
    Complete,
}

/// What the browser shows, of everything in the library: a genre and an artist picked exactly, and a
/// fuzzy search over album titles, all of which must pass.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub genre: Option<String>,
    pub artist: Option<String>,
    pub search: String,
}

impl Filter {
    /// Whether it lets everything through, so there is nothing to clear.
    pub fn is_empty(&self) -> bool {
        self.genre.is_none() && self.artist.is_none() && self.search.is_empty()
    }
}

/// What the keyboard is talking to. Only one thing at a time, by construction.
#[derive(Debug, Clone)]
pub enum Focus {
    /// The album list: the arrow keys move the selection, typing starts a search.
    Albums,
    /// The album search field: typing appends to the query and the browser narrows as it goes.
    Search,
    /// A picker over the current view (see [`Picker`]).
    Picker(Picker),
}

/// What a picker is picking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    Genre,
    Artist,
    /// The browser's order, over [`SortOrder::ALL`] rather than over library values.
    Sort,
}

/// An open picker: a search over the subject's values, the ones matching it, and where the selection
/// sits. Row 0 is the standing "any" entry that clears the filter, so row `n + 1` is `matches[n]`;
/// the sort picker has no such entry, its options being an order and not a filter.
#[derive(Debug, Clone)]
pub struct Picker {
    pub subject: Subject,
    pub query: String,
    pub matches: Vec<String>,
    pub selected: usize,
}

impl Picker {
    /// Whether row 0 is the "any genre"/"any artist" entry that clears the filter.
    pub fn has_any_row(&self) -> bool {
        self.subject != Subject::Sort
    }

    /// How many rows it shows.
    pub fn rows(&self) -> usize {
        self.matches.len() + usize::from(self.has_any_row())
    }

    /// The value the selection sits on, or `None` for the "any" row.
    pub fn picked(&self) -> Option<&str> {
        let ix = self.selected.checked_sub(usize::from(self.has_any_row()))?;
        self.matches.get(ix).map(String::as_str)
    }
}

/// A track in the play queue. Its album is named by id rather than carried along: the album list
/// holds the tags and the cover already, and looking them up beats keeping a second copy.
#[derive(Debug, Clone)]
pub struct QueueItem {
    pub path: PathBuf,
    pub album_id: u64,
    pub title: String,
}

/// The order the browser starts in: by artist, then by year within each artist. The framework's own
/// default sorts by album name, which reads as no order at all in a list whose rows lead with the
/// year and the artist.
const INITIAL_SORT: SortOrder = SortOrder { group_by_artist: Some(Dir::Asc), field: SortField::Year, field_dir: Dir::Asc };

/// How many log records are kept for the log view; older ones are dropped.
const LOG_CAPACITY: usize = 500;

pub struct Model {
    pub conf: Conf,
    /// The covers held for display, and how the terminal draws them (see the covers module).
    pub covers: Covers,
    pub scan: ScanState,
    /// Every album, ordered by artist then title -- the order scan events upsert into, not the
    /// order the browser shows. See [`shown`](Self::shown).
    pub albums: Vec<Album>,
    /// Whether `albums` has drifted from the persisted index since it was last written, so the
    /// quiet rescans don't rewrite it every time.
    pub index_dirty: bool,
    /// Which albums the browser shows, of everything in `albums`.
    pub filter: Filter,
    /// What the keyboard is talking to.
    pub focus: Focus,
    /// Indices into `albums` in display order, filtered (see [`refresh`]).
    pub shown: Vec<usize>,
    /// Whether `shown` still reflects `albums` and `sort`. Set by anything that changes either;
    /// cleared by [`refresh`], which the event loop calls once per burst of messages rather than
    /// once per message -- re-sorting the whole library on each of a scan's hundreds of album
    /// reports is what made the first frame take seconds to arrive.
    pub shown_dirty: bool,
    pub sort: SortOrder,
    /// The album the browser's selection sits on, by id, so it stays on that album as the scan
    /// reorders the list under it -- and so nothing needs fixing up when the order changes. `None`
    /// until the selection is moved, which means the first row.
    pub selected: Option<u64>,
    /// The browser list's own state, which is to say where it is scrolled to. Kept across frames:
    /// the widget writes the offset it settled on back into it, and a fresh one each frame would
    /// throw that away and re-derive an offset that only just brings the selection into view --
    /// pinning it to the bottom row.
    pub list: ListState,
    pub view: View,
    pub engine: player::Engine,
    pub queue: Vec<QueueItem>,
    /// Which queue item is playing, an index into `queue`.
    pub current: usize,
    pub play_state: player::PlayState,
    pub repeat: player::Repeat,
    pub pos: Duration,
    /// The playing track's length, once the engine has opened it and said so.
    pub len: Option<Duration>,
    /// The position last asked of the engine, held until playback actually reaches it. A burst of
    /// seeks accumulates from here rather than from the round-trip-lagged reported position, and
    /// reports still in flight from before the seek are ignored until it arrives -- otherwise they
    /// yank the bar back and it rubberbands.
    pub pending_seek: Option<Duration>,
    /// Whether the queue's titles and album ids need matching against the library again, because the
    /// library changed under it. Done once per burst of messages rather than per album reported: a
    /// scan reports hundreds, and re-matching a whole queue for each is the same quadratic cost.
    pub queue_stale: bool,
    /// Whether the queue, or the state around it, has changed since it was last written. The event
    /// loop writes them out after a burst of messages rather than on each one, so holding a seek key
    /// does not rewrite the session on every report.
    pub dirty_playlist: bool,
    pub dirty_player: bool,
    /// Recent log records, newest last (see the logger module).
    pub log: VecDeque<logger::Entry>,
    /// Set to leave the event loop.
    pub quit: bool,
}

impl Model {
    /// The model as a previous run left it: the queue it was playing, where in it, and the repeat mode
    /// and order it was using.
    pub fn restored(
        conf: Conf,
        covers: Covers,
        engine: player::Engine,
        albums: Vec<Album>,
        restored: session::Restored,
    ) -> Self {
        let mut model = Model {
            engine,
            queue: vec![],
            current: 0,
            play_state: player::PlayState::Paused,
            repeat: restored.repeat,
            pos: Duration::ZERO,
            len: None,
            pending_seek: None,
            queue_stale: false,
            dirty_playlist: false,
            dirty_player: false,
            conf,
            covers,
            scan: ScanState::Scanning,
            albums,
            index_dirty: false,
            filter: Filter::default(),
            focus: Focus::Albums,
            shown: vec![],
            shown_dirty: false,
            sort: restored.sort.unwrap_or(INITIAL_SORT),
            selected: None,
            list: ListState::default(),
            view: View::Library,
            log: VecDeque::new(),
            quit: false,
        };
        // Only paths were saved: the titles and album ids come back from the library, matched by
        // path, so a queue is readable before any scan has run.
        model.queue = restored
            .tracks
            .iter()
            .map(|path| QueueItem {
                title: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                album_id: 0,
                path: path.clone(),
            })
            .collect();
        hydrate(&model.albums, &mut model.queue);
        model.current = restored.current.min(model.queue.len().saturating_sub(1));
        // A restored session comes up where it left off, on the player, ready to resume.
        if !model.queue.is_empty() {
            model.view = View::Player;
        }
        refresh(&mut model);
        model
    }

    /// Which row of `shown` the selection sits on: the selected album's, or the first.
    pub fn selected_row(&self) -> usize {
        self.selected.and_then(|id| self.shown.iter().position(|&ix| self.albums[ix].id == id)).unwrap_or(0)
    }

    /// The album the browser's selection sits on.
    pub fn selected_album(&self) -> Option<&Album> {
        self.albums.get(*self.shown.get(self.selected_row())?)
    }

    /// The album shown at `row`, if there is one.
    pub fn album_at(&self, row: usize) -> Option<&Album> {
        self.albums.get(*self.shown.get(row)?)
    }

    /// Puts the selection on the album at `row`, clamped to the list.
    pub fn select_row(&mut self, row: usize) {
        let row = row.min(self.shown.len().saturating_sub(1));
        self.selected = self.shown.get(row).map(|&ix| self.albums[ix].id);
    }

    /// The queue item playing, if the queue isn't empty.
    pub fn playing(&self) -> Option<&QueueItem> {
        self.queue.get(self.current)
    }

    /// The queue's albums in order, one entry per run of tracks from the same album.
    pub fn queue_albums(&self) -> Vec<u64> {
        let mut albums = Vec::new();
        for item in &self.queue {
            if albums.last() != Some(&item.album_id) {
                albums.push(item.album_id);
            }
        }
        albums
    }

    /// The album a queue item belongs to, if it is still in the library.
    pub fn album_of(&self, item: &QueueItem) -> Option<&Album> {
        self.albums.iter().find(|album| album.id == item.album_id)
    }

    /// Sends a command to the engine. The channel is unbounded, so this only fails once the engine
    /// is gone.
    pub fn send(&self, cmd: player::Cmd) {
        if self.engine.cmd.try_send(cmd).is_err() {
            log::error!("the player engine is gone");
        }
    }

    pub fn push_log(&mut self, entry: logger::Entry) {
        if self.log.len() == LOG_CAPACITY {
            self.log.pop_front();
        }
        self.log.push_back(entry);
    }
}

/// Recomputes which albums the browser shows and in what order: those the filter lets through,
/// ordered by [`Model::sort`]. A search overrides that order, ranking its best matches first, with
/// the sort breaking ties; an empty one leaves the order alone, every album ranking equal.
///
/// The selection needs no fixing up: it names an album, not a row.
pub fn refresh(model: &mut Model) {
    model.shown_dirty = false;
    let sort = model.sort;
    let filter = &model.filter;
    let mut ranked: Vec<(usize, usize)> = model
        .albums
        .iter()
        .enumerate()
        .filter(|(_, album)| filter.genre.as_ref().is_none_or(|genre| album.genre == *genre))
        .filter(|(_, album)| filter.artist.as_ref().is_none_or(|artist| album.artist == *artist))
        .filter_map(|(ix, album)| Some((ix, search::rank(&album.title, &filter.search)?)))
        .collect();
    ranked.sort_by(|&(a, ra), &(b, rb)| rb.cmp(&ra).then_with(|| sort.cmp(&model.albums[a], &model.albums[b])));
    model.shown = ranked.into_iter().map(|(ix, _)| ix).collect();
}

/// Fills in the queue's titles and album ids from the library, matched by path -- which is how a
/// queue restored from paths alone gets its tags back, and how it keeps them as the library is
/// rescanned.
///
/// Indexes the library once and then walks the queue once. Asking each album whether it owns each
/// queue item instead is quadratic, and quadratic on a queue holding a whole library is over two
/// seconds before the first frame.
pub fn hydrate(albums: &[Album], queue: &mut [QueueItem]) {
    if queue.is_empty() {
        return;
    }
    let mut by_path: HashMap<&std::path::Path, (u64, &str)> = HashMap::new();
    for album in albums {
        for track in &album.tracks {
            by_path.insert(track.path.as_path(), (album.id, track.title.as_str()));
        }
    }
    for item in queue.iter_mut() {
        if let Some(&(album_id, title)) = by_path.get(item.path.as_path()) {
            item.album_id = album_id;
            item.title = title.to_string();
        }
    }
}

/// Every distinct value of `subject` in the library, sorted -- what its picker searches over. An
/// album with no genre tag contributes none, and shows only under "any genre".
pub fn picker_options(model: &Model, subject: Subject) -> Vec<String> {
    let values: BTreeSet<&String> = match subject {
        Subject::Genre => model.albums.iter().map(|a| &a.genre).filter(|genre| !genre.is_empty()).collect(),
        Subject::Artist => model.albums.iter().map(|a| &a.artist).collect(),
        Subject::Sort => return SortOrder::ALL.iter().map(|order| order.label()).collect(),
    };
    values.into_iter().cloned().collect()
}

/// A picker over `subject`, showing everything until its query narrows it.
pub fn open_picker(model: &Model, subject: Subject) -> Picker {
    let matches = picker_options(model, subject);
    // The sort picker opens on the order in use, so stepping from it is stepping from where you are.
    let selected = match subject {
        Subject::Sort => SortOrder::ALL.iter().position(|&order| order == model.sort).unwrap_or(0),
        _ => 0,
    };
    Picker { subject, query: String::new(), matches, selected }
}

#[cfg(test)]
pub use testing::browser;

#[cfg(test)]
mod testing {
    use super::*;
    use crate::covers::Covers;
    use phonoscule::library::TrackInfo;
    use ratatui_image::picker::Picker;
    /// A browser over `n` synthetic albums, sorted so their titles read in order.
    pub fn browser(n: usize) -> Model {
        let conf = Conf::new("tui", "/music".into());
        let albums = (0..n)
            .map(|i| Album {
                id: i as u64,
                title: format!("Album {i:03}"),
                artist: format!("Artist {:03}", i % 3),
                genre: if i % 2 == 0 { "Metal".into() } else { "Jazz".into() },
                year: Some(2000 + (i % 5) as u32),
                cover_id: None,
                cover: None,
                accent: None,
                tracks: (0..3)
                    .map(|t| TrackInfo { path: format!("{i}/{t}.opus").into(), title: format!("Track {t}") })
                    .collect(),
            })
            .collect();
        let engine = player::start(player::Client { name: "phonoscule-tui-test".into(), description: String::new() });
        // A thumbnail directory that need not exist: the tests ask what covers are wanted, and never
        // run the loads that would read it.
        let covers = Covers::new(Picker::halfblocks(), Some("/covers".into()));
        Model::restored(conf, covers, engine, albums, session::Restored::default())
    }
}
