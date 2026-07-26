//! The application state.

use crate::covers::Cover;
use crate::logger;
use phonoscule::config::Conf;
use phonoscule::library::Album;
use phonoscule::player;
use phonoscule::sort::{Dir, SortField, SortOrder};
use ratatui::widgets::ListState;
use ratatui_image::picker::Picker;
use std::collections::VecDeque;
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
    /// What the terminal can draw images with (see the covers module).
    pub picker: Picker,
    /// The cover on screen, the only one held: the browser's selection, or the playing album once
    /// there is one.
    pub cover: Option<Cover>,
    pub scan: ScanState,
    /// Every album, ordered by artist then title -- the order scan events upsert into, not the
    /// order the browser shows. See [`shown`](Self::shown).
    pub albums: Vec<Album>,
    /// Whether `albums` has drifted from the persisted index since it was last written, so the
    /// quiet rescans don't rewrite it every time.
    pub index_dirty: bool,
    /// Indices into `albums` in display order (see [`refresh`]).
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
    /// Recent log records, newest last (see the logger module).
    pub log: VecDeque<logger::Entry>,
    /// Set to leave the event loop.
    pub quit: bool,
}

impl Model {
    pub fn new(conf: Conf, picker: Picker, engine: player::Engine, albums: Vec<Album>) -> Self {
        let mut model = Model {
            engine,
            queue: vec![],
            current: 0,
            play_state: player::PlayState::Paused,
            repeat: player::Repeat::Off,
            pos: Duration::ZERO,
            len: None,
            pending_seek: None,
            conf,
            picker,
            cover: None,
            scan: ScanState::Scanning,
            albums,
            index_dirty: false,
            shown: vec![],
            shown_dirty: false,
            sort: INITIAL_SORT,
            selected: None,
            list: ListState::default(),
            view: View::Library,
            log: VecDeque::new(),
            quit: false,
        };
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

    /// Puts the selection on the album at `row`, clamped to the list.
    pub fn select_row(&mut self, row: usize) {
        let row = row.min(self.shown.len().saturating_sub(1));
        self.selected = self.shown.get(row).map(|&ix| self.albums[ix].id);
    }

    /// The queue item playing, if the queue isn't empty.
    pub fn playing(&self) -> Option<&QueueItem> {
        self.queue.get(self.current)
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

/// Recomputes the display order from [`Model::sort`]. The selection needs no fixing up: it names an
/// album, not a row.
pub fn refresh(model: &mut Model) {
    model.shown_dirty = false;
    let sort = model.sort;
    let mut shown: Vec<usize> = (0..model.albums.len()).collect();
    shown.sort_by(|&a, &b| sort.cmp(&model.albums[a], &model.albums[b]));
    model.shown = shown;
}

#[cfg(test)]
pub use testing::browser;

#[cfg(test)]
mod testing {
    use super::*;
    use phonoscule::library::TrackInfo;
    use ratatui_image::picker::Picker;
    /// A browser over `n` synthetic albums, sorted so their titles read in order.
    pub fn browser(n: usize) -> Model {
        let conf = Conf::new("tui", "/music".into());
        let albums = (0..n)
            .map(|i| Album {
                id: i as u64,
                title: format!("Album {i:03}"),
                artist: format!("Artist {i:03}"),
                genre: "Genre".into(),
                year: Some(2000),
                cover_id: None,
                cover: None,
                accent: None,
                tracks: vec![TrackInfo { path: format!("{i}.opus").into(), title: "Track".into() }],
            })
            .collect();
        let engine = player::start(player::Client { name: "phonoscule-tui-test".into(), description: String::new() });
        Model::new(conf, Picker::halfblocks(), engine, albums)
    }
}
