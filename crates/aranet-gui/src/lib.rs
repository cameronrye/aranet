//! GUI for Aranet sensors.
//!
//! This crate provides a native desktop GUI for Aranet environmental sensors
//! using the egui framework.
//!
//! # Status
//!
//! This crate is currently a placeholder. The full GUI implementation is planned
//! for Phase 4 of the roadmap.

/// Application state for the GUI.
///
/// This struct holds the state that persists across frames.
#[derive(Debug, Default)]
pub struct AppState {
    /// Whether the app is currently scanning for devices.
    pub scanning: bool,
    /// Error message to display, if any.
    pub error_message: Option<String>,
    /// Status message to display.
    pub status_message: String,
}

impl AppState {
    /// Create a new AppState with default values.
    pub fn new() -> Self {
        Self {
            scanning: false,
            error_message: None,
            status_message: "Ready".to_string(),
        }
    }

    /// Set an error message.
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.error_message = Some(message.into());
    }

    /// Clear the error message.
    pub fn clear_error(&mut self) {
        self.error_message = None;
    }

    /// Set the status message.
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status_message = message.into();
    }

    /// Check if there is an active error.
    pub fn has_error(&self) -> bool {
        self.error_message.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_state_new() {
        let state = AppState::new();
        assert!(!state.scanning);
        assert!(state.error_message.is_none());
        assert_eq!(state.status_message, "Ready");
    }

    #[test]
    fn test_app_state_default() {
        let state = AppState::default();
        assert!(!state.scanning);
        assert!(state.error_message.is_none());
        assert!(state.status_message.is_empty());
    }

    #[test]
    fn test_app_state_set_error() {
        let mut state = AppState::new();
        assert!(!state.has_error());

        state.set_error("Test error");
        assert!(state.has_error());
        assert_eq!(state.error_message.as_deref(), Some("Test error"));
    }

    #[test]
    fn test_app_state_clear_error() {
        let mut state = AppState::new();
        state.set_error("Test error");
        assert!(state.has_error());

        state.clear_error();
        assert!(!state.has_error());
        assert!(state.error_message.is_none());
    }

    #[test]
    fn test_app_state_set_status() {
        let mut state = AppState::new();
        assert_eq!(state.status_message, "Ready");

        state.set_status("Scanning...");
        assert_eq!(state.status_message, "Scanning...");
    }

    #[test]
    fn test_app_state_debug() {
        let state = AppState::new();
        let debug_str = format!("{:?}", state);
        assert!(debug_str.contains("AppState"));
        assert!(debug_str.contains("scanning"));
    }
}
