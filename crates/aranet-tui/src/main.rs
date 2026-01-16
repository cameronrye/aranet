use std::io::{self, stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    ExecutableCommand,
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};

/// Application state
struct App {
    should_quit: bool,
}

impl App {
    fn new() -> Self {
        Self { should_quit: false }
    }

    fn handle_key(&mut self, key: KeyCode) {
        if key == KeyCode::Char('q') {
            self.should_quit = true;
        }
    }
}

/// Set up the terminal for TUI rendering
fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore the terminal to its original state
fn restore_terminal() -> Result<()> {
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

/// Draw the UI
fn draw(frame: &mut Frame) {
    let area = frame.area();

    let block = Block::default()
        .title(" Aranet TUI ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let message = Paragraph::new("Aranet TUI - Coming Soon\n\nPress 'q' to quit")
        .alignment(Alignment::Center)
        .block(block);

    // Center the message vertically
    let vertical_center = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Length(5),
            Constraint::Percentage(40),
        ])
        .split(area);

    frame.render_widget(message, vertical_center[1]);
}

/// Main event loop
fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let mut app = App::new();

    while !app.should_quit {
        terminal.draw(draw)?;

        // Poll for events with a timeout
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.handle_key(key.code);
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut terminal = setup_terminal()?;

    // Run the app and ensure terminal is restored even on error
    let result = run(&mut terminal);

    restore_terminal()?;

    result
}
