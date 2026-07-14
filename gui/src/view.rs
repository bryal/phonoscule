//! Rendering the model: the library browser and the now-playing (Cover Flow) views.

use crate::model::{App, ScanState, View, album_runs, run_of};
use crate::update::Msg;
use iced::widget::{button, column, container, hover, image, responsive, row, scrollable, slider, stack, text};
use iced::{Center, Element, Fill, Theme};
use phonoscule_gui::coverflow::cover_flow;
use phonoscule_gui::library::Album;
use phonoscule_gui::player;
use std::cmp::min;
use std::time::Duration;

pub fn theme(_app: &App) -> Theme {
    Theme::Dark
}

pub fn style(_app: &App, theme: &Theme) -> iced::theme::Style {
    iced::theme::Style { background_color: iced::Color::BLACK, text_color: theme.palette().text }
}

pub fn view(app: &App) -> Element<'_, Msg> {
    let tab = |label, target| {
        let b = button(text(label).size(14)).on_press(Msg::Show(target));
        if app.view == target { b } else { b.style(button::secondary) }
    };
    let top = row![tab("Library", View::Library), tab("Now Playing", View::NowPlaying)].spacing(8).padding(8);
    let body = match app.view {
        View::Library => library_view(app),
        View::NowPlaying => now_playing_view(app),
    };
    column![top, body].into()
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
    responsive(move |size| {
        // As many columns as fit the actual width at the base card size, so albums wrap instead
        // of clipping -- then the cards stretch to use the row fully, so dropping a column
        // doesn't leave a bare right margin.
        let width = size.width - 2.0 * PADDING;
        let cols = (((width + SPACING) / (CARD_SIDE + SPACING)) as usize).max(1);
        let side = ((width - SPACING * (cols - 1) as f32) / cols as f32).floor().max(CARD_SIDE / 2.0);
        let mut grid = column![].spacing(24).padding(PADDING);
        for (row_ix, albums) in app.albums.chunks(cols).enumerate() {
            let mut r = row![].spacing(SPACING);
            for (col_ix, album) in albums.iter().enumerate() {
                r = r.push(album_card(row_ix * cols + col_ix, album, side));
            }
            grid = grid.push(r);
        }
        let grid = scrollable(grid).height(Fill);
        match app.scan {
            // The scan status floats over the grid rather than claiming layout space; rescans
            // (the watcher, the periodic poll) must not shift the albums around.
            ScanState::Scanning => {
                let status = shadowed_text(format!("Scanning {:?}…", app.conf.music_dir), 14.0, |_| {
                    iced::Color { a: 0.6, ..iced::Color::WHITE }
                });
                stack![grid, container(status).center_x(Fill).align_bottom(Fill).padding(12)].into()
            }
            ScanState::Complete => grid.into(),
        }
    })
    .into()
}

/// The base width of an album card in the library grid; actual cards stretch a bit beyond this
/// to fill their row.
const CARD_SIDE: f32 = 168.0;

fn album_card(ix: usize, album: &Album, side: f32) -> Element<'_, Msg> {
    let cover: Element<'_, Msg> = match &album.cover {
        Some(c) => image(c.handle.clone())
            .width(side)
            .height(side)
            .content_fit(iced::ContentFit::Cover)
            .into(),
        None => container(text(&album.title).size(16).center())
            .width(side)
            .height(side)
            .center(side)
            .style(container::rounded_box)
            .into(),
    };
    // Action bubbles along the cover's right edge, shown only while hovering the cover.
    let play = text("▶").size(13).center();
    let enqueue = text("+").size(19).font(iced::Font { weight: iced::font::Weight::Bold, ..iced::Font::DEFAULT }).center();
    let bubbles = container(
        column![
            // Nudged right: a right-pointing triangle looks left-leaning when geometrically centered.
            bubble(container(play).center(Fill).padding(iced::Padding { left: 2.0, ..iced::Padding::ZERO }), Msg::PlayAlbum(ix)),
            bubble(container(enqueue).center(Fill), Msg::QueueAlbum(ix)),
        ]
        .spacing(6),
    )
    .align_right(Fill)
    .padding(8);
    let cover = hover(button(cover).padding(0).style(button::text).on_press(Msg::PlayAlbum(ix)), bubbles);
    column![
        cover,
        text(&album.title).size(14),
        text(&album.artist).size(12).style(text::secondary),
    ]
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
    let current = &app.queue[min(app.current, app.queue.len() - 1)];

    let covers =
        album_runs(&app.queue).iter().map(|run| app.queue[run.start].cover.clone()).collect();
    let flow = cover_flow(covers, app.anim_pos, Msg::CoverClicked);

    let shown_pos = match (app.seek_drag, app.len) {
        (Some(frac), Some(len)) => len.mul_f32(frac.clamp(0.0, 1.0)),
        _ => app.pos,
    };
    let frac = match (app.seek_drag, app.len) {
        (Some(frac), _) => frac,
        (None, Some(len)) if !len.is_zero() => (app.pos.as_secs_f32() / len.as_secs_f32()).clamp(0.0, 1.0),
        _ => 0.0,
    };
    let seek_bar = row![
        text(fmt_time(shown_pos)).size(12),
        slider(0.0..=1.0f32, frac, Msg::SeekChanged)
            .step(0.001_f32)
            .on_release(Msg::SeekReleased)
            .width(Fill),
        text(app.len.map(fmt_time).unwrap_or_else(|| "--:--".into())).size(12),
    ]
    .spacing(12)
    .align_y(Center);

    let controls = row![
        button(text("⏮").size(18)).style(button::text).on_press(Msg::Prev),
        button(
            text(match app.play_state {
                player::PlayState::Playing => "⏸",
                player::PlayState::Paused => "▶",
            })
            .size(24),
        )
        .style(button::text)
        .on_press(Msg::Toggle),
        button(text("⏭").size(18)).style(button::text).on_press(Msg::Next),
    ]
    .spacing(24)
    .align_y(Center);

    let body = column![
        flow,
        text(&current.title).size(20),
        text(format!("{} — {}", current.artist, current.album)).size(14).style(text::secondary),
        seek_bar,
        controls,
    ]
    .spacing(10)
    .padding(16)
    .align_x(Center);

    stack![body, container(run_tracks_overlay(app)).align_right(Fill).center_y(Fill).padding(24)].into()
}

/// The current album run's track list, overlaid in translucent text with the playing track
/// highlighted; clicking a track jumps playback there. Scrolls (without a visible scrollbar)
/// when an album has more tracks than fit.
fn run_tracks_overlay(app: &App) -> Element<'_, Msg> {
    let runs = album_runs(&app.queue);
    let run = runs.get(run_of(&runs, app.current)).cloned().unwrap_or(0..0);
    let mut list = column![].spacing(2);
    for ix in run {
        let item = &app.queue[ix];
        let playing = ix == app.current;
        let label = shadowed_text(&item.title, 16.0, move |theme| {
            if playing { theme.palette().primary } else { iced::Color { a: 0.6, ..iced::Color::WHITE } }
        });
        list = list.push(button(label).padding([2, 8]).style(button::text).on_press(Msg::TrackClicked(ix)));
    }
    let invisible_scrollbar = scrollable::Scrollbar::new().width(0).margin(0).scroller_width(0);
    scrollable(list).direction(scrollable::Direction::Vertical(invisible_scrollbar)).into()
}

/// Translucent text with a faked drop shadow -- the same text in translucent black, offset one
/// pixel down-right, layered underneath -- so text floating over busy content stays legible.
fn shadowed_text<'a>(
    content: impl iced::widget::text::IntoFragment<'a> + Clone,
    size: f32,
    color: impl Fn(&Theme) -> iced::Color + 'a,
) -> Element<'a, Msg> {
    let front = text(content.clone())
        .size(size)
        .style(move |theme: &Theme| text::Style { color: Some(color(theme)) });
    let shadow =
        text(content).size(size).style(|_theme| text::Style { color: Some(iced::Color { a: 0.7, ..iced::Color::BLACK }) });
    stack![
        container(shadow).padding(iced::Padding { top: 1.0, left: 1.0, right: 0.0, bottom: 0.0 }),
        container(front).padding(iced::Padding { top: 0.0, left: 0.0, right: 1.0, bottom: 1.0 }),
    ]
    .into()
}

fn fmt_time(t: Duration) -> String {
    format!("{:02}:{:02}", t.as_secs() / 60, t.as_secs() % 60)
}
