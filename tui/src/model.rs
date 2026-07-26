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
    /// Indices into `albums` in display order (see [`refresh`]). Every selection is an index into
    /// this.
    pub shown: Vec<usize>,
    pub sort: SortOrder,
    /// The browser's selection, as an index into `shown`.
    pub selected: usize,
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
            sort: SortOrder::default(),
            selected: 0,
            view: View::Library,
            log: VecDeque::new(),
            quit: false,
        };
        refresh(&mut model);
        model
    }

    /// The album the browser's selection sits on.
    pub fn selected_album(&self) -> Option<&Album> {
        self.albums.get(*self.shown.get(self.selected)?)
    }

    pub fn push_log(&mut self, entry: logger::Entry) {
        if self.log.len() == LOG_CAPACITY {
            self.log.pop_front();
        }
        self.log.push_back(entry);
    }
}

/// Recomputes the display order from [`Model::sort`], keeping the selection on the album it was on
/// where that album is still there.
pub fn refresh(model: &mut Model) {
    let was = model.selected_album().map(|album| album.id);
    let sort = model.sort;
    let mut shown: Vec<usize> = (0..model.albums.len()).collect();
    shown.sort_by(|&a, &b| sort.cmp(&model.albums[a], &model.albums[b]));
    model.shown = shown;
    model.selected = was
        .and_then(|id| model.shown.iter().position(|&ix| model.albums[ix].id == id))
        .unwrap_or(model.selected)
        .min(model.shown.len().saturating_sub(1));
}
