//! The application state.

use crate::covers::Cover;
use crate::logger;
use phonoscule::config::Conf;
use phonoscule::library::Album;
use phonoscule::sort::SortOrder;
use ratatui_image::picker::Picker;
use std::collections::VecDeque;

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
    pub view: View,
    /// Recent log records, newest last (see the logger module).
    pub log: VecDeque<logger::Entry>,
    /// Set to leave the event loop.
    pub quit: bool,
}

impl Model {
    pub fn new(conf: Conf, picker: Picker, albums: Vec<Album>) -> Self {
        let mut model = Model {
            conf,
            picker,
            cover: None,
            scan: ScanState::Scanning,
            albums,
            index_dirty: false,
            shown: vec![],
            shown_dirty: false,
            sort: SortOrder::default(),
            selected: None,
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
