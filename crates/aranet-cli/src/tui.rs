//! TUI module for Aranet terminal dashboard.
//!
//! This module provides an interactive terminal user interface for monitoring
//! Aranet environmental sensors. It can be used standalone (when only the `tui`
//! feature is enabled) or as a subcommand of the CLI (when both `cli` and `tui`
//! features are enabled).

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

/// Application state for the TUI
pub struct App {
    should_quit: bool,
}

impl App {
    /// Create a new App instance
    pub fn new() -> Self {
        Self { should_quit: false }
    }

    /// Handle a key press event
    pub fn handle_key(&mut self, key: KeyCode) {
        if key == KeyCode::Char('q') {
            self.should_quit = true;
        }
    }

    /// Check if the app should quit
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Set up the terminal for TUI rendering
pub fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore the terminal to its original state
pub fn restore_terminal() -> Result<()> {
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

/// Draw the UI
pub fn draw(frame: &mut Frame) {
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

/// Main event loop for the TUI
pub fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let mut app = App::new();

    while !app.should_quit() {
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

/// Run the TUI application
///
/// This is the main entry point for the TUI. It sets up the terminal,
/// runs the event loop, and ensures the terminal is restored on exit.
pub async fn run() -> Result<()> {
    let mut terminal = setup_terminal()?;

    // Run the app and ensure terminal is restored even on error
    let result = run_loop(&mut terminal);

    restore_terminal()?;

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_new() {
        let app = App::new();
        assert!(!app.should_quit());
    }

    #[test]
    fn test_app_default() {
        let app = App::default();
        assert!(!app.should_quit());
    }

    #[test]
    fn test_app_handle_key_q_quits() {
        let mut app = App::new();
        assert!(!app.should_quit());

        app.handle_key(KeyCode::Char('q'));
        assert!(app.should_quit());
    }

    #[test]
    fn test_app_handle_key_other_does_not_quit() {
        let mut app = App::new();

        app.handle_key(KeyCode::Char('a'));
        assert!(!app.should_quit());

        app.handle_key(KeyCode::Enter);
        assert!(!app.should_quit());

        app.handle_key(KeyCode::Esc);
        assert!(!app.should_quit());

        app.handle_key(KeyCode::Up);
        assert!(!app.should_quit());
    }

    #[test]
    fn test_app_handle_key_uppercase_q_does_not_quit() {
        let mut app = App::new();

        // Only lowercase 'q' should quit
        app.handle_key(KeyCode::Char('Q'));
        assert!(!app.should_quit());
    }

    #[test]
    fn test_app_should_quit_returns_correct_state() {
        let mut app = App::new();

        // Initial state
        assert!(!app.should_quit);
        assert!(!app.should_quit());

        // After setting manually
        app.should_quit = true;
        assert!(app.should_quit);
        assert!(app.should_quit());
    }
}
