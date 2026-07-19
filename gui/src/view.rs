//! Rendering the model: the library browser and the player (Cover Flow) views.

use crate::model::{
    App, Modal, PICKER_INPUT_ID, PICKER_SCROLL_ID, Picker, PickerSubject, ScanState, TRACK_MENU_SCROLL_ID, View, album_runs,
    glow_now, run_of,
};
use crate::update::{Grouping, Msg, Promotion, Scope};
use iced::widget::{
    button, center, column, container, hover, image, mouse_area, opaque, responsive, row, scrollable, slider, stack, text,
    text_input,
};
use iced::{Center, Color, Element, Fill, Theme, color};
use phonoscule_gui::album_grid::album_grid;
use phonoscule_gui::background;
use phonoscule_gui::coverflow::{FlowCover, cover_flow};
use phonoscule_gui::library::Album;
use phonoscule_gui::player;
use std::cmp::min;
use std::time::Duration;

const FA_PLAY: &str = "\u{f04b}";
const FA_PAUSE: &str = "\u{f04c}";
const FA_BACKWARD_STEP: &str = "\u{f048}";
const FA_FORWARD_STEP: &str = "\u{f051}";
const FA_PLUS: &str = "\u{2b}";
const FA_LIST: &str = "\u{f03a}";
const FA_REPEAT: &str = "\u{f363}";
const FA_ELLIPSIS: &str = "\u{f141}";
const FA_XMARK: &str = "\u{f00d}";

fn font_awesome_solid() -> iced::Font {
    iced::Font {
        family: iced::font::Family::Name("Font Awesome 7 Free"),
        weight: iced::font::Weight::Black,
        ..iced::Font::DEFAULT
    }
}

pub fn theme(_app: &App) -> Theme {
    // The one place the current theme is chosen; a light/dark toggle would read it from `app`.
    Theme::Dark
}

pub fn style(_app: &App, theme: &Theme) -> iced::theme::Style {
    iced::theme::Style { background_color: Color::BLACK, text_color: theme.palette().text }
}

pub fn view(app: &App) -> Element<'_, Msg> {
    let body = match app.view {
        View::Library => library_view(app),
        View::Player => player_view(app),
    };
    // The top bar (nav tabs, and the library's filter tools) floats over the top as a glass
    // panel, so the body can use the full window height (the covers may touch the top on a short
    // window); the player floats over the bottom. Both sit above the body, over the backdrop glow.
    let glow = glow_now(app);
    let mut layers: Vec<Element<'_, Msg>> = vec![background::background(glow.color, glow.center).into(), body, top_bar(app)];
    if let Some(bar) = player_bar(app) {
        layers.push(container(bar).center_x(Fill).align_bottom(Fill).into());
    }
    // Modals float over everything (tabs and player bar included); the track menu and the filter
    // picker belong to the library view, the actions menu to either.
    let modal = match &app.modal {
        Some(Modal::Tracks(_)) if app.view == View::Library => track_menu_modal(app),
        Some(Modal::Actions) => Some(actions_modal(app)),
        Some(Modal::Picker(picker)) if app.view == View::Library => Some(picker_modal(picker)),
        _ => None,
    };
    if let Some(modal) = modal {
        layers.push(modal);
    }
    stack(layers).into()
}

/// A nav tab, styled like a track list entry: the active view's tab is the lit `active_text`, the others dim.
fn tab<'a>(app: &App, label: &'a str, target: View) -> Element<'a, Msg> {
    let text = if app.view == target { active_text(label, 21.0, 0.95) } else { inactive_text(label, 21.0, 0.8) };
    button(text).style(button::text).padding(4).on_press(Msg::Show(target)).into()
}

/// The floating top bar: the nav tabs and -- in the library -- the filter tools. When the window
/// is wide enough they share one row, tabs left and filter right, vertically centered on each
/// other; too narrow for that, the bar wraps to two rows (tabs above, filter tools below) rather
/// than letting the groups overlap.
fn top_bar(app: &App) -> Element<'_, Msg> {
    let tabs = || row![tab(app, "Library", View::Library), tab(app, "Player", View::Player)].spacing(20);
    let padding = iced::Padding { top: 8.0, right: 12.0, bottom: 8.0, left: 12.0 };
    if app.view != View::Library || app.albums.is_empty() {
        return glass_panel(app, container(tabs()).padding(padding).into(), Edge::Bottom);
    }
    responsive(move |size| {
        let bar: Element<'_, Msg> = if top_bar_fits(app, size.width) {
            row![tabs(), container(filter_tools(app)).align_right(Fill)].align_y(Center).into()
        } else {
            column![tabs(), container(filter_tools(app)).align_right(Fill)].spacing(6).into()
        };
        glass_panel(app, container(bar).padding(padding).into(), Edge::Bottom)
    })
    .into()
}

/// Whether the nav tabs and the filter tools fit side by side at this window width. A rough upper
/// estimate from Iosevka's fixed 0.5 em character advance plus the widgets' paddings and spacings
/// -- the two-row flip only needs to happen safely before actual overlap, not at an exact pixel.
fn top_bar_fits(app: &App, width: f32) -> bool {
    let chars = |s: &str, size: f32| s.chars().count() as f32 * 0.5 * size;
    let tabs = chars("Library", 21.0) + chars("Player", 21.0) + 2.0 * 8.0 + 20.0;
    let chips = chars(app.filter.genre.as_deref().unwrap_or("All genres"), 13.0)
        + chars(app.filter.artist.as_deref().unwrap_or("All artists"), 13.0)
        + 2.0 * 24.0;
    let filter = chips + 210.0 /* the search field */ + 3.0 * 30.0 /* clear, play, queue */ + 5.0 * 8.0;
    tabs + filter + 24.0 /* window padding */ + 24.0 /* breathing room between the groups */ <= width
}

/// The playing track's title & artist, the seek bar, and the playback controls.
fn player_bar(app: &App) -> Option<Element<'_, Msg>> {
    let current = app.queue.get(min(app.current, app.queue.len().checked_sub(1)?))?;

    // Deliberately no visual feedback while holding/dragging the slider (the grabbing mouse
    // cursor is the only hint): the bar always shows the actual playback position, so the
    // moments around a seek (position reports racing the seek command) have nothing to flash
    // back and forth. The bar simply jumps once when the player reports the new position.
    let frac = match app.len {
        Some(len) if !len.is_zero() => (app.pos.as_secs_f32() / len.as_secs_f32()).clamp(0.0, 1.0),
        _ => 0.0,
    };
    let seek_bar = row![
        text(fmt_time(app.pos)).size(13),
        slider(0.0..=1.0f32, frac, Msg::SeekChanged).step(0.001_f32).on_release(Msg::SeekReleased).width(Fill),
        text(app.len.map(fmt_time).unwrap_or_else(|| "--:--".into())).size(13),
    ]
    .spacing(12)
    .align_y(Center);

    // The repeat button cycles the mode, dimmed when off and tagged with a single glyph per mode
    // (equal widths, so the button doesn't shift as it cycles); the ellipsis opens the actions
    // menu (shuffle and friends). They flank the transport controls.
    let repeat_tag = match app.repeat {
        player::Repeat::Off => "×",
        player::Repeat::Track => "∙",
        player::Repeat::Album => "◎",
        player::Repeat::Playlist => "∞",
    };
    let alpha = match app.repeat {
        player::Repeat::Off => 0.35,
        _ => 1.0,
    };
    let repeat_style = move |theme: &Theme| text::Style { color: Some(Color { a: alpha, ..theme.palette().text }) };
    let repeat_icon = text(FA_REPEAT).font(font_awesome_solid()).size(17).style(repeat_style);
    let repeat = button(row![repeat_icon, text(repeat_tag).size(16).style(repeat_style)].spacing(4).align_y(Center))
        .style(button::text)
        .on_press(Msg::CycleRepeat);
    let actions =
        button(text(FA_ELLIPSIS).font(font_awesome_solid()).size(17)).style(button::text).on_press(Msg::OpenActionsMenu);

    let controls = row![
        repeat,
        button(text(FA_BACKWARD_STEP).font(font_awesome_solid()).size(18)).style(button::text).on_press(Msg::Prev),
        button(
            text(match app.play_state {
                player::PlayState::Playing => FA_PAUSE,
                player::PlayState::Paused => FA_PLAY,
            })
            .font(font_awesome_solid())
            .size(24)
            .width(30)
            .center(),
        )
        .style(button::text)
        .on_press(Msg::Toggle),
        button(text(FA_FORWARD_STEP).font(font_awesome_solid()).size(18))
            .style(button::text)
            .on_press(Msg::Next { repeat: false }),
        actions,
    ]
    .spacing(24)
    .align_y(Center);

    // A freshly restored queue has placeholder items with empty tags until the scan hydrates
    // them: show nothing rather than a dangling dash.
    let byline = match (current.artist.is_empty(), current.album.is_empty()) {
        (false, false) => format!("{} · {}", current.artist, current.album),
        _ => format!("{}{}", current.artist, current.album),
    };
    let bar = column![
        text(&current.title).size(20),
        text(byline)
            .size(16)
            .style(|theme: &Theme| text::Style { color: Some(theme.extended_palette().secondary.strong.color) }),
        seek_bar,
        controls,
    ]
    .spacing(5)
    .padding([14.0, 20.0])
    .align_x(Center)
    .width(Fill);

    Some(glass_panel(app, bar.into(), Edge::Top))
}

/// The edge of a [`glass_panel`] that carries its highlight hairline: the edge facing the content
/// the panel floats over (top for a bottom bar, bottom for a top bar).
enum Edge {
    Top,
    Bottom,
}

/// Frosted-glass impression for a floating bar: dark glass tinted by the current glow color (so
/// it transitions with track changes just like the backdrop), with an accent hairline along the
/// given edge as the glass highlight.
fn glass_panel<'a>(app: &App, content: Element<'a, Msg>, highlight_edge: Edge) -> Element<'a, Msg> {
    let g = glow_now(app).color;
    let tinted = |k: f32, a: f32| Color { r: g.r * k, g: g.g * k, b: g.b * k, a };
    let glass = tinted(0.18, 0.68);
    let highlight = tinted(0.70, 0.35);
    let hairline = container(iced::widget::Space::new()).width(Fill).height(1).style(move |_theme| container::Style {
        background: Some(iced::Background::Color(highlight)),
        ..container::Style::default()
    });
    let panel = container(content).width(Fill).style(move |_theme| container::Style {
        background: Some(iced::Background::Color(glass)),
        ..container::Style::default()
    });
    match highlight_edge {
        Edge::Top => column![hairline, panel].into(),
        Edge::Bottom => column![panel, hairline].into(),
    }
}

fn library_view(app: &App) -> Element<'_, Msg> {
    if app.albums.is_empty() {
        let status = match app.scan {
            ScanState::Scanning => "Scanning",
            ScanState::Complete => "No albums found under",
        };
        return container(text(format!("{status} {:?}…", app.conf.music_dir))).center(Fill).into();
    }
    // Room to scroll the last row out from under the floating player bar.
    let bottom_clearance = if app.queue.is_empty() { 16.0 } else { PLAYER_BAR_HEIGHT };
    // Responsive for the top clearance only: it must track whether the floating top bar sits in
    // one row or two (see `top_bar`), which depends on the window width.
    responsive(move |size| {
        let top_clearance = if top_bar_fits(app, size.width) { TAB_BAR_HEIGHT } else { TWO_ROW_BAR_HEIGHT };
        // The grid widget owns layout, scrolling, selection, and keyboard navigation (see
        // `album_grid`); the view supplies each card's cover element and its texts, and receives
        // whole actions back (Alt+Space queues the selection, Ctrl+Space plays it). The selection is
        // externalized into the model so it survives view switches.
        let mut grid = album_grid(Msg::PlayAlbum, Msg::QueueAlbum)
            .top_clearance(top_clearance)
            .bottom_clearance(bottom_clearance)
            .selected(app.selected, Msg::AlbumSelected)
            .on_menu(Msg::OpenTrackMenu)
            // The track menu is modal: its opaque backdrop only blocks clicks, this blocks the rest.
            .interactive(app.modal.is_none());
        // The grid shows the filtered view of the library; its cell indices (which every grid
        // message carries) are indices into `app.filtered`.
        for (cell, &ix) in app.filtered.iter().enumerate() {
            let album = &app.albums[ix];
            grid = grid.push(album_cover(cell, album), &album.title, &album.artist);
        }
        let mut layers: Vec<Element<'_, Msg>> = vec![grid.into()];
        if app.filtered.is_empty() {
            layers.push(container(inactive_text("No albums match the filter", 16.0, 0.8)).center(Fill).into());
        }
        // The scan status floats over the grid rather than claiming layout space; rescans (the
        // watcher, the periodic poll) must not shift the albums around.
        if app.scan == ScanState::Scanning {
            let status = inactive_text(format!("Scanning {:?}…", app.conf.music_dir), 14.0, 0.7);
            // Sits just above the player bar (when there is one).
            let padding = iced::Padding { top: 12.0, right: 12.0, bottom: bottom_clearance.max(12.0), left: 12.0 };
            layers.push(container(status).center_x(Fill).align_bottom(Fill).padding(padding).into());
        }
        stack(layers).into()
    })
    .into()
}

/// The library's filter tools, living in the top bar: genre and artist chips (each opening its
/// searchable picker), the fuzzy album-title search, and play/queue-all buttons acting on every
/// album currently matching, in displayed order.
fn filter_tools(app: &App) -> Element<'_, Msg> {
    let chip = |label: &str, subject: PickerSubject| {
        button(text(label.to_owned()).size(13))
            .style(|_theme, status| {
                let alpha = match status {
                    button::Status::Hovered | button::Status::Pressed => 0.9,
                    button::Status::Active | button::Status::Disabled => 0.6,
                };
                button::Style {
                    background: Some(iced::Background::Color(Color { a: alpha, ..Color::BLACK })),
                    text_color: Color::WHITE,
                    border: iced::border::rounded(13.0),
                    ..button::Style::default()
                }
            })
            .padding([5, 12])
            .on_press(Msg::OpenPicker(subject))
    };
    let genre = chip(app.filter.genre.as_deref().unwrap_or("All genres"), PickerSubject::Genre);
    let artist = chip(app.filter.artist.as_deref().unwrap_or("All artists"), PickerSubject::Artist);
    let search = text_input("Search albums…", &app.filter.search).on_input(Msg::SearchChanged).size(13).width(210);
    let enabled = !app.filtered.is_empty();
    let play = text(FA_PLAY).font(font_awesome_solid()).size(13);
    let enqueue = text(FA_PLUS).font(font_awesome_solid()).size(15);
    // Clears every filter; sits apart from the play/queue pair at the other end, and disables
    // (dimming) when there is nothing to clear.
    let clear = text(FA_XMARK).font(font_awesome_solid()).size(14);
    let clear = button(clear).style(button::text).on_press_maybe((!app.filter.is_empty()).then_some(Msg::ClearFilters));
    row![
        clear,
        genre,
        artist,
        search,
        button(play).style(button::text).on_press_maybe(enabled.then_some(Msg::PlayAll)),
        button(enqueue).style(button::text).on_press_maybe(enabled.then_some(Msg::QueueAll)),
    ]
    .spacing(8)
    .align_y(Center)
    .into()
}

/// The searchable filter picker (see [`Picker`]): its query field is focused on open, so typing
/// filters immediately -- and arrows and Enter pass through the field, so keyboard picking works
/// without leaving it. Slot 0 is the standing "(all)" entry clearing the filter. Hovering a row
/// moves the selection; a click or Enter picks. Dismissal like the other modals.
fn picker_modal(picker: &Picker) -> Element<'_, Msg> {
    let placeholder = match picker.subject {
        PickerSubject::Genre => "Search genres…",
        PickerSubject::Artist => "Search artists…",
    };
    let input = text_input(placeholder, &picker.query)
        .id(PICKER_INPUT_ID)
        .on_input(Msg::PickerQuery)
        .on_submit(Msg::PickerPick)
        .size(14);

    let mut list = column![].spacing(2);
    for (slot, value) in std::iter::once(None).chain(picker.matches.iter().map(Some)).enumerate() {
        let label: Element<'_, Msg> = match value {
            Some(value) => text(value).size(14).into(),
            None => text("(all)").size(14).style(text::secondary).into(),
        };
        let selected = slot == picker.selected;
        let entry = container(label).width(Fill).padding([4, 8]).style(move |_theme| container::Style {
            background: selected.then(|| iced::Background::Color(color!(0xffffff, 0.1))),
            border: iced::border::rounded(6.0),
            ..container::Style::default()
        });
        list = list.push(mouse_area(entry).on_enter(Msg::PickerHover(slot)).on_press(Msg::PickerChoose(slot)));
    }
    let invisible_scrollbar = scrollable::Scrollbar::new().width(0).margin(0).scroller_width(0);
    let list = scrollable(list).direction(scrollable::Direction::Vertical(invisible_scrollbar)).id(PICKER_SCROLL_ID);

    let panel = container(column![input, list].spacing(10)).padding(14).width(340).max_height(480).style(|theme: &Theme| {
        container::Style {
            background: Some(iced::Background::Color(Color { a: 0.97, ..theme.extended_palette().primary.weak.color })),
            border: iced::Border { color: color!(0xffffff, 0.1), width: 1.0, radius: 10.0.into() },
            ..container::Style::default()
        }
    });
    opaque(mouse_area(center(opaque(panel))).on_press(Msg::CloseModal))
}

/// The track menu for the album the open [`Modal::Tracks`] points at: a centered modal listing its tracks,
/// each with play/enqueue bubbles, so single tracks can be played or queued (queueing keeps the
/// menu open -- queueing several in a row is the natural flow). The inner `opaque` swallows clicks
/// on the panel itself; the `mouse_area` catches clicks outside it to dismiss (Escape dismisses
/// too, via `key_to_msg`); the outer `opaque` blocks the mouse from everything underneath.
/// `None` when no menu is open (or a rescan dropped the album from under it).
fn track_menu_modal(app: &App) -> Option<Element<'_, Msg>> {
    let menu = app.track_menu()?;
    let album = app.albums.get(menu.album)?;

    let mut list = column![].spacing(2);
    for (track_ix, track) in album.tracks.iter().enumerate() {
        let play = text(FA_PLAY).font(font_awesome_solid()).size(10);
        let enqueue = text(FA_PLUS).font(font_awesome_solid()).size(12);
        let entry = row![
            text(&track.title).size(14).width(Fill),
            bubble(container(play).center(Fill), Msg::PlayTrack { album: menu.album, track: track_ix }),
            bubble(container(enqueue).center(Fill), Msg::QueueTrack { album: menu.album, track: track_ix }),
        ]
        .spacing(6)
        .align_y(Center);
        // The selection, brightened a step above the panel; hovering a row moves it there, so the
        // mouse and the arrow keys drive the same highlight.
        let selected = track_ix == menu.selected;
        let entry = container(entry).padding([0, 4]).style(move |_theme| container::Style {
            background: selected.then(|| iced::Background::Color(color!(0xffffff, 0.1))),
            border: iced::border::rounded(6.0),
            ..container::Style::default()
        });
        list = list.push(mouse_area(entry).on_enter(Msg::MenuHover(track_ix)));
    }
    // Long albums scroll within the panel's height cap (wheel-only, like the other lists); the id
    // lets keyboard navigation snap the selection into view (see `menu_step`).
    let invisible_scrollbar = scrollable::Scrollbar::new().width(0).margin(0).scroller_width(0);
    let list = scrollable(list).direction(scrollable::Direction::Vertical(invisible_scrollbar)).id(TRACK_MENU_SCROLL_ID);

    let header = column![text(&album.title).size(17), text(&album.artist).size(13).style(text::secondary)].spacing(2);
    // A weak-primary panel rather than near-black, so the black action bubbles (and their hover
    // brightening) stay visible against it.
    let panel =
        container(column![header, list].spacing(12)).padding(16).width(420).max_height(560).style(|theme| container::Style {
            background: Some(iced::Background::Color(Color { a: 0.98, ..theme.extended_palette().background.base.color })),
            border: iced::Border {
                color: theme.extended_palette().background.strong.color.scale_alpha(0.75),
                width: 1.0,
                radius: 10.0.into(),
            },
            ..container::Style::default()
        });
    Some(opaque(mouse_area(center(opaque(panel))).on_press(Msg::CloseModal)))
}

/// The player actions menu: a centered modal with one entry per action on the playing queue
/// (shuffles now; exports and friends later), each showing its shortcut key. Same dismissal as
/// the track menu: Escape, or a click outside the panel.
fn actions_modal(app: &App) -> Element<'_, Msg> {
    let _ = app;
    let entry = |label: &'static str, hint: &'static str, msg: Msg| {
        let hint =
            text(hint).size(12).style(|theme: &Theme| text::Style { color: Some(Color { a: 0.5, ..theme.palette().text }) });
        button(row![text(label).size(15).width(Fill), hint].spacing(12).align_y(Center))
            .style(button::text)
            .width(Fill)
            .padding([6, 8])
            .on_press(msg)
    };
    let list = column![
        entry("Shuffle other albums", "alt+s", Msg::Shuffle {
            grouping: Grouping::Albums,
            scope: Scope::Others,
            promotion: Promotion::Literal
        }),
        entry("Shuffle all albums", "ctrl+s", Msg::Shuffle {
            grouping: Grouping::Albums,
            scope: Scope::All,
            promotion: Promotion::Literal
        }),
        entry("Shuffle other tracks", "alt+z", Msg::Shuffle {
            grouping: Grouping::Tracks,
            scope: Scope::Others,
            promotion: Promotion::Literal
        }),
        entry("Shuffle all tracks", "ctrl+z", Msg::Shuffle {
            grouping: Grouping::Tracks,
            scope: Scope::All,
            promotion: Promotion::Literal
        }),
        entry("Clear playlist", "ctrl+k", Msg::ClearQueue),
    ]
    .spacing(2);
    let panel = container(list).padding(12).width(280).style(|theme: &Theme| container::Style {
        background: Some(iced::Background::Color(Color { a: 0.97, ..theme.extended_palette().primary.weak.color })),
        border: iced::Border { color: color!(0xffffff, 0.1), width: 1.0, radius: 10.0.into() },
        ..container::Style::default()
    });
    opaque(mouse_area(center(opaque(panel))).on_press(Msg::CloseModal))
}

/// Approximate height of the floating player bar, used to keep content clear of it: the library
/// grid's bottom scroll room, and how far the cover flow and track list are lifted.
const PLAYER_BAR_HEIGHT: f32 = 156.0;

/// Where the library grid starts, leaving the floating top bar clear with a gap below it.
const TAB_BAR_HEIGHT: f32 = 60.0;

/// The grid's top clearance when the top bar wraps to two rows (tabs above, filter tools below).
const TWO_ROW_BAR_HEIGHT: f32 = 96.0;

/// An album's cover element for the grid: the artwork (or a fallback tile) with the floating
/// action bubbles over it. Size-agnostic -- the grid lays it out to exactly its cover square; it
/// also draws the card's texts and the selection backdrop itself.
fn album_cover(ix: usize, album: &Album) -> Element<'_, Msg> {
    let cover: Element<'_, Msg> = match &album.cover {
        Some(c) => image(c.handle.clone()).width(Fill).height(Fill).content_fit(iced::ContentFit::Cover).into(),
        // No pixels (not loaded yet, or the album has none): a tile tinted by the album's accent
        // color when the index knows it -- a zeroth level of detail below the thumbnail, so a
        // fresh launch shows the library as a color mosaic that sharpens into artwork. Dimmed, so
        // the title stays readable on any accent.
        None => {
            let accent = album.accent;
            container(text(&album.title).size(16).center())
                .center(Fill)
                .style(move |theme| match accent {
                    Some(c) => container::Style {
                        background: Some(iced::Background::Color(Color {
                            r: 0.55 * c.r,
                            g: 0.55 * c.g,
                            b: 0.55 * c.b,
                            a: 1.0,
                        })),
                        border: iced::border::rounded(2.0),
                        ..container::Style::default()
                    },
                    None => container::rounded_box(theme),
                })
                .into()
        }
    };
    // Action bubbles along the cover's right edge, shown only while hovering the cover. Entering
    // the play bubble preloads the high-res cover, hiding its decode behind the hover-to-click gap;
    // the list bubble opens the album's track menu (as do right-click and Enter -- see the grid).
    let play = text(FA_PLAY).font(font_awesome_solid()).size(12);
    let enqueue = text(FA_PLUS).font(font_awesome_solid()).size(14);
    let tracks = text(FA_LIST).font(font_awesome_solid()).size(11);
    let bubbles = container(
        column![
            mouse_area(bubble(container(play).center(Fill), Msg::PlayAlbum(ix))).on_enter(Msg::PreloadAlbum(ix)),
            bubble(container(enqueue).center(Fill), Msg::QueueAlbum(ix)),
            bubble(container(tracks).center(Fill), Msg::OpenTrackMenu(ix)),
        ]
        .spacing(6),
    )
    .align_right(Fill)
    .padding(8);
    hover(cover, bubbles)
}

/// A small round action button, floating over content.
fn bubble(label: impl Into<Element<'static, Msg>>, msg: Msg) -> Element<'static, Msg> {
    const DIAMETER: f32 = 30.0;
    button(label)
        .width(DIAMETER)
        .height(DIAMETER)
        .padding(0)
        .style(|_theme, status| {
            let alpha = match status {
                button::Status::Hovered | button::Status::Pressed => 0.9,
                button::Status::Active | button::Status::Disabled => 0.6,
            };
            button::Style {
                background: Some(iced::Background::Color(Color { a: alpha, ..Color::BLACK })),
                text_color: Color::WHITE,
                border: iced::border::rounded(DIAMETER / 2.0),
                ..button::Style::default()
            }
        })
        .on_press(msg)
        .into()
}

fn player_view(app: &App) -> Element<'_, Msg> {
    if app.queue.is_empty() {
        return container(text("Play or queue an album from the library")).center(Fill).into();
    }
    // One FlowCover per album run, at whatever detail exists yet: the accent color (known from
    // the index before any pixels), the thumbnail once loaded, and the high-res version if the
    // global cache holds it (see `ensure_hires`) -- the flow draws the best tier and sharpens as
    // better ones arrive. Only an album with nothing known at all falls to the grey placeholder.
    let covers = album_runs(&app.queue)
        .iter()
        .map(|run| {
            let item = &app.queue[run.start];
            match (&item.cover, item.accent) {
                (None, None) => None,
                (cover, accent) => Some(FlowCover {
                    id: cover.as_ref().map_or(item.album_id, |c| c.id),
                    thumb: cover.as_ref().map(|c| c.handle.clone()),
                    accent,
                    full: cover.as_ref().and_then(|c| app.hires.peek(c.id)),
                }),
            }
        })
        .collect();
    // The reflections' floor fade must match the rendered backdrop; the covers are lifted clear
    // of the player bar, and the track list with them.
    let glow = glow_now(app);
    let flow = cover_flow(covers, app.anim_pos, glow.color, glow.center, PLAYER_BAR_HEIGHT, Msg::CoverClicked);

    // The track list floats over the flow, windowed to the space between the bars. Clicks pass
    // through it (bare text, no buttons) so the covers behind stay clickable; the wheel, over the
    // list, steps the playing-track selection instead of scrolling a view (see
    // `Msg::TrackListScrolled`) -- the window follows the selection.
    let track_list_overlay = responsive(move |size| {
        let rows = ((size.height - TAB_BAR_HEIGHT - PLAYER_BAR_HEIGHT) / TRACK_ROW_HEIGHT).max(1.0) as usize;
        let list = mouse_area(run_tracks_overlay(app, rows)).on_scroll(Msg::TrackListScrolled);
        container(list)
            .align_right(Fill)
            .center_y(Fill)
            .padding(iced::Padding { top: TAB_BAR_HEIGHT, bottom: PLAYER_BAR_HEIGHT, left: 0.0, right: 10.0 })
            .into()
    });

    // The view-wide wheel: its horizontal axis walks albums like PageUp/PageDown (see
    // `Msg::PlayerScrolled`). Over the track list the inner mouse_area captures instead, and its
    // handler routes the horizontal axis the same way; clicks pass through to the flow either way.
    mouse_area(stack![flow, track_list_overlay]).on_scroll(Msg::PlayerScrolled).into()
}

/// Per-row height estimate for the player's track list overlay (text, padding, and spacing),
/// rounded up: over-estimating only trims the window, under-estimating would overflow the region.
const TRACK_ROW_HEIGHT: f32 = 28.0;

/// The current album run's track list, overlaid in translucent text with the playing track
/// highlighted. Deliberately inert to the mouse -- clicks fall through to the cover flow; only
/// the wheel acts, via the enclosing `mouse_area` (see `player_view`). When the run holds more
/// tracks than `rows`, a window of them shows, placed so the selection sits at its proportional
/// position -- at fraction f of the run, f of the way down the window -- like the pickers' snap.
fn run_tracks_overlay(app: &App, rows: usize) -> Element<'_, Msg> {
    let runs = album_runs(&app.queue);
    let run = runs.get(run_of(&runs, app.current)).cloned().unwrap_or(0..0);
    let window = if run.len() <= rows {
        run
    } else {
        let pos = app.current - run.start;
        let fraction = pos as f32 / (run.len() - 1) as f32;
        let selection_row = (fraction * (rows - 1) as f32).round() as usize;
        let first = run.start + pos.saturating_sub(selection_row).min(run.len() - rows);
        first..first + rows
    };
    let list = window.map(|ix| {
        let item = &app.queue[ix];
        let label =
            if ix == app.current { active_text(&item.title, 16.0, 0.90) } else { inactive_text(&item.title, 16.0, 0.80) };
        container(label).padding([2, 8]).into()
    });
    // Right-aligned: the list hugs the window's right edge, so the titles' ragged side faces
    // the content.
    column(list).spacing(2).align_x(iced::Alignment::End).into()
}

/// Bright-white text lit by a top-left `primary`-colored glow over the usual drop shadow (the active list entry / nav tab).
fn active_text<'a>(content: impl iced::widget::text::IntoFragment<'a> + Clone, size: f32, opacity: f32) -> Element<'a, Msg> {
    stack![
        shadow_layer(content.clone(), size, (2.0, 2.0), move |_| color!(0x000000, 0.8 * opacity)),
        shadow_layer(content.clone(), size, (-1.0, -1.0), move |theme| Color { a: 0.6 * opacity, ..theme.palette().primary }),
        shadow_layer(content, size, (0.0, 0.0), move |_| color!(0xffffff, 0.9 * opacity)),
    ]
    .into()
}

/// Dim text with just the drop shadow -- for inactive entries / tabs and other quiet overlays.
fn inactive_text<'a>(content: impl iced::widget::text::IntoFragment<'a> + Clone, size: f32, opacity: f32) -> Element<'a, Msg> {
    stack![
        shadow_layer(content.clone(), size, (1.0, 1.0), move |_| color!(0x000000, 0.8 * opacity)),
        shadow_layer(content, size, (0.0, 0.0), move |_| color!(0xf0f0f0, opacity)),
    ]
    .into()
}

/// One layer of a shadowed text: `content` as text, its color resolved against the theme, shifted
/// by `offset` pixels. The complementary +/- padding nets to zero, so the box stays content-sized
/// and every layer of a stack registers exactly; the copy simply spills its offset into the
/// whitespace around the text (fine for shadows, which are secondary).
fn shadow_layer<'a>(
    content: impl iced::widget::text::IntoFragment<'a>,
    size: f32,
    (dx, dy): (f32, f32),
    color: impl Fn(&Theme) -> Color + 'a,
) -> Element<'a, Msg> {
    container(text(content).size(size).style(move |theme: &Theme| text::Style { color: Some(color(theme)) }))
        .padding(iced::Padding { left: dx, top: dy, right: -dx, bottom: -dy })
        .into()
}

fn fmt_time(t: Duration) -> String {
    format!("{:02}:{:02}", t.as_secs() / 60, t.as_secs() % 60)
}
