//! Drawing the frame: a header, the body of whichever view is up, and a status line.

use crate::covers;
use crate::model::{Focus, Model, Picker, ScanState, Subject, View};
use phonoscule::library::Album;
use phonoscule::player;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListState, Paragraph};
use ratatui_image::Image;
use std::time::Duration;

/// The frame: one header row, the body, one status row. No borders anywhere -- the terminal's own
/// edges are frame enough, and every row spent on decoration is a row not spent on albums.
pub fn view(frame: &mut Frame, model: &mut Model) {
    // The browser carries a filter row under the header; the player has nothing to put there.
    let filter_rows = match model.view {
        View::Library => 1,
        View::Player => 0,
    };
    let [header, filters, body, status] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(filter_rows), Constraint::Min(1), Constraint::Length(1)])
            .areas(frame.area());

    header_line(frame, model, header);
    if filter_rows > 0 {
        filter_line(frame, model, filters);
    }
    match model.view {
        View::Library => library(frame, model, body),
        View::Player => player(frame, model, body),
    }
    status_line(frame, model, status);
    // Over everything, since it is what the keyboard is talking to.
    match &model.focus {
        Focus::Picker(picker) => picker_overlay(frame, picker, body),
        Focus::Tracks(_) => track_overlay(frame, model, body),
        Focus::Albums | Focus::Search => (),
    }
}

/// An album's tracks, over the body: numbered, the selection highlighted, and a line naming what the
/// keys do -- this is where someone arrives wanting one track rather than a record.
fn track_overlay(frame: &mut Frame, model: &mut Model, body: Rect) {
    let Some(album) = model.acting_album() else { return };
    let Some(menu) = model.track_menu() else { return };
    let title = format!(" {} - {} ", album.artist, album.title);
    let rows: Vec<Line> = album
        .tracks
        .iter()
        .enumerate()
        .map(|(n, track)| {
            Line::from(vec![Span::raw(format!("{:02} ", n + 1)).fg(Color::DarkGray), Span::raw(track.title.clone())])
        })
        .collect();

    let width = body.width.clamp(24, 64);
    let height = (rows.len() as u16 + 4).min(body.height.max(5));
    let area = Rect { x: body.x + 2, y: body.y, width, height };
    frame.render_widget(Clear, area);
    let block = Block::bordered().title(title).border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [rows_area, keys_area] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let list = List::new(rows).highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    model.menu_list.select(Some(menu.selected));
    frame.render_stateful_widget(list, rows_area, &mut model.menu_list);
    let keys = "↵ play  ⌥↵ queue  ^a album  ⌥a queue album";
    frame.render_widget(Paragraph::new(Line::from(keys).fg(Color::DarkGray)), keys_area);
}

/// The filter row: the order in use, the genre and artist picked, and the album search. Each is dim
/// while it lets everything through and lit once it narrows something, so what is filtering is
/// visible at a glance -- and the key that opens each sits beside it.
fn filter_line(frame: &mut Frame, model: &Model, area: Rect) {
    let searching = matches!(model.focus, Focus::Search);
    let mut spans = vec![
        chip("^o", &model.sort.label(), true),
        Span::raw("  "),
        chip("^g", model.filter.genre.as_deref().unwrap_or("any genre"), model.filter.genre.is_some()),
        Span::raw("  "),
        chip("^t", model.filter.artist.as_deref().unwrap_or("any artist"), model.filter.artist.is_some()),
        Span::raw("  "),
        Span::raw("^a play  ").fg(Color::DarkGray),
        Span::raw("^f ").fg(Color::DarkGray),
    ];
    match (searching, model.filter.search.is_empty()) {
        (false, true) => spans.push(Span::raw("search").fg(Color::DarkGray)),
        _ => spans.push(Span::raw(model.filter.search.clone()).fg(Color::White)),
    }
    let line = Line::from(spans);
    // The terminal's own cursor marks where typing goes, rather than a block drawn to look like one.
    if searching {
        frame.set_cursor_position((area.x + line.width() as u16, area.y));
    }
    frame.render_widget(Paragraph::new(line), area);
}

/// One filter chip: the key that opens it, then its value, lit when it is narrowing something.
fn chip(key: &str, value: &str, lit: bool) -> Span<'static> {
    let value = Span::raw(format!("{key} {value}"));
    if lit { value.fg(Color::Cyan) } else { value.fg(Color::DarkGray) }
}

/// A picker, over the body: its search, then its rows, the selection highlighted. Sized to what it
/// holds, up to half the body, and anchored top-left so the album list stays partly visible.
fn picker_overlay(frame: &mut Frame, picker: &Picker, body: Rect) {
    let title = match picker.subject {
        Subject::Genre => "Genre",
        Subject::Artist => "Artist",
        Subject::Sort => "Order",
    };
    let width = body.width.clamp(20, 48);
    let height = (picker.rows() as u16 + 3).min(body.height.max(4));
    let area = Rect { x: body.x + 2, y: body.y, width, height };
    frame.render_widget(Clear, area);

    let block = Block::bordered().title(format!(" {title} ")).border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [query_area, rows_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    let query = Line::from(Span::raw(picker.query.clone()).fg(Color::White));
    frame.set_cursor_position((query_area.x + query.width() as u16, query_area.y));
    frame.render_widget(Paragraph::new(query), query_area);

    let mut rows: Vec<Line> = Vec::new();
    if picker.has_any_row() {
        rows.push(Line::from(Span::raw(format!("any {}", title.to_lowercase())).fg(Color::DarkGray)));
    }
    rows.extend(picker.matches.iter().map(|value| Line::from(value.clone())));
    let list = List::new(rows).highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(Some(picker.selected));
    frame.render_stateful_widget(list, rows_area, &mut state);
}

/// The view tabs on the left, and on the right whatever the current view wants said about itself.
fn header_line(frame: &mut Frame, model: &Model, area: Rect) {
    let mut spans = Vec::new();
    for view in View::ALL {
        spans.push(Span::raw(" "));
        let label = Span::raw(view.label());
        spans.push(if view == model.view { label.bold().fg(Color::White) } else { label.fg(Color::DarkGray) });
    }
    // The volume belongs to the application rather than to either view, so it shows in both -- and
    // only once the mixer has said what it is.
    let volume = match model.volume {
        Some(volume) => format!("vol {}%   ", (volume * 100.0).round() as u32),
        None => String::new(),
    };
    let right = match model.view {
        View::Library if model.filter.is_empty() => format!("{volume}{} ", plural(model.shown.len(), "album")),
        View::Library => format!("{volume}{} of {} albums ", model.shown.len(), model.albums.len()),
        // Albums, not tracks: the status line's "track N of M" already counts those.
        View::Player => match album_runs(model) {
            0 => volume,
            runs => format!("{volume}{} queued ", plural(runs, "album")),
        },
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
        // An empty library and a filter that matches none of it are different problems, and telling
        // someone their library is empty while it holds hundreds of albums is no help at all.
        let message = match (model.albums.is_empty(), model.scan) {
            (true, ScanState::Scanning) => format!("Scanning {}...", model.conf.music_dir.display()),
            (true, ScanState::Complete) => format!("No albums found under {}", model.conf.music_dir.display()),
            (false, _) => "No albums match the filter".to_string(),
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

    // The cover takes what the pane can spare once the byline and a few tracks have their rows.
    let for_cover = Size::new(area.width, area.height.saturating_sub(DETAILS_ROWS));
    let cover_size = covers::square(&model.covers.picker, for_cover);
    let [cover_area, rest] = Layout::vertical([Constraint::Length(cover_size.height), Constraint::Min(0)]).areas(area);
    let [cover_area, _] = Layout::horizontal([Constraint::Length(cover_size.width), Constraint::Min(0)]).areas(cover_area);

    let accent = album.accent.map(|c| Color::Rgb(channel(c.r), channel(c.g), channel(c.b)));
    let (cover_id, nearby) = (album.cover_id, nearby_covers(model));
    let mut lines =
        vec![Line::from(Span::raw(album.title.clone()).bold()), Line::from(Span::raw(album.artist.clone()).fg(Color::Cyan))];
    let year = album.year.map(|year| format!("{year:04}"));
    let genre = Some(album.genre.clone()).filter(|genre| !genre.is_empty());
    let byline: Vec<String> = [year, genre].into_iter().flatten().collect();
    if !byline.is_empty() {
        lines.push(Line::from(Span::raw(byline.join(" - ")).fg(Color::DarkGray)));
    }
    lines.push(Line::default());
    for (n, track) in album.tracks.iter().enumerate() {
        lines.push(Line::from(vec![Span::raw(format!("{:02} ", n + 1)).fg(Color::DarkGray), Span::raw(track.title.clone())]));
    }

    draw_cover(frame, model, cover_area, cover_id, accent, covers::Quality::Thumb, &nearby);
    frame.render_widget(Paragraph::new(lines), rest);
}

/// Draws the cover for `cover_id` in `area`, asking for it -- and for `also`, the ones worth having
/// ready -- at the size this pane draws them in.
///
/// Each pane asks for one quality at one size, which is what keeps a cache to a single live size: the
/// browser's preview wants thumbnails, the player wants the artwork decoded, and neither asks for the
/// other's at its own size.
///
/// Until a cover arrives, and for an album that has no artwork at all, the area is filled with the
/// album's accent colour, which the index knows long before any pixels are read. So a cover never
/// holds up a keypress; the colour is simply replaced once the artwork is ready.
fn draw_cover(
    frame: &mut Frame,
    model: &mut Model,
    area: Rect,
    cover_id: Option<u64>,
    accent: Option<Color>,
    quality: covers::Quality,
    also: &[u64],
) {
    let size = Size::new(area.width, area.height);
    if let Some(id) = cover_id {
        model.covers.want(id, quality, size);
        for &other in also {
            model.covers.want(other, quality, size);
        }
        if let Some(protocol) = model.covers.best(id, size) {
            frame.render_widget(Image::new(protocol), area);
            return;
        }
    }
    let fill = accent.unwrap_or(Color::DarkGray);
    frame.render_widget(Block::default().style(Style::default().bg(fill)), area);
}

/// The albums around the browser's cursor, whose thumbnails are worth having before they are asked
/// for: a single keypress reaches them.
fn nearby_covers(model: &Model) -> Vec<u64> {
    let row = model.selected_row();
    let first = row.saturating_sub(covers::PIN_RADIUS);
    (first..=row + covers::PIN_RADIUS).filter_map(|row| model.album_at(row)?.cover_id).collect()
}

/// An accent colour component as a terminal one.
fn channel(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn album_row(album: &Album) -> Line<'_> {
    let year = match album.year {
        // Four digits whatever the year, so a stray `(0)` from an unparseable date tag does not
        // scallop the left edge of every title beside it.
        Some(year) => format!("({year:04}) "),
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
    let cover_size = covers::square(&model.covers.picker, for_cover);
    let [cover_area, rest] = Layout::vertical([Constraint::Length(cover_size.height), Constraint::Min(0)]).areas(area);
    let [cover_area, _] = Layout::horizontal([Constraint::Length(cover_size.width), Constraint::Min(0)]).areas(cover_area);

    // The player draws a cover large, so it wants the high-resolution decode; the queue's neighbours
    // get theirs ready too, for skipping through it.
    let cover_id = model.playing().and_then(|item| model.album_of(item)).and_then(|album| album.cover_id);
    let accent = Some(accent_of(model));
    let ahead = crate::update::full_window(model);
    draw_cover(frame, model, cover_area, cover_id, accent, covers::Quality::Full, &ahead);

    let mut lines = vec![Line::default(), Line::from(Span::raw(title).bold())];
    if let Some((artist, album, year)) = byline {
        lines.push(Line::from(Span::raw(artist).fg(Color::Cyan)));
        let year = year.map(|y| format!(" ({y:04})")).unwrap_or_default();
        lines.push(Line::from(Span::raw(format!("{album}{year}")).fg(Color::DarkGray)));
    }
    frame.render_widget(Paragraph::new(lines), rest);
}

/// How many runs of tracks from one album the queue holds -- what the player lists as groups.
fn album_runs(model: &Model) -> usize {
    let mut runs = 0;
    let mut last: Option<u64> = None;
    for item in &model.queue {
        if last != Some(item.album_id) {
            runs += 1;
            last = Some(item.album_id);
        }
    }
    runs
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
    let total = total_time(model);
    let accent = accent_of(model);
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

/// The status line, holding what the view above it does not already show.
///
/// In the browser that is what is playing, since the player is not on screen. In the player it is
/// only the state around the playback the view already spells out: how it is playing, and where in
/// the queue. A warning or error the framework logged takes the whole line over -- a terminal we
/// have taken over leaves nowhere else for it, and a failed scan should not pass in silence.
fn status_line(frame: &mut Frame, model: &Model, area: Rect) {
    if let Some(entry) = model.log.back().filter(|entry| entry.level <= log::Level::Warn) {
        let color = if entry.level == log::Level::Error { Color::Red } else { Color::Yellow };
        frame.render_widget(Paragraph::new(Line::from(format!(" {}", entry.message)).fg(color)), area);
        return;
    }
    let (left, right) = match model.view {
        View::Library => (browsing_status(model), selection_status(model)),
        View::Player => (playing_status(model), queue_status(model)),
    };
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right.width() as u16 + 1)]).areas(area);
    frame.render_widget(Paragraph::new(left), left_area);
    frame.render_widget(Paragraph::new(right).right_aligned(), right_area);
}

/// While browsing: the scan's progress, else what is playing out of sight in the player.
fn browsing_status(model: &Model) -> Line<'static> {
    if model.scan == ScanState::Scanning {
        return Line::from(format!(" Scanning {}...", model.conf.music_dir.display())).fg(Color::DarkGray);
    }
    let Some(item) = model.playing() else { return Line::default() };
    let artist = model.album_of(item).map(|album| album.artist.clone()).unwrap_or_default();
    Line::from(vec![
        Span::raw(format!(" {} ", state_word(model))).fg(Color::DarkGray),
        Span::raw(item.title.clone()).fg(accent_of(model)),
        Span::raw(format!(" - {artist}")).fg(Color::DarkGray),
        Span::raw(format!("  [{}/{}]", fmt_time(model.pos), total_time(model))).fg(Color::DarkGray),
    ])
}

/// While browsing: how many tracks the selected album holds.
fn selection_status(model: &Model) -> Line<'static> {
    match model.selected_album() {
        Some(album) => Line::from(plural(album.tracks.len(), "track")).fg(Color::DarkGray),
        None => Line::default(),
    }
}

/// `1 track`, `2 tracks`.
fn plural(n: usize, noun: &str) -> String {
    let s = if n == 1 { "" } else { "s" };
    format!("{n} {noun}{s}")
}

/// In the player: how it is playing. The title, album and times are all on screen above.
fn playing_status(model: &Model) -> Line<'static> {
    let repeat = match model.repeat {
        player::Repeat::Off => String::new(),
        player::Repeat::Track => "   repeat track".to_string(),
        player::Repeat::Album => "   repeat album".to_string(),
        player::Repeat::Playlist => "   repeat queue".to_string(),
    };
    Line::from(vec![Span::raw(format!(" {}", state_word(model))).fg(accent_of(model)), Span::raw(repeat).fg(Color::DarkGray)])
}

/// In the player: where in the queue playback has reached.
fn queue_status(model: &Model) -> Line<'static> {
    if model.queue.is_empty() {
        return Line::default();
    }
    Line::from(format!("track {} of {}", model.current + 1, model.queue.len())).fg(Color::DarkGray)
}

fn state_word(model: &Model) -> &'static str {
    match model.play_state {
        player::PlayState::Playing => "Playing",
        player::PlayState::Paused => "Paused",
    }
}

/// The playing album's accent colour, for the few things tinted by it.
fn accent_of(model: &Model) -> Color {
    model
        .playing()
        .and_then(|item| model.album_of(item))
        .and_then(|album| album.accent)
        .map(|c| Color::Rgb(channel(c.r), channel(c.g), channel(c.b)))
        .unwrap_or(Color::Cyan)
}

fn total_time(model: &Model) -> String {
    model.len.map(fmt_time).unwrap_or_else(|| "-:--".into())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::covers::Quality;
    use crate::model::{Model, browser};
    use crate::update::{Edge, Msg, update};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Applies a message, discarding what it asks the event loop to do next.
    fn send(model: &mut Model, msg: Msg) {
        let _ = update(model, msg);
    }

    /// Where the browser's list starts: below the header and the filter row.
    const FIRST_LIST_ROW: u16 = 2;

    /// The row the selection highlight is drawn on, and the text of the topmost list row -- which
    /// together say where the view is scrolled to.
    fn drawn(terminal: &mut Terminal<TestBackend>, model: &mut Model) -> (u16, String) {
        terminal.draw(|frame| view(frame, model)).unwrap();
        let buffer = terminal.backend().buffer();
        let reversed = |y: u16| (0..buffer.area.width).any(|x| buffer[(x, y)].modifier.contains(Modifier::REVERSED));
        let row = (0..buffer.area.height).find(|&y| reversed(y)).expect("something must be selected");
        let top: String = (0..40).map(|x| buffer[(x, FIRST_LIST_ROW)].symbol()).collect();
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

    /// A filter that matches nothing says so, rather than claiming the library is empty -- which it
    /// is not, and which would send someone looking for a problem with their music directory.
    #[test]
    fn a_filter_matching_nothing_says_so() {
        let mut model = browser(5);
        let mut terminal = Terminal::new(TestBackend::new(120, 20)).unwrap();

        send(&mut model, Msg::Search(Some('z')));
        send(&mut model, Msg::Typed('z'));
        send(&mut model, Msg::Typed('z'));
        crate::update::reconcile(&mut model);
        assert!(model.shown.is_empty(), "nothing matches zzz");

        terminal.draw(|frame| view(frame, &mut model)).unwrap();
        let drawn: String = terminal.backend().buffer().content().iter().map(|cell| cell.symbol()).collect();
        assert!(drawn.contains("No albums match the filter"), "{drawn:?}");
        assert!(!drawn.contains("No albums found"), "the library is not empty");
    }

    /// Each pane asks for one quality only, which is what keeps each cache to a single live size. A
    /// pane that asked the other's cache at its own size would hold two sizes there, and an album both
    /// browsed and playing would have its cover re-encoded on every switch between the views.
    #[test]
    fn each_pane_asks_for_one_quality() {
        let mut model = browser(5);
        for album in &mut model.albums {
            album.cover_id = Some(album.id + 100);
        }
        // Artwork is only asked for where its file is known, so let the player have somewhere to
        // decode from.
        for id in 100..105 {
            model.covers.learn_file(id, std::sync::Arc::new("/cover.jpg".into()));
        }
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();

        terminal.draw(|frame| view(frame, &mut model)).unwrap();
        let browsing = model.covers.take_wanted();
        assert!(!browsing.is_empty(), "the browser asks for the covers it shows");
        assert!(browsing.iter().all(|r| r.quality == Quality::Thumb), "the browser asks for thumbnails only");

        // Let those loads finish. A load still in flight would hide a second request for the same
        // cover, which is exactly the case this test is here to catch.
        for request in browsing {
            let pixels = image::RgbaImage::from_pixel(8, 8, image::Rgba([9, 9, 9, 255]));
            let image = image::DynamicImage::ImageRgba8(pixels);
            let protocol = model.covers.picker.new_protocol(image, request.size, covers::resize()).unwrap();
            model.covers.absorb(covers::Load {
                cover_id: request.cover_id,
                quality: request.quality,
                size: request.size,
                generation: 0,
                protocol: Some(protocol),
            });
        }

        send(&mut model, Msg::PlaySelected);
        terminal.draw(|frame| view(frame, &mut model)).unwrap();
        let playing = model.covers.take_wanted();
        assert!(!playing.is_empty(), "the player asks for the cover it shows");
        assert!(playing.iter().all(|r| r.quality == Quality::Full), "the player asks for artwork only");
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
        assert_eq!(row, FIRST_LIST_ROW, "the first album should be selected on the first list row");
        assert!(top.starts_with("(2000) Artist 000"), "the view should be back at the top, showing {top:?}");
    }
}
