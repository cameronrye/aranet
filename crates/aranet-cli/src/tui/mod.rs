//! Main entry point for the TUI dashboard.
//!
//! This module ties together all the TUI components and provides the main
//! event loop for the terminal user interface. It handles:
//!
//! - Terminal setup and restoration
//! - Channel creation for worker communication
//! - The main event loop with input handling and rendering
//! - Graceful shutdown coordination

pub mod app;
pub mod errors;
pub mod input;
pub mod messages;
pub mod ui;
pub mod worker;

pub use app::App;
pub use messages::{Command, SensorEvent};
pub use worker::SensorWorker;

use std::io::{self, stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    ExecutableCommand,
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::*;
use tokio::sync::mpsc;
use tracing::info;

use aranet_store::default_db_path;

/// Set up the terminal for TUI rendering.
///
/// Enables raw mode, mouse capture, and switches to the alternate screen buffer.
pub fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    stdout().execute(EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout());
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore the terminal to its original state.
///
/// Disables mouse capture, raw mode and returns to the main screen buffer.
pub fn restore_terminal() -> Result<()> {
    stdout().execute(DisableMouseCapture)?;
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

/// Run the TUI application.
///
/// This is the main entry point for the TUI. It:
/// 1. Creates communication channels between UI and worker
/// 2. Gets the store path (if available)
/// 3. Spawns the background sensor worker
/// 4. Runs the main event loop
/// 5. Ensures graceful shutdown
pub async fn run() -> Result<()> {
    // Create communication channels
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(32);
    let (event_tx, event_rx) = mpsc::channel::<SensorEvent>(32);

    // Get the store path for persistence
    let store_path = default_db_path();
    info!("Store path: {:?}", store_path);

    // Create and spawn the background worker
    let worker = SensorWorker::new(cmd_rx, event_tx, store_path);
    let worker_handle = tokio::spawn(worker.run());

    // Create the application
    let mut app = App::new(cmd_tx.clone(), event_rx);

    // Set up terminal
    let mut terminal = setup_terminal()?;

    // Load cached devices from store first (shows data immediately)
    let _ = cmd_tx.try_send(Command::LoadCachedData);

    // Then auto-scan for live devices
    let _ = cmd_tx.try_send(Command::Scan {
        duration: Duration::from_secs(5),
    });

    // Run the main event loop
    let result = run_event_loop(&mut terminal, &mut app, &cmd_tx).await;

    // Send shutdown command to worker
    let _ = cmd_tx.try_send(Command::Shutdown);

    // Restore terminal
    restore_terminal()?;

    // Wait for worker to complete
    let _ = worker_handle.await;

    result
}

/// Main event loop for the TUI.
async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    command_tx: &mpsc::Sender<Command>,
) -> Result<()> {
    while !app.should_quit() {
        // Tick spinner animation
        app.tick_spinner();
        app.clean_expired_messages();

        // Draw the UI
        terminal.draw(|f| ui::draw(f, app))?;

        // Poll for keyboard and mouse events with timeout
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        let action = input::handle_key(key.code, app.editing_alias, app.pending_confirmation.is_some());
                        if let Some(cmd) = input::apply_action(app, action, command_tx) {
                            let _ = command_tx.try_send(cmd);
                        }
                    }
                }
                Event::Mouse(mouse_event) => {
                    let action = input::handle_mouse(mouse_event);
                    if let Some(cmd) = input::apply_action(app, action, command_tx) {
                        let _ = command_tx.try_send(cmd);
                    }
                }
                _ => {}
            }
        }

        // Non-blocking receive of sensor events
        while let Ok(event) = app.event_rx.try_recv() {
            app.handle_sensor_event(event);
        }

        // Check for auto-refresh of connected devices
        let devices_to_refresh = app.check_auto_refresh();
        for device_id in devices_to_refresh {
            let _ = command_tx.try_send(Command::RefreshReading { device_id });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    #[test]
    fn test_terminal_functions_exist() {
        // Just verify the functions compile correctly
        // Actual terminal tests require a real terminal
        let _ = restore_terminal;
        let _ = setup_terminal;
    }

    #[test]
    fn test_input_handling_quit() {
        let action = input::handle_key(KeyCode::Char('q'), false, false);
        assert_eq!(action, input::Action::Quit);
    }

    #[test]
    fn test_input_handling_scan() {
        let action = input::handle_key(KeyCode::Char('s'), false, false);
        assert_eq!(action, input::Action::Scan);
    }

    #[test]
    fn test_input_handling_connect_all() {
        // Lowercase 'c' connects selected device
        let action = input::handle_key(KeyCode::Char('c'), false, false);
        assert_eq!(action, input::Action::Connect);

        // Uppercase 'C' connects all devices
        let action = input::handle_key(KeyCode::Char('C'), false, false);
        assert_eq!(action, input::Action::ConnectAll);
    }

    #[test]
    fn test_input_handling_other_keys() {
        let action = input::handle_key(KeyCode::Char('a'), false, false);
        // 'a' is now mapped to ToggleAlertHistory
        assert_eq!(action, input::Action::ToggleAlertHistory);

        // Enter is now mapped to ChangeSetting
        let action = input::handle_key(KeyCode::Enter, false, false);
        assert_eq!(action, input::Action::ChangeSetting);
    }

    #[test]
    fn test_input_handling_confirmation() {
        // When confirmation is pending, only Y/N keys work
        let action = input::handle_key(KeyCode::Char('y'), false, true);
        assert_eq!(action, input::Action::Confirm);

        let action = input::handle_key(KeyCode::Char('n'), false, true);
        assert_eq!(action, input::Action::Cancel);

        let action = input::handle_key(KeyCode::Esc, false, true);
        assert_eq!(action, input::Action::Cancel);

        // Other keys are ignored during confirmation
        let action = input::handle_key(KeyCode::Char('q'), false, true);
        assert_eq!(action, input::Action::None);
    }
}

