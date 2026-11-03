//! Key bindings: the one place a key press becomes a [`Msg`].
//!
//! The browser's letters are reserved for type-to-search, so it binds only chords, digits and `?`.
//! Kept as a single table so it can be made configurable without hunting through the views.

use crate::model::{Focus, Subject, View};
use crate::update::{Edge, Msg};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use phonoscule::queue::{Grouping, Scope};

/// The bindings below, for `--help` and the in-app key list. Kept here rather than beside the
/// argument handling so that changing a binding puts its description in front of whoever changed it.
///
/// Spelled out, unlike the filter bar's `^f` chips, because help is read once and a bar is read at a
/// glance.
pub const HELP: &str = "\
Anywhere
  Space              Play or pause
  Tab, Shift+Tab     Switch between the Library and Player views
  Esc                Close what is open, or go back to the Library
  F1, ?              Show these keys
  Ctrl+Q, Ctrl+C     Quit
  Alt+R              Cycle the repeat mode: off, track, album, queue
  Ctrl+S, Ctrl+Z     Shuffle the whole queue: by album, by track
  Alt+S, Alt+Z       Shuffle all but what is playing: by album, by track
  Ctrl+K             Clear the queue
  Ctrl+Home/End      Rest at the queue's first, last track

Play and enqueue, in the Library: P plays, E enqueues; Ctrl the selection, Alt all shown
  Ctrl+P, Ctrl+E     The selected album, or in its tracks the selected track
  Alt+P, Alt+E       Every album shown, or in its tracks the whole album

Library
  Up/Down, PgUp/PgDn, Home/End
                     Move the selection
  Enter              Open the selected album's tracks
  (any letter)       Search album titles, narrowing as you type
  Backspace          Correct the search
  Ctrl+F             Back to the search
  1, 2, 3            Pick the order, a genre, an artist
  Ctrl+W             Clear every filter

An album's tracks
  Up/Down, PgUp/PgDn, Home/End
                     Move the selection
  Enter              Play the selected track alone

A picker
  (any letter)       Search what it holds
  Up/Down            Move the selection
  Enter              Take it; its first row clears the filter it sets
  Esc                Close, changing nothing

Player
  Left/Right         Seek
  Up/Down            Volume
  Home/End           Previous, next track
  PgUp/PgDn          Previous, next album
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
        KeyCode::F(1) if matches!(focus, Focus::Help(_)) => return Some(Msg::Done),
        KeyCode::F(1) => return Some(Msg::OpenHelp),
        KeyCode::Char('r') if alt => return Some(Msg::CycleRepeat),
        KeyCode::Char('w') if ctrl => return Some(Msg::ClearFilters),
        KeyCode::Char('f') if ctrl => return Some(Msg::Search(None)),
        KeyCode::Char(c @ ('p' | 'e')) if ctrl != alt => return play_or_enqueue(view, focus, c == 'p', alt),
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
        KeyCode::Home if ctrl => return Some(Msg::RestAt(Edge::First)),
        KeyCode::End if ctrl => return Some(Msg::RestAt(Edge::Last)),
        _ => (),
    }

    // What is being typed at gets the plain keys, so a search can contain a space or a `q`.
    match focus {
        Focus::Search => return searching(key),
        Focus::Picker(_) => return picking(key),
        Focus::Tracks(_) => return in_track_menu(key),
        Focus::Help(_) => return in_help(key),
        Focus::Albums => (),
    }

    let plain = key.modifiers.is_empty();
    // A digit or `?` some layouts shift, and a letter typed in capitals, all arrive with Shift held.
    let typed = plain || key.modifiers == KeyModifiers::SHIFT;
    match key.code {
        KeyCode::Tab => return Some(Msg::Show(view.next())),
        KeyCode::BackTab => return Some(Msg::Show(view.prev())),
        KeyCode::Esc => return Some(Msg::Show(View::Library)),
        // Play/pause from anywhere, as in the GUI.
        KeyCode::Char(' ') if plain => return Some(Msg::Toggle),
        KeyCode::Char('?') if typed => return Some(Msg::OpenHelp),
        // In the order the filter bar shows them. From the player too: they bring the library up.
        KeyCode::Char('1') if typed => return Some(Msg::OpenPicker(Subject::Sort)),
        KeyCode::Char('2') if typed => return Some(Msg::OpenPicker(Subject::Genre)),
        KeyCode::Char('3') if typed => return Some(Msg::OpenPicker(Subject::Artist)),
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
            KeyCode::Enter if plain => Some(Msg::OpenTracks),
            // Typing searches, which is why no letter carries a binding of its own.
            KeyCode::Char(c) if typed => Some(Msg::Search(Some(c))),
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
            KeyCode::PageUp => Some(Msg::PrevAlbum),
            KeyCode::PageDown => Some(Msg::NextAlbum),
            _ => None,
        },
    }
}

/// Ctrl+P and Ctrl+E play and enqueue the selection; Alt+P and Alt+E (`all`) everything shown. In a
/// track menu those are its selected track and its whole album; in the browser, the selected album
/// and every album the filter lets through. Nothing in the player, where neither is on screen, nor
/// while a picker or the key list is up.
fn play_or_enqueue(view: View, focus: &Focus, play: bool, all: bool) -> Option<Msg> {
    if view == View::Player {
        return None;
    }
    let (selection, shown) = match focus {
        Focus::Tracks(_) => ((Msg::PlayTrack, Msg::QueueTrack), (Msg::PlaySelected, Msg::QueueSelected)),
        Focus::Albums | Focus::Search => ((Msg::PlaySelected, Msg::QueueSelected), (Msg::PlayShown, Msg::QueueShown)),
        Focus::Picker(_) | Focus::Help(_) => return None,
    };
    let (play_msg, enqueue_msg) = if all { shown } else { selection };
    Some(if play { play_msg } else { enqueue_msg })
}

/// In an album's track menu: the arrows walk its tracks and Enter plays the one they land on. The
/// play and enqueue chords, handled above, do the rest.
fn in_track_menu(key: KeyEvent) -> Option<Msg> {
    match key.code {
        KeyCode::Up => Some(Msg::TrackMove(-1)),
        KeyCode::Down => Some(Msg::TrackMove(1)),
        KeyCode::PageUp => Some(Msg::TrackMove(-10)),
        KeyCode::PageDown => Some(Msg::TrackMove(10)),
        KeyCode::Home => Some(Msg::TrackMove(isize::MIN)),
        KeyCode::End => Some(Msg::TrackMove(isize::MAX)),
        KeyCode::Enter if key.modifiers.is_empty() => Some(Msg::PlayTrack),
        KeyCode::Char(' ') if key.modifiers.is_empty() => Some(Msg::Toggle),
        KeyCode::Char('?') => Some(Msg::OpenHelp),
        KeyCode::Esc => Some(Msg::Done),
        _ => None,
    }
}

/// Reading the key list: the arrows scroll it, and the keys that opened it close it again.
fn in_help(key: KeyEvent) -> Option<Msg> {
    match key.code {
        KeyCode::Up => Some(Msg::HelpScroll(-1)),
        KeyCode::Down => Some(Msg::HelpScroll(1)),
        KeyCode::PageUp => Some(Msg::HelpScroll(-10)),
        KeyCode::PageDown => Some(Msg::HelpScroll(10)),
        KeyCode::Home => Some(Msg::HelpScroll(isize::MIN)),
        KeyCode::End => Some(Msg::HelpScroll(isize::MAX)),
        KeyCode::Char(' ') if key.modifiers.is_empty() => Some(Msg::Toggle),
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?' | 'q') => Some(Msg::Done),
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

#[cfg(test)]
mod test {
    use super::*;
    use crate::model::TrackMenu;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> Option<Msg> {
        press_in(&Focus::Albums, code, modifiers)
    }

    fn press_in(focus: &Focus, code: KeyCode, modifiers: KeyModifiers) -> Option<Msg> {
        key_to_msg(View::Library, focus, KeyEvent::new(code, modifiers))
    }

    /// The letter is the verb and the modifier the scope, in the browser and in a track menu alike.
    #[test]
    fn p_plays_e_enqueues_ctrl_takes_the_selection_alt_all_shown() {
        use KeyModifiers as M;
        let (p, e) = (KeyCode::Char('p'), KeyCode::Char('e'));
        let tracks = Focus::Tracks(TrackMenu { album: 0, selected: 0 });

        assert!(matches!(press_in(&Focus::Albums, p, M::CONTROL), Some(Msg::PlaySelected)));
        assert!(matches!(press_in(&Focus::Albums, e, M::CONTROL), Some(Msg::QueueSelected)));
        assert!(matches!(press_in(&Focus::Albums, p, M::ALT), Some(Msg::PlayShown)));
        assert!(matches!(press_in(&Focus::Albums, e, M::ALT), Some(Msg::QueueShown)));
        assert!(matches!(press_in(&Focus::Search, p, M::CONTROL), Some(Msg::PlaySelected)), "a chord is not typing");

        assert!(matches!(press_in(&tracks, p, M::CONTROL), Some(Msg::PlayTrack)));
        assert!(matches!(press_in(&tracks, e, M::CONTROL), Some(Msg::QueueTrack)));
        assert!(matches!(press_in(&tracks, p, M::ALT), Some(Msg::PlaySelected)));
        assert!(matches!(press_in(&tracks, e, M::ALT), Some(Msg::QueueSelected)));

        assert!(press_in(&Focus::Albums, p, M::CONTROL | M::ALT).is_none(), "both modifiers is neither scope");
        let in_player = key_to_msg(View::Player, &Focus::Albums, KeyEvent::new(p, M::ALT));
        assert!(in_player.is_none(), "the player shows neither the selection nor the albums shown");
    }

    /// The digits open the filter bar's pickers from the list, and type while a search has the keys.
    #[test]
    fn digits_open_the_pickers_unless_typing() {
        let digit = |c| KeyCode::Char(c);
        assert!(matches!(press(digit('1'), KeyModifiers::NONE), Some(Msg::OpenPicker(Subject::Sort))));
        assert!(matches!(press(digit('2'), KeyModifiers::NONE), Some(Msg::OpenPicker(Subject::Genre))));
        assert!(matches!(press(digit('3'), KeyModifiers::NONE), Some(Msg::OpenPicker(Subject::Artist))));
        assert!(matches!(press_in(&Focus::Search, digit('2'), KeyModifiers::NONE), Some(Msg::Typed('2'))));
    }
}
