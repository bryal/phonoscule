//! Key bindings: the one place a key press becomes a [`Msg`].
//!
//! Only keys no widget captured arrive here (see the keyboard subscription in `main`): the library
//! grid handles its own navigation and its selection's play and enqueue, and a focused text field
//! its typing. Kept as a single table so it can be made configurable without hunting through the
//! views.

use crate::model::{ModalKind, PickerSubject, View};
use crate::update::{MenuDir, Msg, QueueEdge, Seek, SeekDir, Zoom};
use iced::keyboard::{Key, Modifiers, key::Named};
use phonoscule::queue::{Grouping, Scope};
use std::time::Duration;

/// The bindings below, for `--help` and the in-app key list. Kept here rather than beside the
/// argument handling so that changing a binding puts its description in front of whoever changed it.
pub const HELP: &str = "\
Anywhere
  Space              Play or pause
  Tab, Shift+Tab     Switch between the Library and Player views
  Esc                Close what is open, or go back to the Library
  F1, ?              Show these keys
  Ctrl+Q             Quit
  Alt+R              Cycle the repeat mode: off, track, album, queue
  Ctrl+S, Ctrl+Z     Shuffle the whole queue: by album, by track
  Alt+S, Alt+Z       Shuffle all but what is playing: by album, by track
  Ctrl+K             Clear the queue
  Ctrl+Home/End      Rest at the queue's first, last track
  Ctrl+Plus, Ctrl+Minus
                     Zoom in, out
  Ctrl+=             Reset the zoom

Play and enqueue, in the Library: P plays, E enqueues; Ctrl the selection, Alt all shown
  Ctrl+P, Ctrl+E     The selected album, or in its tracks the selected track
  Alt+P, Alt+E       Every album shown, or in its tracks the whole album

Library
  Arrows             Move the selection
  Enter              Open the selected album's tracks
  (any letter)       Search album titles
  Ctrl+F             Back to the search
  1, 2, 3            Pick the order, a genre, an artist
  Tab, Shift+Tab     In the search or a picker, move along the filters
  Ctrl+W             Clear every filter

An album's tracks
  Up/Down            Move the selection
  Enter              Play the selected track alone

A picker
  (any letter)       Search what it holds
  Up/Down            Move the selection
  Enter              Take it; its first row clears the filter it sets

Player
  Left/Right         Seek
  Up/Down            Volume
  Home/End           Previous, next track
  PgUp/PgDn          Previous, next album
";

/// How far one arrow press scrolls the key list, in logical pixels; a page key scrolls ten times as far.
const HELP_STEP: f32 = 40.0;

/// Translates a key press into a message for the current `view`, or `None` for keys we don't bind.
/// `repeat` marks auto-repeat from a held key: continuous actions honor it (seek/scrub, walking the
/// queue), while one-shot ones don't (holding Space must not machine-gun play/pause). With a modal
/// open, it gets its own bindings and everything else is suppressed. See [`HELP`] for the bindings.
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
    // Alt/Logo chords belong to the window manager -- all but the few we bind.
    let alt_ours = modifiers.alt() && matches!(&key, Key::Character(c) if matches!(c.as_str(), "s" | "z" | "r" | "p" | "e"));
    if (modifiers.alt() && !alt_ours) || modifiers.logo() {
        return None;
    }
    let one_shot = |msg| if repeat { None } else { Some(msg) };
    // Read off the resolved characters, since `?` and, on some layouts, the digits are shifted.
    let question = !modifiers.control() && !modifiers.alt() && matches!(&modified_key, Key::Character(c) if c.as_str() == "?");

    // Over whatever is up, and closed again by the key that opened it.
    if key == Key::Named(Named::F1) || question {
        return one_shot(if modal == Some(ModalKind::Help) { Msg::CloseModal } else { Msg::OpenHelp });
    }
    if modifiers.control() && matches!(&key, Key::Character(c) if c.as_str() == "q") {
        return one_shot(Msg::Quit);
    }
    // In the order the filter bar shows them, so one picker can be swapped for the next.
    let filter_dialog = || match &modified_key {
        Key::Character(c) if !modifiers.control() && !modifiers.alt() => match c.as_str() {
            "1" => one_shot(Msg::OpenSort),
            "2" => one_shot(Msg::OpenPicker(PickerSubject::Genre)),
            "3" => one_shot(Msg::OpenPicker(PickerSubject::Artist)),
            _ => None,
        },
        _ => None,
    };
    // `(play, all)` for P or E held with exactly one of Ctrl and Alt: P plays and E enqueues, Ctrl
    // takes the selection and Alt all that is shown.
    let play_or_enqueue = match &key {
        Key::Character(c) if modifiers.alt() != modifiers.control() && !modifiers.shift() => match c.as_str() {
            "p" => Some((true, modifiers.alt())),
            "e" => Some((false, modifiers.alt())),
            _ => None,
        },
        _ => None,
    };

    match modal {
        // The track menu is modal: it gets its own bindings and everything else is suppressed.
        // Mirrors the grid's one level down -- arrows move the selection, Enter (which opened the
        // menu) plays it -- and Escape dismisses. Bare Space stays the global play/pause toggle.
        Some(ModalKind::Tracks) => {
            // One per press: holding Ctrl+E must not machine-gun the queue.
            return match (play_or_enqueue, key) {
                (Some((true, false)), _) => one_shot(Msg::MenuPlay),
                (Some((false, false)), _) => one_shot(Msg::MenuQueue),
                (Some((true, true)), _) => one_shot(Msg::MenuPlayAlbum),
                (Some((false, true)), _) => one_shot(Msg::MenuQueueAlbum),
                (None, _) if !modifiers.is_empty() => None,
                (None, Key::Named(Named::Escape)) => one_shot(Msg::CloseModal),
                (None, Key::Named(Named::ArrowUp)) => Some(Msg::MenuMove(MenuDir::Up)),
                (None, Key::Named(Named::ArrowDown)) => Some(Msg::MenuMove(MenuDir::Down)),
                (None, Key::Named(Named::Space)) => one_shot(Msg::Toggle),
                (None, Key::Named(Named::Enter)) => one_shot(Msg::MenuPlay),
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
        // field is unfocused (the field captures the first press to unfocus itself), and so do
        // the digits that swap to another picker.
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
                _ => filter_dialog(),
            };
        }
        // The sort picker: a fixed list, so no search field -- arrows move, Enter picks, and Tab
        // and the digits walk the filter inputs like the other pickers. Escape and Ctrl+W dismiss.
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
                _ => filter_dialog(),
            };
        }
        // The key list: the arrows and page keys scroll it, Escape closes it, and play/pause holds.
        Some(ModalKind::Help) => {
            return match key {
                _ if !modifiers.is_empty() => None,
                Key::Named(Named::Escape) => one_shot(Msg::CloseModal),
                Key::Named(Named::ArrowUp) => Some(Msg::HelpScroll(-HELP_STEP)),
                Key::Named(Named::ArrowDown) => Some(Msg::HelpScroll(HELP_STEP)),
                Key::Named(Named::PageUp) => Some(Msg::HelpScroll(-10.0 * HELP_STEP)),
                Key::Named(Named::PageDown) => Some(Msg::HelpScroll(10.0 * HELP_STEP)),
                Key::Named(Named::Space) => one_shot(Msg::Toggle),
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

    // Everything the filter lets through, from the library, where it is on screen. Ctrl takes the
    // grid's selection, which the grid binds itself: one reaching here found nothing selected.
    match play_or_enqueue {
        Some((true, true)) if view == View::Library => return one_shot(Msg::PlayAll),
        Some((false, true)) if view == View::Library => return one_shot(Msg::QueueAll),
        Some(_) => return None,
        None => {}
    }
    // From the player too: they bring the library up.
    if let Some(msg) = filter_dialog() {
        return Some(msg);
    }

    // View-independent navigation takes precedence over the per-view bindings below. Bare Space
    // toggles play/pause in every view; Tab cycles the views, unless the album search has focus,
    // where it walks the filter inputs (see `Msg::TabPressed`).
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
        // Starting to type searches: the first character rides along to the field, which takes over
        // (as the focused widget) for the rest. Letters can carry bindings only with a modifier,
        // exactly so that this works. Repeats welcome -- held keys repeat in a text field.
        View::Library => match (key, modifiers.alt(), modifiers.control()) {
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

#[cfg(test)]
mod test {
    use super::*;

    /// A press of `typed` with `modifiers` held, on a layout where it is the key's own character.
    fn press(view: View, modal: Option<ModalKind>, typed: &str, modifiers: Modifiers) -> Option<Msg> {
        let key = Key::Character(typed.into());
        key_to_msg(view, modal, key.clone(), key, modifiers, false)
    }

    /// The letter is the verb and the modifier the scope. Ctrl in the library is the grid's, which
    /// binds it only while it has a selection.
    #[test]
    fn p_plays_e_enqueues_ctrl_takes_the_selection_alt_all_shown() {
        let tracks = Some(ModalKind::Tracks);
        assert!(matches!(press(View::Library, None, "p", Modifiers::ALT), Some(Msg::PlayAll)));
        assert!(matches!(press(View::Library, None, "e", Modifiers::ALT), Some(Msg::QueueAll)));
        assert!(press(View::Library, None, "p", Modifiers::CTRL).is_none(), "nothing selected for the grid to play");
        assert!(press(View::Player, None, "p", Modifiers::ALT).is_none(), "the player shows none of the albums");

        assert!(matches!(press(View::Library, tracks, "p", Modifiers::CTRL), Some(Msg::MenuPlay)));
        assert!(matches!(press(View::Library, tracks, "e", Modifiers::CTRL), Some(Msg::MenuQueue)));
        assert!(matches!(press(View::Library, tracks, "p", Modifiers::ALT), Some(Msg::MenuPlayAlbum)));
        assert!(matches!(press(View::Library, tracks, "e", Modifiers::ALT), Some(Msg::MenuQueueAlbum)));
    }

    /// The digits open the filter dialogs from either view and from one another, and are read off
    /// the character typed, so a layout that shifts them still has them.
    #[test]
    fn digits_open_the_filter_dialogs() {
        let none = Modifiers::empty();
        assert!(matches!(press(View::Library, None, "1", none), Some(Msg::OpenSort)));
        assert!(matches!(press(View::Player, None, "2", none), Some(Msg::OpenPicker(PickerSubject::Genre))));
        assert!(matches!(press(View::Library, Some(ModalKind::Sort), "3", none), Some(Msg::OpenPicker(PickerSubject::Artist))));

        let azerty =
            key_to_msg(View::Library, None, Key::Character("&".into()), Key::Character("1".into()), Modifiers::SHIFT, false);
        assert!(matches!(azerty, Some(Msg::OpenSort)));
        assert!(matches!(press(View::Library, None, "a", none), Some(Msg::SearchTyped(_))), "letters still search");
    }

    /// Ctrl+Q quits from anywhere, a modal included.
    #[test]
    fn ctrl_q_quits_under_a_modal() {
        assert!(matches!(press(View::Library, Some(ModalKind::Tracks), "q", Modifiers::CTRL), Some(Msg::Quit)));
        assert!(matches!(press(View::Player, Some(ModalKind::Help), "q", Modifiers::CTRL), Some(Msg::Quit)));
    }

    /// `?` and F1 open the key list over anything, and close it again.
    #[test]
    fn the_help_keys_toggle_the_key_list() {
        let f1 =
            |modal| key_to_msg(View::Player, modal, Key::Named(Named::F1), Key::Named(Named::F1), Modifiers::empty(), false);
        assert!(matches!(press(View::Library, None, "?", Modifiers::SHIFT), Some(Msg::OpenHelp)));
        assert!(matches!(press(View::Library, Some(ModalKind::Tracks), "?", Modifiers::SHIFT), Some(Msg::OpenHelp)));
        assert!(matches!(press(View::Library, Some(ModalKind::Help), "?", Modifiers::SHIFT), Some(Msg::CloseModal)));
        assert!(matches!(f1(None), Some(Msg::OpenHelp)));
        assert!(matches!(f1(Some(ModalKind::Help)), Some(Msg::CloseModal)));
    }
}
