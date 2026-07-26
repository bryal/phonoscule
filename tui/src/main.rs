use crossterm::{
    ExecutableCommand,
    event::{self, Event, KeyCode},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Gauge, Paragraph},
};
use ratatui_image::{Image, picker::Picker, protocol::Protocol};
use std::{io, time::Duration};

// Dummy app state
struct App {
    progress: f64,        // 0.0 to 1.0
    current_time: String, // "01:23"
    is_playing: bool,
    cover_state: ratatui_image::protocol::State,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Setup terminal
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let mut picker = Picker::from_query_stdio().unwrap();
    let dyn_img = image::open("/home/jojo/Transcoded/Veil of Maya/[m]other/cover.jpg")?;
    let cover_state = picker.new_protocol_state(dyn_img);

    // let target_size = ratatui::layout::Size::new(40, 20);
    // let cover_protocol = picker.new_protocol(dyn_img, target_size, ratatui_image::Resize::Fit(None)).unwrap();

    let mut app = App { progress: 0.45, current_time: "01:23".to_string(), is_playing: true, cover_art: cover_protocol };

    // 3. Main Loop
    loop {
        terminal.draw(|f| draw_ui(f, &mut app))?;

        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') {
                    break;
                }
                if key.code == KeyCode::Char(' ') {
                    app.is_playing = !app.is_playing;
                }
            }
        }
    }

    // 4. Teardown
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

fn draw_ui(f: &mut ratatui::Frame, app: &mut App) {
    let size = f.area();

    // Split screen into:
    // 1. Main Cover Art (taking up all available remaining space)
    // 2. Progress bar + time (height of 1)
    // 3. Media controls (height of 1)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(10),   // Cover Art Area
            Constraint::Length(1), // Progress Bar Area
            Constraint::Length(1), // Controls Area
        ])
        .split(size);

    // --- 1. COVER ART ---
    // The Image widget takes the protocol reference. ratatui-image handles
    // the heavy lifting of Kitty protocol escape sequences.
    let image_widget = Image::new(&app.cover_art);

    // We center the image by creating a wrapper block or letting the protocol handle resizing
    let cover_block = Block::default().borders(Borders::ALL).title(" Now Playing ");

    // Render the image widget inside the top chunk
    f.render_widget(image_widget, cover_block.inner(chunks[0]));
    f.render_widget(cover_block, chunks[0]);

    // --- 2. PROGRESS BAR & TIMESTAMP ---
    // Split the middle chunk horizontally to put time on the right
    let progress_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(10),   // The actual bar
            Constraint::Length(7), // " 01:23 " timestamp
        ])
        .split(chunks[1]);

    let progress_bar = Gauge::default()
        .gauge_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .use_unicode(true) // Gives us nice smooth blocks instead of ASCII
        .ratio(app.progress);

    let timestamp = Paragraph::new(format!(" {} ", app.current_time)).alignment(Alignment::Right);

    f.render_widget(progress_bar, progress_chunks[0]);
    f.render_widget(timestamp, progress_chunks[1]);

    // --- 3. CONTROLS ---
    let play_icon = if app.is_playing { "⏸" } else { "▶" };
    let controls_text = format!("⏮   {}   ⏭", play_icon);

    let controls = Paragraph::new(controls_text).alignment(Alignment::Center).style(Style::default().fg(Color::White));

    f.render_widget(controls, chunks[2]);
}
