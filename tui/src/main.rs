//! Phonoscule TUI: an album-focused music player for the terminal.
//!
//! A sketch: it draws a static now-playing frame from dummy state, to prove out cover art in the
//! terminal (see [`ratatui_image`]). None of it is wired to the player yet.

use crossterm::event::{self, Event, KeyCode};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Gauge, Paragraph},
};
use ratatui_image::{StatefulImage, picker::Picker, protocol::StatefulProtocol};
use std::time::Duration;

struct App {
    /// Playback position through the track, 0 to 1.
    progress: f64,
    current_time: String,
    is_playing: bool,
    cover: StatefulProtocol,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = std::env::args().nth(1) else {
        return Err("usage: phonoscule-tui <cover-image>".into());
    };

    // Asks the terminal what image protocol it speaks, falling back to unicode half blocks.
    let picker = Picker::from_query_stdio()?;
    let mut app = App {
        progress: 0.45,
        current_time: "01:23".to_string(),
        is_playing: true,
        cover: picker.new_resize_protocol(image::open(&path)?),
    };

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result?;
    Ok(())
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    loop {
        terminal.draw(|frame| draw(frame, app))?;
        if !event::poll(Duration::from_millis(16))? {
            continue;
        }
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Char(' ') => app.is_playing = !app.is_playing,
                _ => (),
            }
        }
    }
}

fn draw(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(10),   // cover art
            Constraint::Length(1), // seek bar
            Constraint::Length(1), // controls
        ])
        .split(frame.area());

    let cover_block = Block::default().borders(Borders::ALL).title(" Now Playing ");
    frame.render_stateful_widget(StatefulImage::default(), cover_block.inner(chunks[0]), &mut app.cover);
    frame.render_widget(cover_block, chunks[0]);

    let seek = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(7)])
        .split(chunks[1]);
    let bar = Gauge::default()
        .gauge_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .use_unicode(true)
        .ratio(app.progress);
    frame.render_widget(bar, seek[0]);
    frame.render_widget(Paragraph::new(format!(" {} ", app.current_time)).alignment(Alignment::Right), seek[1]);

    let play_icon = if app.is_playing { "⏸" } else { "▶" };
    let controls =
        Paragraph::new(format!("⏮   {play_icon}   ⏭")).alignment(Alignment::Center).style(Style::default().fg(Color::White));
    frame.render_widget(controls, chunks[2]);
}
