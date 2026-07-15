//! Rendering the model: the library browser and the now-playing (Cover Flow) views.

use crate::model::{App, ScanState, View, album_runs, run_of};
use crate::update::Msg;
use iced::widget::{button, column, container, hover, image, responsive, row, scrollable, slider, stack, text};
use iced::{Center, Element, Fill, Theme, color};
use phonoscule_gui::background;
use phonoscule_gui::coverflow::cover_flow;
use phonoscule_gui::library::Album;
use phonoscule_gui::player;
use std::cmp::min;
use std::time::Duration;

const FA_PLAY: &str = "\u{f04b}";
const FA_PAUSE: &str = "\u{f04c}";
const FA_BACKWARD_STEP: &str = "\u{f048}";
const FA_FORWARD_STEP: &str = "\u{f051}";
const FA_PLUS: &str = "\u{2b}";

fn font_awesome_solid() -> iced::Font {
    iced::Font {
        family: iced::font::Family::Name("Font Awesome 7 Free"),
        weight: iced::font::Weight::Black,
        ..iced::Font::DEFAULT
    }
}

pub fn theme(_app: &App) -> Theme {
    Theme::Dark
}

pub fn style(_app: &App, theme: &Theme) -> iced::theme::Style {
    iced::theme::Style { background_color: iced::Color::BLACK, text_color: theme.palette().text }
}

pub fn view(app: &App) -> Element<'_, Msg> {
    let body = match app.view {
        View::Library => library_view(app),
        View::NowPlaying => now_playing_view(app),
    };
    // The nav tabs float over the top-left as bare shadowed text, so the body can use the full
    // window height (the covers may touch the top on a short window); the player floats over the
    // bottom. Both sit above the body, over the backdrop glow.
    let tabs = row![tab(app, "Library", View::Library), tab(app, "Now Playing", View::NowPlaying)].spacing(20);
    let tabs = container(tabs).padding(iced::Padding { top: 10.0, right: 12.0, bottom: 0.0, left: 12.0 });
    let mut layers: Vec<Element<'_, Msg>> = vec![background::background(app.glow).into(), body, tabs.into()];
    if let Some(bar) = player_bar(app) {
        layers.push(container(bar).center_x(Fill).align_bottom(Fill).into());
    }
    stack(layers).into()
}

/// A nav tab, styled like a track list entry: the active view's tab is the lit `active_text`,
/// the others dim.
fn tab<'a>(app: &App, label: &'a str, target: View) -> Element<'a, Msg> {
    let text = if app.view == target { active_text(label, 21.0, 1.0) } else { inactive_text(label, 21.0, 0.75) };
    button(text).style(button::text).padding(4).on_press(Msg::Show(target)).into()
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

    let controls = row![
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
        button(text(FA_FORWARD_STEP).font(font_awesome_solid()).size(18)).style(button::text).on_press(Msg::Next),
    ]
    .spacing(24)
    .align_y(Center);

    let bar = column![
        text(&current.title).size(20),
        text(format!("{} — {}", current.artist, current.album)).size(14).style(text::secondary),
        seek_bar,
        controls,
    ]
    .spacing(10)
    .padding(16)
    .align_x(Center)
    .width(Fill);

    // Frosted-glass impression: dark glass tinted by the animated accent (the same color as the
    // backdrop glow, so it crossfades with track changes), with an accent hairline along the
    // top edge as the glass highlight.
    let tinted = |k: f32, a: f32| {
        let g = app.glow;
        iced::Color { r: g.r * k, g: g.g * k, b: g.b * k, a }
    };
    let glass = tinted(0.16, 0.72);
    let highlight = tinted(0.85, 0.5);
    let hairline = container(iced::widget::Space::new()).width(Fill).height(1).style(move |_theme| container::Style {
        background: Some(iced::Background::Color(highlight)),
        ..container::Style::default()
    });
    let panel = container(bar).style(move |_theme| container::Style {
        background: Some(iced::Background::Color(glass)),
        ..container::Style::default()
    });
    Some(column![hairline, panel].into())
}

fn library_view(app: &App) -> Element<'_, Msg> {
    if app.albums.is_empty() {
        let status = match app.scan {
            ScanState::Scanning => "Scanning",
            ScanState::Complete => "No albums found under",
        };
        return container(text(format!("{status} {:?}…", app.conf.music_dir))).center(Fill).into();
    }
    const SPACING: f32 = 16.0;
    const PADDING: f32 = 16.0;
    // Room to scroll the last row out from under the floating player bar.
    let bottom_padding = if app.queue.is_empty() { PADDING } else { PLAYER_BAR_HEIGHT };
    responsive(move |size| {
        // As many columns as fit the actual width at the base card size, so albums wrap instead
        // of clipping -- then the cards stretch to use the row fully, so dropping a column
        // doesn't leave a bare right margin.
        let width = size.width - 2.0 * PADDING;
        let cols = (((width + SPACING) / (CARD_SIDE + SPACING)) as usize).max(1);
        let side = ((width - SPACING * (cols - 1) as f32) / cols as f32).floor().max(CARD_SIDE / 2.0);
        let mut grid = column![].spacing(24).padding(iced::Padding {
            // Clear the floating nav tabs at the top.
            top: TAB_BAR_HEIGHT,
            right: PADDING,
            bottom: bottom_padding,
            left: PADDING,
        });
        for (row_ix, albums) in app.albums.chunks(cols).enumerate() {
            let mut r = row![].spacing(SPACING);
            for (col_ix, album) in albums.iter().enumerate() {
                r = r.push(album_card(row_ix * cols + col_ix, album, side));
            }
            grid = grid.push(r);
        }
        // No scrollbar (it would overlap the floating player bar and can't be shortened):
        // wheel/touchpad scrolling only, like the track list overlay.
        let invisible_scrollbar = scrollable::Scrollbar::new().width(0).margin(0).scroller_width(0);
        let grid = scrollable(grid).direction(scrollable::Direction::Vertical(invisible_scrollbar)).height(Fill);
        match app.scan {
            // The scan status floats over the grid rather than claiming layout space; rescans
            // (the watcher, the periodic poll) must not shift the albums around.
            ScanState::Scanning => {
                let status = inactive_text(format!("Scanning {:?}…", app.conf.music_dir), 14.0, 0.6);
                // Sits just above the player bar (when there is one).
                let padding = iced::Padding { top: 12.0, right: 12.0, bottom: bottom_padding.max(12.0), left: 12.0 };
                stack![grid, container(status).center_x(Fill).align_bottom(Fill).padding(padding)].into()
            }
            ScanState::Complete => grid.into(),
        }
    })
    .into()
}

/// The base width of an album card in the library grid; actual cards stretch a bit beyond this
/// to fill their row.
const CARD_SIDE: f32 = 168.0;

/// Approximate height of the floating player bar, used to keep content clear of it: the library
/// grid's bottom scroll room, and how far the cover flow and track list are lifted.
const PLAYER_BAR_HEIGHT: f32 = 170.0;

/// Where the library grid starts, leaving the floating nav tabs clear with a gap below them.
const TAB_BAR_HEIGHT: f32 = 60.0;

fn album_card(ix: usize, album: &Album, side: f32) -> Element<'_, Msg> {
    let cover: Element<'_, Msg> = match &album.cover {
        Some(c) => image(c.handle.clone()).width(side).height(side).content_fit(iced::ContentFit::Cover).into(),
        None => container(text(&album.title).size(16).center())
            .width(side)
            .height(side)
            .center(side)
            .style(container::rounded_box)
            .into(),
    };
    // Action bubbles along the cover's right edge, shown only while hovering the cover.
    let play = text(FA_PLAY).font(font_awesome_solid()).size(12);
    let enqueue = text(FA_PLUS).font(font_awesome_solid()).size(14);
    let bubbles = container(
        column![
            bubble(container(play).center(Fill), Msg::PlayAlbum(ix)),
            bubble(container(enqueue).center(Fill), Msg::QueueAlbum(ix)),
        ]
        .spacing(6),
    )
    .align_right(Fill)
    .padding(8);
    let cover = hover(button(cover).padding(0).style(button::text).on_press(Msg::PlayAlbum(ix)), bubbles);
    column![cover, text(&album.title).size(15), text(&album.artist).size(13).style(text::secondary),]
        .spacing(4)
        .width(side)
        .into()
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
                background: Some(iced::Background::Color(iced::Color { a: alpha, ..iced::Color::BLACK })),
                text_color: iced::Color::WHITE,
                border: iced::border::rounded(DIAMETER / 2.0),
                ..button::Style::default()
            }
        })
        .on_press(msg)
        .into()
}

fn now_playing_view(app: &App) -> Element<'_, Msg> {
    if app.queue.is_empty() {
        return container(text("Play or queue an album from the library")).center(Fill).into();
    }
    let covers = album_runs(&app.queue).iter().map(|run| app.queue[run.start].cover.clone()).collect();
    // The reflections' floor fade must match the rendered backdrop; the covers are lifted clear
    // of the player bar, and the track list with them.
    let flow = cover_flow(covers, app.anim_pos, app.glow, PLAYER_BAR_HEIGHT, Msg::CoverClicked);

    // The track list is centered in a vertical region. That region reserves the player bar's
    // space when the list is short (so it sits comfortably above the bar), but gives that space
    // back continuously as the list grows, down to none -- so as the window shrinks the list
    // slides into and behind the translucent bar smoothly, never jumping between two placements
    // or cropping at the boundary.
    let runs = album_runs(&app.queue);
    let run_len = runs.get(run_of(&runs, app.current)).map_or(0, |r| r.len());
    let overlay = responsive(move |size| {
        const MARGIN: f32 = 24.0;
        // Per-row height estimate (title + button padding + spacing), rounded up: over-
        // estimating only adds slack, whereas under-estimating would crop the list.
        const ROW_HEIGHT: f32 = 30.0;
        let list_height = run_len as f32 * ROW_HEIGHT;
        let region = list_height.clamp(size.height - PLAYER_BAR_HEIGHT - MARGIN, size.height - MARGIN);
        let bottom = (size.height - MARGIN - region).max(0.0);
        let padding = iced::Padding { top: MARGIN, right: MARGIN, bottom, left: MARGIN };
        container(run_tracks_overlay(app)).align_right(Fill).center_y(Fill).padding(padding).into()
    });
    stack![flow, overlay].into()
}

/// The current album run's track list, overlaid in translucent text with the playing track
/// highlighted; clicking a track jumps playback there. Scrolls (without a visible scrollbar)
/// when an album has more tracks than fit.
fn run_tracks_overlay(app: &App) -> Element<'_, Msg> {
    let runs = album_runs(&app.queue);
    let run = runs.get(run_of(&runs, app.current)).cloned().unwrap_or(0..0);
    // Right-aligned: the list hugs the window's right edge, so the titles' ragged side faces
    // the content.
    let mut list = column![].spacing(2).align_x(iced::Alignment::End);
    for ix in run {
        let item = &app.queue[ix];
        let label = if ix == app.current { active_text(&item.title, 16.0, 1.0) } else { inactive_text(&item.title, 16.0, 0.6) };
        list = list.push(button(label).padding([2, 8]).style(button::text).on_press(Msg::TrackClicked(ix)));
    }
    let invisible_scrollbar = scrollable::Scrollbar::new().width(0).margin(0).scroller_width(0);
    scrollable(list).direction(scrollable::Direction::Vertical(invisible_scrollbar)).into()
}

/// Translucent text with a faked drop shadow -- the same text in translucent black, offset one
/// pixel down-right, layered underneath -- so text floating over busy content stays legible.
/// The drop shadow shared by all shadowed text: black, offset one pixel down-right.
fn drop_shadow() -> ((f32, f32), iced::Color) {
    ((1.0, 1.0), color!(0x000000, 0.7))
}

/// Bright-white text lit from the top-left by a `primary`-colored glow, over the usual drop
/// shadow -- for the active entry in a list / the active nav tab.
fn active_text<'a>(content: impl iced::widget::text::IntoFragment<'a> + Clone, size: f32, opacity: f32) -> Element<'a, Msg> {
    let glow = ((-1.0, -1.0), iced::Theme::Dark.palette().primary);
    shadowed(content, size, color!(0xffffff, opacity), &[drop_shadow(), glow])
}

/// Dim text with just the drop shadow -- for inactive entries / tabs and other quiet overlays.
fn inactive_text<'a>(content: impl iced::widget::text::IntoFragment<'a> + Clone, size: f32, opacity: f32) -> Element<'a, Msg> {
    shadowed(content, size, color!(0xf0f0f0, opacity), &[drop_shadow()])
}

/// `text` (in the default font) with drop-shadow / glow copies behind it: each `(offset, color)`
/// is a copy displaced by `offset` pixels. All layers share one bounding box, sized to cover the
/// front (at the origin) and every offset, so the stack overlays them regardless of direction.
fn shadowed<'a>(
    content: impl iced::widget::text::IntoFragment<'a> + Clone,
    size: f32,
    front: iced::Color,
    shadows: &[((f32, f32), iced::Color)],
) -> Element<'a, Msg> {
    let xs = || std::iter::once(0.0).chain(shadows.iter().map(|&((dx, _), _)| dx));
    let ys = || std::iter::once(0.0).chain(shadows.iter().map(|&((_, dy), _)| dy));
    let (min_x, max_x) = (xs().fold(0.0, f32::min), xs().fold(0.0, f32::max));
    let (min_y, max_y) = (ys().fold(0.0, f32::min), ys().fold(0.0, f32::max));
    let layer = |dx: f32, dy: f32, color: iced::Color| {
        container(text(content.clone()).size(size).style(move |_theme| text::Style { color: Some(color) }))
            .padding(iced::Padding { left: dx - min_x, top: dy - min_y, right: max_x - dx, bottom: max_y - dy })
    };
    // Shadows behind (drawn first), front on top.
    let mut layers: Vec<Element<'a, Msg>> = shadows.iter().map(|&((dx, dy), color)| layer(dx, dy, color).into()).collect();
    layers.push(layer(0.0, 0.0, front).into());
    stack(layers).into()
}

fn fmt_time(t: Duration) -> String {
    format!("{:02}:{:02}", t.as_secs() / 60, t.as_secs() % 60)
}
