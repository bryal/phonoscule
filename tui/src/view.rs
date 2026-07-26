//! Drawing the frame: a header, the body of whichever view is up, and a status line.

use crate::covers;
use crate::model::{Model, ScanState, View};
use phonoscule::library::Album;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListState, Paragraph};
use ratatui_image::StatefulImage;

/// The frame: one header row, the body, one status row. No borders anywhere -- the terminal's own
/// edges are frame enough, and every row spent on decoration is a row not spent on albums.
pub fn view(frame: &mut Frame, model: &mut Model) {
    let [header, body, status] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());

    header_line(frame, model, header);
    match model.view {
        View::Library => library(frame, model, body),
        View::Player => player(frame, model, body),
    }
    frame.render_widget(status_line(model), status);
}

/// The view tabs on the left, and on the right whatever the current view wants said about itself.
fn header_line(frame: &mut Frame, model: &Model, area: Rect) {
    let mut spans = Vec::new();
    for view in View::ALL {
        spans.push(Span::raw(" "));
        let label = Span::raw(view.label());
        spans.push(if view == model.view { label.bold().fg(Color::White) } else { label.fg(Color::DarkGray) });
    }
    let right = match model.view {
        View::Library => format!("{} albums ", model.shown.len()),
        View::Player => String::new(),
    };
    let [tabs, info] = Layout::horizontal([Constraint::Min(0), Constraint::Length(right.len() as u16)]).areas(area);
    frame.render_widget(Paragraph::new(Line::from(spans)), tabs);
    frame.render_widget(Paragraph::new(right).fg(Color::DarkGray), info);
}

/// The width below which the browser drops the preview pane and gives the whole body to the list --
/// a narrow terminal is better off reading titles than squinting at a thumbnail.
const PREVIEW_MIN_BODY: u16 = 64;

/// Rows the preview keeps below the cover: the title, artist and byline, a blank, and a few tracks.
/// The cover gets the rest.
const DETAILS_ROWS: u16 = 9;

/// How wide the preview pane gets: a third of the body, so a wide terminal shows a bigger cover,
/// bounded so it neither shrinks below a legible track list nor crowds out the album titles.
fn preview_width(body: u16) -> u16 {
    (body / 3).clamp(30, 48)
}

/// The album browser: the list on the left, the selected album's cover and details on the right.
fn library(frame: &mut Frame, model: &mut Model, area: Rect) {
    if model.shown.is_empty() {
        let message = match model.scan {
            ScanState::Scanning => format!("Scanning {}...", model.conf.music_dir.display()),
            ScanState::Complete => format!("No albums found under {}", model.conf.music_dir.display()),
        };
        frame.render_widget(Paragraph::new(message).centered().fg(Color::DarkGray), area);
        return;
    }
    let (list_area, preview_area) = match area.width >= PREVIEW_MIN_BODY {
        true => {
            let [list, preview] =
                Layout::horizontal([Constraint::Min(0), Constraint::Length(preview_width(area.width))]).areas(area);
            (list, Some(preview))
        }
        false => (area, None),
    };

    let rows: Vec<Line> = model.shown.iter().map(|&ix| album_row(&model.albums[ix])).collect();
    let list = List::new(rows).highlight_style(Style::default().add_modifier(Modifier::REVERSED)).highlight_symbol("");
    // The list widget owns the scroll offset, so it keeps the selection in view for us.
    let mut state = ListState::default().with_selected(Some(model.selected_row()));
    frame.render_stateful_widget(list, list_area, &mut state);

    if let Some(preview_area) = preview_area {
        preview(frame, model, preview_area);
    }
}

/// The selected album: its cover, then its byline, then its tracks. Indented one column off the
/// list, with no divider -- the cover is edge enough.
fn preview(frame: &mut Frame, model: &mut Model, area: Rect) {
    let [_, area] = Layout::horizontal([Constraint::Length(2), Constraint::Min(0)]).areas(area);
    let Some(album) = model.selected_album() else { return };

    // The cover takes what the pane can spare once the byline and a few tracks have their rows, as
    // large as that allows.
    let for_cover = Size::new(area.width, area.height.saturating_sub(DETAILS_ROWS));
    let cover_size = match &model.cover {
        Some(cover) => cover.size_in(for_cover),
        None => covers::square(&model.picker, for_cover),
    };
    let [cover_area, rest] = Layout::vertical([Constraint::Length(cover_size.height), Constraint::Min(0)]).areas(area);
    let [cover_area, _] = Layout::horizontal([Constraint::Length(cover_size.width), Constraint::Min(0)]).areas(cover_area);

    let accent = album.accent.map(|c| Color::Rgb(channel(c.r), channel(c.g), channel(c.b)));
    let mut lines =
        vec![Line::from(Span::raw(album.title.clone()).bold()), Line::from(Span::raw(album.artist.clone()).fg(Color::Cyan))];
    let year = album.year.map(|year| year.to_string());
    let genre = Some(album.genre.clone()).filter(|genre| !genre.is_empty());
    let byline: Vec<String> = [year, genre].into_iter().flatten().collect();
    if !byline.is_empty() {
        lines.push(Line::from(Span::raw(byline.join(" - ")).fg(Color::DarkGray)));
    }
    lines.push(Line::default());
    for (n, track) in album.tracks.iter().enumerate() {
        lines.push(Line::from(vec![Span::raw(format!("{:02} ", n + 1)).fg(Color::DarkGray), Span::raw(track.title.clone())]));
    }

    match &mut model.cover {
        Some(cover) => {
            let image = StatefulImage::default().resize(covers::resize());
            frame.render_stateful_widget(image, cover_area, &mut cover.protocol);
        }
        // No artwork loaded (still scanning, or the album has none): a block of the album's accent
        // colour, which the index knows before any pixels are read. The same zeroth level of detail
        // the GUI's grid shows.
        None => {
            let fill = accent.unwrap_or(Color::DarkGray);
            frame.render_widget(Block::default().style(Style::default().bg(fill)), cover_area);
        }
    }
    frame.render_widget(Paragraph::new(lines), rest);
}

/// An accent colour component as a terminal one.
fn channel(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn album_row(album: &Album) -> Line<'_> {
    let year = match album.year {
        Some(year) => format!("({year}) "),
        None => "       ".to_string(),
    };
    Line::from(vec![
        Span::raw(year).fg(Color::DarkGray),
        Span::raw(&album.artist).fg(Color::Cyan),
        Span::raw(" - ").fg(Color::DarkGray),
        Span::raw(&album.title),
    ])
}

fn player(frame: &mut Frame, _model: &mut Model, area: Rect) {
    frame.render_widget(Paragraph::new("Nothing playing").centered().fg(Color::DarkGray), area);
}

/// The status line: what is playing, and the scan's progress while it runs. A warning or error the
/// framework logged takes the line over instead -- there is nowhere else for it to go in a terminal
/// we have taken over, and a silently failed scan is worse than a cluttered status line.
fn status_line(model: &Model) -> Paragraph<'static> {
    if let Some(entry) = model.log.back().filter(|entry| entry.level <= log::Level::Warn) {
        let color = if entry.level == log::Level::Error { Color::Red } else { Color::Yellow };
        return Paragraph::new(Line::from(format!(" {}", entry.message)).fg(color));
    }
    let left = match model.scan {
        ScanState::Scanning => "Scanning...".to_string(),
        ScanState::Complete => match model.selected_album() {
            Some(album) => format!("{} tracks", album.tracks.len()),
            None => String::new(),
        },
    };
    Paragraph::new(Line::from(format!(" {left}")).fg(Color::DarkGray))
}
