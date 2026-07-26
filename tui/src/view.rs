//! Drawing the frame: a header, the body of whichever view is up, and a status line.

use crate::covers;
use crate::model::{Model, ScanState, View};
use phonoscule::library::Album;
use phonoscule::player;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, Paragraph};
use ratatui_image::StatefulImage;
use std::time::Duration;

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
        View::Player => format!("{} in queue ", model.queue.len()),
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
    // The widget scrolls only when the selection would fall outside the offset it already has, so
    // moving off the bottom row moves the highlight and leaves the view alone.
    model.list.select(Some(model.selected_row()));
    frame.render_stateful_widget(list, list_area, &mut model.list);

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

/// The player: the playing album's cover on the left with its byline beneath, the queue on the right
/// grouped by album, and the seek bar along the bottom.
fn player(frame: &mut Frame, model: &mut Model, area: Rect) {
    if model.queue.is_empty() {
        let help = "Nothing queued. Pick an album in the Library and press Enter.";
        frame.render_widget(Paragraph::new(help).centered().fg(Color::DarkGray), area);
        return;
    }
    let [body, seek] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    // Wide enough for a cover beside the queue, or the queue alone.
    let (cover_area, queue_area) = match body.width >= PREVIEW_MIN_BODY {
        true => {
            let [cover, queue] = Layout::horizontal([Constraint::Percentage(45), Constraint::Min(0)]).areas(body);
            (Some(cover), queue)
        }
        false => (None, body),
    };
    if let Some(cover_area) = cover_area {
        now_playing(frame, model, cover_area);
    }
    let lines = queue_lines(model);
    // Scrolled so the playing track stays in view as the queue advances past the bottom.
    let playing_row = playing_row(model);
    let offset = playing_row.saturating_sub(usize::from(queue_area.height) / 2);
    frame.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), queue_area);
    frame.render_widget(seek_bar(model, seek.width), seek);
}

/// The playing album's cover, as large as the pane allows, with the track and album beneath it.
fn now_playing(frame: &mut Frame, model: &mut Model, area: Rect) {
    let [_, area, _] = Layout::horizontal([Constraint::Length(1), Constraint::Min(0), Constraint::Length(2)]).areas(area);
    let Some(item) = model.playing() else { return };
    let title = item.title.clone();
    let byline = model.album_of(item).map(|album| (album.artist.clone(), album.title.clone(), album.year));

    let for_cover = Size::new(area.width, area.height.saturating_sub(4));
    let cover_size = match &model.cover {
        Some(cover) => cover.size_in(for_cover),
        None => covers::square(&model.picker, for_cover),
    };
    let [cover_area, rest] = Layout::vertical([Constraint::Length(cover_size.height), Constraint::Min(0)]).areas(area);
    let [cover_area, _] = Layout::horizontal([Constraint::Length(cover_size.width), Constraint::Min(0)]).areas(cover_area);

    match &mut model.cover {
        Some(cover) => {
            let image = StatefulImage::default().resize(covers::resize());
            frame.render_stateful_widget(image, cover_area, &mut cover.protocol);
        }
        None => frame.render_widget(Block::default().style(Style::default().bg(Color::DarkGray)), cover_area),
    }

    let mut lines = vec![Line::default(), Line::from(Span::raw(title).bold())];
    if let Some((artist, album, year)) = byline {
        lines.push(Line::from(Span::raw(artist).fg(Color::Cyan)));
        let year = year.map(|y| format!(" ({y})")).unwrap_or_default();
        lines.push(Line::from(Span::raw(format!("{album}{year}")).fg(Color::DarkGray)));
    }
    frame.render_widget(Paragraph::new(lines), rest);
}

/// Which line of the queue listing the playing track sits on, album bylines and blanks included.
fn playing_row(model: &Model) -> usize {
    let mut row = 0;
    let mut run: Option<u64> = None;
    for (ix, item) in model.queue.iter().enumerate() {
        if run != Some(item.album_id) {
            run = Some(item.album_id);
            row += if ix > 0 { 2 } else { 1 };
        }
        if ix == model.current {
            return row;
        }
        row += 1;
    }
    row
}

/// The seek bar: elapsed and total either side of a bar filled to the playing position, tinted with
/// the album's accent.
fn seek_bar(model: &Model, width: u16) -> Paragraph<'static> {
    let elapsed = fmt_time(model.pos);
    let total = model.len.map(fmt_time).unwrap_or_else(|| "-:--".into());
    let accent = model
        .playing()
        .and_then(|item| model.album_of(item))
        .and_then(|album| album.accent)
        .map(|c| Color::Rgb(channel(c.r), channel(c.g), channel(c.b)))
        .unwrap_or(Color::Cyan);
    // The bar takes what the two timestamps and their spacing leave.
    let labels = elapsed.len() + total.len() + 4;
    let bar = usize::from(width).saturating_sub(labels);
    let done = match model.len.map(|len| len.as_secs_f64()).filter(|len| *len > 0.0) {
        Some(len) => ((model.pos.as_secs_f64() / len) * bar as f64).round().min(bar as f64) as usize,
        None => 0,
    };
    Paragraph::new(Line::from(vec![
        Span::raw(format!(" {elapsed} ")).fg(Color::DarkGray),
        Span::raw("━".repeat(done)).fg(accent),
        Span::raw("━".repeat(bar.saturating_sub(done))).fg(Color::DarkGray),
        Span::raw(format!(" {total}")).fg(Color::DarkGray),
    ]))
}

/// The queue as an album-grouped track list: a byline per run of tracks from one album, its tracks
/// beneath it, and a mark on the one playing.
fn queue_lines(model: &Model) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut run: Option<u64> = None;
    for (ix, item) in model.queue.iter().enumerate() {
        if run != Some(item.album_id) {
            run = Some(item.album_id);
            let byline = match model.album_of(item) {
                Some(album) => format!("{} - {}", album.artist, album.title),
                None => "Unknown album".to_string(),
            };
            if ix > 0 {
                lines.push(Line::default());
            }
            lines.push(Line::from(Span::raw(byline).bold().fg(Color::Cyan)));
        }
        let playing = ix == model.current;
        let mark = if playing { " > " } else { "   " };
        let title = Span::raw(item.title.clone());
        lines.push(Line::from(vec![
            Span::raw(mark).fg(Color::DarkGray),
            if playing { title.bold() } else { title.fg(Color::Gray) },
        ]));
    }
    lines
}

/// `m:ss`, or `h:mm:ss` for the rare album-length track.
fn fmt_time(t: Duration) -> String {
    let secs = t.as_secs();
    match secs / 3600 {
        0 => format!("{}:{:02}", secs / 60, secs % 60),
        hours => format!("{hours}:{:02}:{:02}", (secs / 60) % 60, secs % 60),
    }
}

/// The status line: what is playing, and the scan's progress while it runs. A warning or error the
/// framework logged takes the line over instead -- there is nowhere else for it to go in a terminal
/// we have taken over, and a silently failed scan is worse than a cluttered status line.
fn status_line(model: &Model) -> Paragraph<'static> {
    if let Some(entry) = model.log.back().filter(|entry| entry.level <= log::Level::Warn) {
        let color = if entry.level == log::Level::Error { Color::Red } else { Color::Yellow };
        return Paragraph::new(Line::from(format!(" {}", entry.message)).fg(color));
    }
    let Some(item) = model.playing() else {
        let left = match model.scan {
            ScanState::Scanning => format!(" Scanning {}...", model.conf.music_dir.display()),
            ScanState::Complete => String::new(),
        };
        return Paragraph::new(Line::from(left).fg(Color::DarkGray));
    };
    let state = match model.play_state {
        player::PlayState::Playing => "Playing",
        player::PlayState::Paused => "Paused",
    };
    let byline = match model.album_of(item) {
        Some(album) => format!("{} - {}", album.artist, album.title),
        None => String::new(),
    };
    let elapsed = fmt_time(model.pos);
    let total = model.len.map(fmt_time).unwrap_or_else(|| "-".into());
    // Tinted with the playing album's accent, the one thing the GUI's glow can be in a terminal.
    let accent = model
        .album_of(item)
        .and_then(|album| album.accent)
        .map(|c| Color::Rgb(channel(c.r), channel(c.g), channel(c.b)))
        .unwrap_or(Color::Cyan);
    let repeat = match model.repeat {
        player::Repeat::Off => "",
        player::Repeat::Track => "  rpt:track",
        player::Repeat::Album => "  rpt:album",
        player::Repeat::Playlist => "  rpt:queue",
    };
    Paragraph::new(Line::from(vec![
        Span::raw(format!(" {state}: ")).fg(Color::DarkGray),
        Span::raw(item.title.clone()).fg(accent).bold(),
        Span::raw(format!("  {byline}")).fg(Color::DarkGray),
        Span::raw(format!("  [{elapsed}/{total}]")).fg(Color::DarkGray),
        Span::raw(repeat).fg(Color::DarkGray),
    ]))
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::model::{Model, browser};
    use crate::update::{Edge, Msg, update};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Applies a message, discarding what it asks the event loop to do next.
    fn send(model: &mut Model, msg: Msg) {
        let _ = update(model, msg);
    }

    /// The row the selection highlight is drawn on, and the text of the topmost list row -- which
    /// together say where the view is scrolled to.
    fn drawn(terminal: &mut Terminal<TestBackend>, model: &mut Model) -> (u16, String) {
        terminal.draw(|frame| view(frame, model)).unwrap();
        let buffer = terminal.backend().buffer();
        let reversed = |y: u16| (0..buffer.area.width).any(|x| buffer[(x, y)].modifier.contains(Modifier::REVERSED));
        let row = (0..buffer.area.height).find(|&y| reversed(y)).expect("something must be selected");
        // Row 0 is the header, so the list starts at row 1.
        let top: String = (0..40).map(|x| buffer[(x, 1)].symbol()).collect();
        (row, top)
    }

    /// Moving up off the bottom row must move the highlight, not scroll the list: the view only
    /// follows the selection when the selection would otherwise leave it.
    #[test]
    fn moving_up_from_the_bottom_row_leaves_the_view_alone() {
        let mut model = browser(100);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();

        // Walk past the bottom of the viewport, so the list has scrolled.
        send(&mut model, Msg::Select(60));
        let (bottom_row, top_before) = drawn(&mut terminal, &mut model);
        assert!(bottom_row > 30, "the selection should have reached the bottom of the view, not row {bottom_row}");

        send(&mut model, Msg::Select(-1));
        let (row, top_after) = drawn(&mut terminal, &mut model);
        assert_eq!(row, bottom_row - 1, "the highlight should move up one row");
        assert_eq!(top_after, top_before, "the view should not have scrolled");
    }

    /// Walking back to the top scrolls the view with the selection, once it has nowhere else to go.
    #[test]
    fn walking_off_the_top_scrolls_back() {
        let mut model = browser(100);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();

        send(&mut model, Msg::Select(60));
        drawn(&mut terminal, &mut model);
        send(&mut model, Msg::SelectEdge(Edge::First));
        let (row, top) = drawn(&mut terminal, &mut model);
        assert_eq!(row, 1, "the first album should be selected on the first list row");
        assert!(top.starts_with("(2000) Artist 000"), "the view should be back at the top, showing {top:?}");
    }
}
