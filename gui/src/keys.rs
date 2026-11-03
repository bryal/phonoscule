//! Key bindings: the one place a key press becomes a [`Msg`].
//!
//! Only keys no widget captured arrive here (see the keyboard subscription in `main`): the library
//! grid handles its own navigation and selection actions, and a focused text field its typing.
//! Kept as a single table so it can be made configurable without hunting through the views.

use crate::model::{ModalKind, View};
use crate::update::{MenuDir, Msg, QueueEdge, Seek, SeekDir, Zoom};
use iced::keyboard::{Key, Modifiers, key::Named};
use phonoscule::queue::{Grouping, Scope};
use std::time::Duration;

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
