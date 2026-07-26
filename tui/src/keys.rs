//! Key bindings: the one place a key press becomes a [`Msg`].
//!
//! Modifier-carrying chords only, never a bare letter -- letters are reserved for type-to-search in
//! the browser. Kept as a single table so it can be made configurable without hunting through the
//! views.

use crate::model::{Focus, Subject, View};
use crate::update::{Edge, Msg};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use phonoscule::queue::{Grouping, Scope};

/// The bindings below, for `--help`. Kept here rather than beside the argument handling so that
/// changing a binding puts its description in front of whoever changed it.
///
/// Spelled out, unlike the filter bar's `^g` chips, because help is read once and a bar is read at a
/// glance.
pub const HELP: &str = "\
Controls:
  Anywhere
    Space              Play or pause
    Tab, Shift+Tab     Switch between the Library and Player views
    Ctrl+Q, Ctrl+C     Quit
    Alt+R              Cycle the repeat mode: off, track, album, queue
    Ctrl+S, Ctrl+Z     Shuffle the whole queue: by album, by track
    Alt+S, Alt+Z       Shuffle all but what is playing: by album, by track
    Ctrl+K             Clear the queue

  Library
    Up/Down, PgUp/PgDn, Home/End
                       Move the selection
    (any letter)       Search album titles, narrowing as you type
    Backspace          Correct the search
    Ctrl+F             Back to the search
    Ctrl+G, Ctrl+T     Pick a genre, an artist
    Ctrl+O             Pick the order albums are listed in
    Ctrl+W             Clear every filter
    Enter              Open the selected album's tracks
    Alt+Enter          Queue the selected album
    Ctrl+A, Alt+A      Play, queue every album shown

  An album's tracks
    Up/Down, PgUp/PgDn, Home/End
                       Move the selection
    Enter              Play that track alone
    Alt+Enter          Queue it, stepping onto the next
    Ctrl+A, Alt+A      Play, queue the whole album
    Esc                Close

  A picker
    (any letter)       Search what it holds
    Up/Down            Move the selection
    Enter              Take it; its first row clears the filter it sets
    Esc                Close, changing nothing

  Player
    Left/Right         Seek
    Up/Down            Volume
    Home/End           Previous, next track

";

/// How far a single seek key press moves, in seconds.
const SEEK_STEP: i64 = 5;

/// How much a single volume key press moves, of the whole range.
const VOLUME_STEP: f32 = 0.05;

/// The message a key press means, given what has focus and which view is up. `None` if it is bound
/// to nothing.
pub fn key_to_msg(view: View, focus: &Focus, key: KeyEvent) -> Option<Msg> {
    // Terminals that speak the kitty keyboard protocol report releases and repeats too; only
    // presses are bindings.
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    // Bindings that hold whatever has focus, typing included: a chord cannot be mistaken for text.
    match key.code {
        KeyCode::Char('c') if ctrl => return Some(Msg::Quit),
        KeyCode::Char('q') if ctrl => return Some(Msg::Quit),
        KeyCode::Char('r') if alt => return Some(Msg::CycleRepeat),
        KeyCode::Char('w') if ctrl => return Some(Msg::ClearFilters),
        KeyCode::Char('f') if ctrl => return Some(Msg::Search(None)),
        KeyCode::Char('g') if ctrl => return Some(Msg::OpenPicker(Subject::Genre)),
        KeyCode::Char('t') if ctrl => return Some(Msg::OpenPicker(Subject::Artist)),
        KeyCode::Char('o') if ctrl => return Some(Msg::OpenPicker(Subject::Sort)),
        // In a track menu these act on the album whose tracks are listed; elsewhere on every album
        // the filter lets through.
        KeyCode::Char('a') if ctrl && matches!(focus, Focus::Tracks(_)) => return Some(Msg::PlaySelected),
        KeyCode::Char('a') if alt && matches!(focus, Focus::Tracks(_)) => return Some(Msg::QueueSelected),
        // As in the GUI: Ctrl shuffles the whole queue, Alt shuffles everything but what is playing;
        // `s` moves albums as units, `z` moves single tracks. Raw mode has already taken Ctrl+S and
        // Ctrl+Z off the terminal's hands (no flow control, no suspend), so both arrive as keys.
        KeyCode::Char('s') if ctrl || alt => {
            let scope = if ctrl { Scope::All } else { Scope::Others };
            return Some(Msg::Shuffle { grouping: Grouping::Albums, scope });
        }
        KeyCode::Char('z') if ctrl || alt => {
            let scope = if ctrl { Scope::All } else { Scope::Others };
            return Some(Msg::Shuffle { grouping: Grouping::Tracks, scope });
        }
        KeyCode::Char('k') if ctrl => return Some(Msg::ClearQueue),
        _ => (),
    }

    // What is being typed at gets the plain keys, so a search can contain a space or a `q`.
    match focus {
        Focus::Search => return searching(key),
        Focus::Picker(_) => return picking(key),
        Focus::Tracks(_) => return in_track_menu(key, alt),
        Focus::Albums => (),
    }

    match key.code {
        KeyCode::Tab => return Some(Msg::Show(view.next())),
        KeyCode::BackTab => return Some(Msg::Show(view.prev())),
        // Play/pause from anywhere, as in the GUI.
        KeyCode::Char(' ') if key.modifiers.is_empty() => return Some(Msg::Toggle),
        _ => (),
    }

    match view {
        View::Library => match key.code {
            KeyCode::Up => Some(Msg::Select(-1)),
            KeyCode::Down => Some(Msg::Select(1)),
            KeyCode::PageUp => Some(Msg::Select(-10)),
            KeyCode::PageDown => Some(Msg::Select(10)),
            KeyCode::Home => Some(Msg::SelectEdge(Edge::First)),
            KeyCode::End => Some(Msg::SelectEdge(Edge::Last)),
            // Enter opens the album's tracks, where one can be played or queued on its own; the whole
            // album is a keypress further in, or Alt+Enter from here.
            KeyCode::Enter if alt => Some(Msg::QueueSelected),
            // Everything the filter lets through: Ctrl+A plays it, Alt+A appends it. The GUI's pair
            // is Ctrl+Enter and Alt+Enter, but Alt+Enter queues the selection here, and terminals
            // take Ctrl+Enter for themselves -- Ghostty toggles full screen with it.
            KeyCode::Char('a') if ctrl => Some(Msg::PlayShown),
            KeyCode::Char('a') if alt => Some(Msg::QueueShown),
            KeyCode::Enter => Some(Msg::OpenTracks),
            // Typing searches, which is why no letter carries a binding of its own.
            KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => Some(Msg::Search(Some(c))),
            // As does rubbing out, for correcting a search after the keys have gone back to the list.
            KeyCode::Backspace => Some(Msg::Rubout),
            _ => None,
        },
        View::Player => match key.code {
            KeyCode::Left => Some(Msg::Seek(-SEEK_STEP)),
            KeyCode::Right => Some(Msg::Seek(SEEK_STEP)),
            KeyCode::Up => Some(Msg::BumpVolume(VOLUME_STEP)),
            KeyCode::Down => Some(Msg::BumpVolume(-VOLUME_STEP)),
            KeyCode::Home => Some(Msg::Prev),
            KeyCode::End => Some(Msg::Next),
            _ => None,
        },
    }
}

/// In an album's track menu: the arrows walk its tracks, Enter plays the one they land on and
/// Alt+Enter appends it. Ctrl+A and Alt+A, handled above, take the album whole.
fn in_track_menu(key: KeyEvent, alt: bool) -> Option<Msg> {
    match key.code {
        KeyCode::Up => Some(Msg::TrackMove(-1)),
        KeyCode::Down => Some(Msg::TrackMove(1)),
        KeyCode::PageUp => Some(Msg::TrackMove(-10)),
        KeyCode::PageDown => Some(Msg::TrackMove(10)),
        KeyCode::Home => Some(Msg::TrackMove(isize::MIN)),
        KeyCode::End => Some(Msg::TrackMove(isize::MAX)),
        KeyCode::Enter if alt => Some(Msg::QueueTrack),
        KeyCode::Enter => Some(Msg::PlayTrack),
        KeyCode::Esc => Some(Msg::Done),
        _ => None,
    }
}

/// Typing in the album search. Everything printable goes into the query, so only the keys that end or
/// undo it are bound.
fn searching(key: KeyEvent) -> Option<Msg> {
    match key.code {
        KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => Some(Msg::Typed(c)),
        KeyCode::Backspace => Some(Msg::Rubout),
        // The search stands either way: Enter leaves it be, Escape hands the keys back with it in
        // place. Ctrl+W is what clears it.
        KeyCode::Enter | KeyCode::Esc => Some(Msg::Done),
        // The list moves under a search still being typed, as it does in the GUI.
        KeyCode::Up => Some(Msg::Select(-1)),
        KeyCode::Down => Some(Msg::Select(1)),
        _ => None,
    }
}

/// Typing in a picker: the same, with the arrows walking its rows rather than the album list.
fn picking(key: KeyEvent) -> Option<Msg> {
    match key.code {
        KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => Some(Msg::Typed(c)),
        KeyCode::Backspace => Some(Msg::Rubout),
        KeyCode::Up => Some(Msg::PickerMove(-1)),
        KeyCode::Down => Some(Msg::PickerMove(1)),
        KeyCode::PageUp => Some(Msg::PickerMove(-10)),
        KeyCode::PageDown => Some(Msg::PickerMove(10)),
        KeyCode::Enter => Some(Msg::Pick),
        KeyCode::Esc => Some(Msg::Done),
        _ => None,
    }
}
