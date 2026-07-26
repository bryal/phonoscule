//! Drawing the frame: a header, the body of whichever view is up, and a status line.

use crate::model::{Model, ScanState, View};
use phonoscule::library::Album;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListState, Paragraph};

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

/// The album browser: one row per album, `(year) artist - title`.
fn library(frame: &mut Frame, model: &mut Model, area: Rect) {
    if model.shown.is_empty() {
        let message = match model.scan {
            ScanState::Scanning => format!("Scanning {}...", model.conf.music_dir.display()),
            ScanState::Complete => format!("No albums found under {}", model.conf.music_dir.display()),
        };
        frame.render_widget(Paragraph::new(message).centered().fg(Color::DarkGray), area);
        return;
    }
    let rows: Vec<Line> = model.shown.iter().map(|&ix| album_row(&model.albums[ix])).collect();
    let list = List::new(rows).highlight_style(Style::default().add_modifier(Modifier::REVERSED)).highlight_symbol("");
    // The list widget owns the scroll offset, so it keeps the selection in view for us.
    let mut state = ListState::default().with_selected(Some(model.selected));
    frame.render_stateful_widget(list, area, &mut state);
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
