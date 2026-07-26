//! Key bindings: the one place a key press becomes a [`Msg`].
//!
//! Modifier-carrying chords only, never a bare letter -- letters are reserved for type-to-search in
//! the browser. Kept as a single table so it can be made configurable without hunting through the
//! views.

use crate::model::View;
use crate::update::{Edge, Msg};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// How far a single seek key press moves, in seconds.
const SEEK_STEP: i64 = 5;

/// The message a key press means in the given view, or `None` if it is bound to nothing.
pub fn key_to_msg(view: View, key: KeyEvent) -> Option<Msg> {
    // Terminals that speak the kitty keyboard protocol report releases and repeats too; only
    // presses are bindings.
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    // View-independent bindings take precedence over the per-view ones below.
    match key.code {
        KeyCode::Char('c') if ctrl => return Some(Msg::Quit),
        KeyCode::Char('q') if ctrl => return Some(Msg::Quit),
        KeyCode::Tab => return Some(Msg::Show(view.next())),
        KeyCode::BackTab => return Some(Msg::Show(view.prev())),
        // Play/pause from anywhere, as in the GUI.
        KeyCode::Char(' ') if key.modifiers.is_empty() => return Some(Msg::Toggle),
        KeyCode::Char('r') if alt => return Some(Msg::CycleRepeat),
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
            KeyCode::Enter => Some(Msg::PlaySelected),
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
