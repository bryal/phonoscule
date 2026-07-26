//! Key bindings: the one place a key press becomes a [`Msg`].
//!
//! Modifier-carrying chords only, never a bare letter -- letters are reserved for type-to-search in
//! the browser. Kept as a single table so it can be made configurable without hunting through the
//! views.

use crate::model::{Focus, Subject, View};
use crate::update::{Edge, Msg};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// How far a single seek key press moves, in seconds.
const SEEK_STEP: i64 = 5;

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
        _ => (),
    }

    // What is being typed at gets the plain keys, so a search can contain a space or a `q`.
    match focus {
        Focus::Search => return searching(key),
        Focus::Picker(_) => return picking(key),
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
            // Enter plays the selection, as the GUI's Ctrl+Space does -- which a terminal cannot be
            // relied on to deliver at all. Alt+Enter queues it, matching the GUI's Alt+Space.
            KeyCode::Enter if alt => Some(Msg::QueueSelected),
            // Everything the filter lets through: Ctrl+A plays it, Alt+A appends it. The GUI's pair
            // is Ctrl+Enter and Alt+Enter, but Alt+Enter queues the selection here, and terminals
            // take Ctrl+Enter for themselves -- Ghostty toggles full screen with it.
            KeyCode::Char('a') if ctrl => Some(Msg::PlayShown),
            KeyCode::Char('a') if alt => Some(Msg::QueueShown),
            KeyCode::Enter => Some(Msg::PlaySelected),
            // Typing searches, which is why no letter carries a binding of its own.
            KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => Some(Msg::Search(Some(c))),
            // As does rubbing out, for correcting a search after the keys have gone back to the list.
            KeyCode::Backspace => Some(Msg::Rubout),
            _ => None,
        },
        View::Player => match key.code {
            KeyCode::Left => Some(Msg::Seek(-SEEK_STEP)),
            KeyCode::Right => Some(Msg::Seek(SEEK_STEP)),
            KeyCode::Home => Some(Msg::Prev),
            KeyCode::End => Some(Msg::Next),
            _ => None,
        },
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
