//! UI panel rendering for the Aranet GUI.
//!
//! This module contains the rendering logic for various panels in the application,
//! split into separate files for maintainability. Each panel is implemented as
//! methods on [`super::app::AranetApp`].

mod alerts;
mod app_settings;
mod comparison;
mod device_detail;
mod device_list;
mod history;
mod service;
mod settings;

// Re-export any panel-specific types if needed in the future
