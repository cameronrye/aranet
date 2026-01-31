//! Native menu bar integration for the Aranet GUI.
//!
//! This module provides cross-platform native menu bar support using the `muda` crate
//! (via the tray-icon re-export to avoid Objective-C class conflicts).
//! It creates native menus on:
//! - macOS: NSMenu (system menu bar at top of screen)
//! - Windows: Win32 menus
//! - Linux: GTK3 menus
//!
//! The menu mirrors the application's keyboard shortcuts and provides access to
//! all major features.

// Use tray_icon's re-export of muda to avoid Objective-C class registration conflicts
use tracing::{debug, info};
use tray_icon::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
    accelerator::{Accelerator, Code, Modifiers},
};

/// Commands that can be triggered from the native menu bar.
#[derive(Debug, Clone)]
pub enum MenuCommand {
    // === File Menu ===
    /// Scan for nearby devices (F5)
    Scan,
    /// Refresh all connected devices (Cmd+R)
    RefreshAll,
    /// Export history as CSV (Cmd+E)
    ExportCsv,
    /// Export history as JSON (Cmd+Shift+E)
    ExportJson,

    // === View Menu ===
    /// Toggle dark/light theme
    ToggleTheme,
    /// Set theme to system default
    ThemeSystem,
    /// Set theme to light mode
    ThemeLight,
    /// Set theme to dark mode
    ThemeDark,
    /// Toggle auto-refresh (A)
    ToggleAutoRefresh,
    /// Set refresh interval
    SetRefreshInterval(u16),
    /// Toggle notifications
    ToggleNotifications,
    /// Toggle CO2 display
    ToggleCo2Display,
    /// Toggle temperature display
    ToggleTemperatureDisplay,
    /// Toggle humidity display
    ToggleHumidityDisplay,
    /// Toggle pressure display
    TogglePressureDisplay,
    /// Switch to Dashboard tab (Cmd+1)
    ShowDashboard,
    /// Switch to History tab (Cmd+2)
    ShowHistory,
    /// Switch to Settings tab (Cmd+3)
    ShowSettings,
    /// Switch to Service tab (Cmd+4)
    ShowService,

    // === Device Menu ===
    /// Connect to a specific device by index
    ConnectDevice(usize),
    /// Disconnect from a device by index
    DisconnectDevice(usize),
    /// Open device aliases configuration
    ManageAliases,
    /// Forget/remove a device
    ForgetDevice(usize),

    // === Help Menu ===
    /// Open online documentation
    OpenDocumentation,
    /// Open GitHub issues page
    ReportIssue,
    /// Check for updates
    CheckForUpdates,
    /// Show about dialog (Windows/Linux)
    ShowAbout,

    /// Quit the application
    Quit,
}

/// Holds menu item IDs for event matching.
pub struct MenuManager {
    // === File menu items ===
    scan_item: MenuItem,
    refresh_item: MenuItem,
    export_csv_item: MenuItem,
    export_json_item: MenuItem,

    // === View menu items ===
    /// Appearance submenu items
    theme_system_item: CheckMenuItem,
    theme_light_item: CheckMenuItem,
    theme_dark_item: CheckMenuItem,
    /// Legacy toggle (for backwards compat with keyboard shortcut)
    #[allow(dead_code)]
    theme_toggle_item: CheckMenuItem,
    auto_refresh_item: CheckMenuItem,
    /// Refresh interval submenu items
    interval_30s_item: CheckMenuItem,
    interval_1m_item: CheckMenuItem,
    interval_5m_item: CheckMenuItem,
    interval_10m_item: CheckMenuItem,
    /// Notifications toggle
    notifications_item: CheckMenuItem,
    /// Display toggles
    show_co2_item: CheckMenuItem,
    show_temp_item: CheckMenuItem,
    show_humidity_item: CheckMenuItem,
    show_pressure_item: CheckMenuItem,
    /// Tab navigation
    dashboard_item: MenuItem,
    history_item: MenuItem,
    settings_item: MenuItem,
    service_item: MenuItem,

    // === Device menu ===
    device_menu: Submenu,
    manage_aliases_item: MenuItem,

    // === Help menu items ===
    documentation_item: MenuItem,
    report_issue_item: MenuItem,
    check_updates_item: MenuItem,
    #[cfg(not(target_os = "macos"))]
    about_item: MenuItem,

    /// The main menu bar
    #[allow(dead_code)]
    menu: Menu,
}

impl MenuManager {
    /// Create the native menu bar.
    ///
    /// On macOS, this should be called from the main thread before the event loop.
    pub fn new() -> Result<Self, tray_icon::menu::Error> {
        let menu = Menu::new();

        // Platform-specific modifier key
        #[cfg(target_os = "macos")]
        let cmd = Modifiers::META;
        #[cfg(not(target_os = "macos"))]
        let cmd = Modifiers::CONTROL;

        // === App menu (macOS only) ===
        #[cfg(target_os = "macos")]
        {
            let app_menu = Submenu::new("Aranet", true);
            app_menu.append(&PredefinedMenuItem::about(Some("Aranet"), None))?;
            app_menu.append(&PredefinedMenuItem::separator())?;
            app_menu.append(&PredefinedMenuItem::services(Some("Aranet")))?;
            app_menu.append(&PredefinedMenuItem::separator())?;
            app_menu.append(&PredefinedMenuItem::hide(Some("Aranet")))?;
            app_menu.append(&PredefinedMenuItem::hide_others(Some("Aranet")))?;
            app_menu.append(&PredefinedMenuItem::show_all(Some("Aranet")))?;
            app_menu.append(&PredefinedMenuItem::separator())?;
            app_menu.append(&PredefinedMenuItem::quit(Some("Aranet")))?;
            menu.append(&app_menu)?;
        }

        // === File menu ===
        let file_menu = Submenu::new("File", true);

        let scan_item = MenuItem::new(
            "Scan for Devices",
            true,
            Some(Accelerator::new(None, Code::F5)),
        );
        let refresh_item = MenuItem::new(
            "Refresh All",
            true,
            Some(Accelerator::new(Some(cmd), Code::KeyR)),
        );

        file_menu.append(&scan_item)?;
        file_menu.append(&refresh_item)?;
        file_menu.append(&PredefinedMenuItem::separator())?;

        // Export submenu
        let export_menu = Submenu::new("Export History", true);
        let export_csv_item = MenuItem::new(
            "Export as CSV...",
            true,
            Some(Accelerator::new(Some(cmd), Code::KeyE)),
        );
        let export_json_item = MenuItem::new(
            "Export as JSON...",
            true,
            Some(Accelerator::new(Some(cmd | Modifiers::SHIFT), Code::KeyE)),
        );
        export_menu.append(&export_csv_item)?;
        export_menu.append(&export_json_item)?;
        file_menu.append(&export_menu)?;

        file_menu.append(&PredefinedMenuItem::separator())?;
        file_menu.append(&PredefinedMenuItem::close_window(None))?;

        #[cfg(not(target_os = "macos"))]
        {
            file_menu.append(&PredefinedMenuItem::separator())?;
            file_menu.append(&PredefinedMenuItem::quit(None))?;
        }

        menu.append(&file_menu)?;

        // === Edit menu (standard system items) ===
        let edit_menu = Submenu::new("Edit", true);
        edit_menu.append(&PredefinedMenuItem::undo(None))?;
        edit_menu.append(&PredefinedMenuItem::redo(None))?;
        edit_menu.append(&PredefinedMenuItem::separator())?;
        edit_menu.append(&PredefinedMenuItem::cut(None))?;
        edit_menu.append(&PredefinedMenuItem::copy(None))?;
        edit_menu.append(&PredefinedMenuItem::paste(None))?;
        edit_menu.append(&PredefinedMenuItem::select_all(None))?;
        menu.append(&edit_menu)?;

        // === View menu ===
        let view_menu = Submenu::new("View", true);

        // Navigation section
        let dashboard_item = MenuItem::new(
            "Dashboard",
            true,
            Some(Accelerator::new(Some(cmd), Code::Digit1)),
        );
        let history_item = MenuItem::new(
            "History",
            true,
            Some(Accelerator::new(Some(cmd), Code::Digit2)),
        );
        let settings_item = MenuItem::new(
            "Settings",
            true,
            Some(Accelerator::new(Some(cmd), Code::Digit3)),
        );
        let service_item = MenuItem::new(
            "Service",
            true,
            Some(Accelerator::new(Some(cmd), Code::Digit4)),
        );

        view_menu.append(&dashboard_item)?;
        view_menu.append(&history_item)?;
        view_menu.append(&settings_item)?;
        view_menu.append(&service_item)?;
        view_menu.append(&PredefinedMenuItem::separator())?;

        // Appearance submenu
        let appearance_menu = Submenu::new("Appearance", true);
        let theme_system_item = CheckMenuItem::new("System", true, false, None);
        let theme_light_item = CheckMenuItem::new("Light", true, false, None);
        let theme_dark_item = CheckMenuItem::new("Dark", true, true, None); // Default to dark
        appearance_menu.append(&theme_system_item)?;
        appearance_menu.append(&theme_light_item)?;
        appearance_menu.append(&theme_dark_item)?;
        view_menu.append(&appearance_menu)?;

        // Hidden theme toggle for keyboard shortcut compatibility
        let theme_toggle_item = CheckMenuItem::new(
            "Dark Mode",
            false, // Hidden
            false,
            Some(Accelerator::new(None, Code::KeyT)),
        );

        view_menu.append(&PredefinedMenuItem::separator())?;

        // Display options submenu
        let display_menu = Submenu::new("Display", true);
        let show_co2_item = CheckMenuItem::new("CO2 Level", true, true, None);
        let show_temp_item = CheckMenuItem::new("Temperature", true, true, None);
        let show_humidity_item = CheckMenuItem::new("Humidity", true, true, None);
        let show_pressure_item = CheckMenuItem::new("Pressure", true, true, None);
        display_menu.append(&show_co2_item)?;
        display_menu.append(&show_temp_item)?;
        display_menu.append(&show_humidity_item)?;
        display_menu.append(&show_pressure_item)?;
        view_menu.append(&display_menu)?;

        view_menu.append(&PredefinedMenuItem::separator())?;

        // Auto-refresh and interval
        let auto_refresh_item = CheckMenuItem::new(
            "Auto Refresh",
            true,
            true,
            Some(Accelerator::new(None, Code::KeyA)),
        );
        view_menu.append(&auto_refresh_item)?;

        let interval_menu = Submenu::new("Refresh Interval", true);
        let interval_30s_item = CheckMenuItem::new("30 seconds", true, false, None);
        let interval_1m_item = CheckMenuItem::new("1 minute", true, true, None); // Default
        let interval_5m_item = CheckMenuItem::new("5 minutes", true, false, None);
        let interval_10m_item = CheckMenuItem::new("10 minutes", true, false, None);
        interval_menu.append(&interval_30s_item)?;
        interval_menu.append(&interval_1m_item)?;
        interval_menu.append(&interval_5m_item)?;
        interval_menu.append(&interval_10m_item)?;
        view_menu.append(&interval_menu)?;

        view_menu.append(&PredefinedMenuItem::separator())?;

        // Notifications
        let notifications_item = CheckMenuItem::new("Enable Notifications", true, true, None);
        view_menu.append(&notifications_item)?;

        view_menu.append(&PredefinedMenuItem::separator())?;
        view_menu.append(&PredefinedMenuItem::fullscreen(None))?;

        menu.append(&view_menu)?;

        // === Device menu ===
        let device_menu = Submenu::new("Device", true);

        // Scan is duplicated here for discoverability
        let device_scan_item = MenuItem::new(
            "Scan for Devices",
            true,
            None, // No accelerator, F5 is on File menu
        );
        device_menu.append(&device_scan_item)?;
        device_menu.append(&PredefinedMenuItem::separator())?;

        // Placeholder for dynamic device list - will be populated at runtime
        let no_devices_item = MenuItem::new("No devices", false, None);
        device_menu.append(&no_devices_item)?;

        device_menu.append(&PredefinedMenuItem::separator())?;

        let manage_aliases_item = MenuItem::new("Manage Aliases...", true, None);
        device_menu.append(&manage_aliases_item)?;

        menu.append(&device_menu)?;

        // === Window menu ===
        let window_menu = Submenu::new("Window", true);
        window_menu.append(&PredefinedMenuItem::minimize(None))?;
        window_menu.append(&PredefinedMenuItem::maximize(None))?;
        window_menu.append(&PredefinedMenuItem::separator())?;
        window_menu.append(&PredefinedMenuItem::fullscreen(None))?;

        #[cfg(target_os = "macos")]
        {
            window_menu.append(&PredefinedMenuItem::separator())?;
            window_menu.append(&PredefinedMenuItem::bring_all_to_front(None))?;
        }

        menu.append(&window_menu)?;

        // === Help menu ===
        let help_menu = Submenu::new("Help", true);

        let documentation_item = MenuItem::new("Aranet Documentation", true, None);
        let report_issue_item = MenuItem::new("Report an Issue...", true, None);
        let check_updates_item = MenuItem::new("Check for Updates...", true, None);

        help_menu.append(&documentation_item)?;
        help_menu.append(&report_issue_item)?;
        help_menu.append(&PredefinedMenuItem::separator())?;
        help_menu.append(&check_updates_item)?;

        // About item for Windows/Linux (macOS has it in App menu)
        #[cfg(not(target_os = "macos"))]
        let about_item = {
            help_menu.append(&PredefinedMenuItem::separator())?;
            let item = MenuItem::new("About Aranet", true, None);
            help_menu.append(&item)?;
            item
        };

        menu.append(&help_menu)?;

        info!("Native menu bar created with enhanced menus");

        Ok(Self {
            // File menu
            scan_item,
            refresh_item,
            export_csv_item,
            export_json_item,

            // View menu - appearance
            theme_system_item,
            theme_light_item,
            theme_dark_item,
            theme_toggle_item,

            // View menu - auto refresh
            auto_refresh_item,
            interval_30s_item,
            interval_1m_item,
            interval_5m_item,
            interval_10m_item,

            // View menu - notifications
            notifications_item,

            // View menu - display
            show_co2_item,
            show_temp_item,
            show_humidity_item,
            show_pressure_item,

            // View menu - tabs
            dashboard_item,
            history_item,
            settings_item,
            service_item,

            // Device menu
            device_menu,
            manage_aliases_item,

            // Help menu
            documentation_item,
            report_issue_item,
            check_updates_item,
            #[cfg(not(target_os = "macos"))]
            about_item,

            menu,
        })
    }

    /// Initialize the menu for macOS (call from main thread before event loop).
    #[cfg(target_os = "macos")]
    pub fn init_for_macos(&self) {
        self.menu.init_for_nsapp();
        debug!("Menu initialized for macOS NSApp");
    }

    /// Initialize the menu (no-op on non-macOS platforms - they need window handle).
    #[cfg(not(target_os = "macos"))]
    pub fn init_for_macos(&self) {
        // On Windows/Linux, we need to init after window creation
        // This is handled separately
    }

    /// Process pending menu events and return any commands.
    pub fn process_events(&self) -> Vec<MenuCommand> {
        let mut commands = Vec::new();

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            // === File menu ===
            if event.id == self.scan_item.id() {
                debug!("Menu: Scan clicked");
                commands.push(MenuCommand::Scan);
            } else if event.id == self.refresh_item.id() {
                debug!("Menu: Refresh All clicked");
                commands.push(MenuCommand::RefreshAll);
            } else if event.id == self.export_csv_item.id() {
                debug!("Menu: Export CSV clicked");
                commands.push(MenuCommand::ExportCsv);
            } else if event.id == self.export_json_item.id() {
                debug!("Menu: Export JSON clicked");
                commands.push(MenuCommand::ExportJson);
            }
            // === View menu - appearance ===
            else if event.id == *self.theme_system_item.id() {
                debug!("Menu: Theme System clicked");
                commands.push(MenuCommand::ThemeSystem);
            } else if event.id == *self.theme_light_item.id() {
                debug!("Menu: Theme Light clicked");
                commands.push(MenuCommand::ThemeLight);
            } else if event.id == *self.theme_dark_item.id() {
                debug!("Menu: Theme Dark clicked");
                commands.push(MenuCommand::ThemeDark);
            } else if event.id == *self.theme_toggle_item.id() {
                debug!("Menu: Toggle Theme clicked");
                commands.push(MenuCommand::ToggleTheme);
            }
            // === View menu - auto refresh ===
            else if event.id == *self.auto_refresh_item.id() {
                debug!("Menu: Toggle Auto Refresh clicked");
                commands.push(MenuCommand::ToggleAutoRefresh);
            } else if event.id == *self.interval_30s_item.id() {
                debug!("Menu: Interval 30s clicked");
                commands.push(MenuCommand::SetRefreshInterval(30));
            } else if event.id == *self.interval_1m_item.id() {
                debug!("Menu: Interval 1m clicked");
                commands.push(MenuCommand::SetRefreshInterval(60));
            } else if event.id == *self.interval_5m_item.id() {
                debug!("Menu: Interval 5m clicked");
                commands.push(MenuCommand::SetRefreshInterval(300));
            } else if event.id == *self.interval_10m_item.id() {
                debug!("Menu: Interval 10m clicked");
                commands.push(MenuCommand::SetRefreshInterval(600));
            }
            // === View menu - notifications ===
            else if event.id == *self.notifications_item.id() {
                debug!("Menu: Toggle Notifications clicked");
                commands.push(MenuCommand::ToggleNotifications);
            }
            // === View menu - display toggles ===
            else if event.id == *self.show_co2_item.id() {
                debug!("Menu: Toggle CO2 Display clicked");
                commands.push(MenuCommand::ToggleCo2Display);
            } else if event.id == *self.show_temp_item.id() {
                debug!("Menu: Toggle Temperature Display clicked");
                commands.push(MenuCommand::ToggleTemperatureDisplay);
            } else if event.id == *self.show_humidity_item.id() {
                debug!("Menu: Toggle Humidity Display clicked");
                commands.push(MenuCommand::ToggleHumidityDisplay);
            } else if event.id == *self.show_pressure_item.id() {
                debug!("Menu: Toggle Pressure Display clicked");
                commands.push(MenuCommand::TogglePressureDisplay);
            }
            // === View menu - tabs ===
            else if event.id == self.dashboard_item.id() {
                debug!("Menu: Dashboard clicked");
                commands.push(MenuCommand::ShowDashboard);
            } else if event.id == self.history_item.id() {
                debug!("Menu: History clicked");
                commands.push(MenuCommand::ShowHistory);
            } else if event.id == self.settings_item.id() {
                debug!("Menu: Settings clicked");
                commands.push(MenuCommand::ShowSettings);
            } else if event.id == self.service_item.id() {
                debug!("Menu: Service clicked");
                commands.push(MenuCommand::ShowService);
            }
            // === Device menu ===
            else if event.id == self.manage_aliases_item.id() {
                debug!("Menu: Manage Aliases clicked");
                commands.push(MenuCommand::ManageAliases);
            }
            // === Help menu ===
            else if event.id == self.documentation_item.id() {
                debug!("Menu: Documentation clicked");
                commands.push(MenuCommand::OpenDocumentation);
            } else if event.id == self.report_issue_item.id() {
                debug!("Menu: Report Issue clicked");
                commands.push(MenuCommand::ReportIssue);
            } else if event.id == self.check_updates_item.id() {
                debug!("Menu: Check Updates clicked");
                commands.push(MenuCommand::CheckForUpdates);
            }
            #[cfg(not(target_os = "macos"))]
            if event.id == self.about_item.id() {
                debug!("Menu: About clicked");
                commands.push(MenuCommand::ShowAbout);
            }
        }

        commands
    }

    // =========================================================================
    // State synchronization methods
    // =========================================================================

    /// Update the theme selection in the Appearance submenu.
    pub fn set_theme(&self, theme: &str) {
        match theme {
            "system" => {
                self.theme_system_item.set_checked(true);
                self.theme_light_item.set_checked(false);
                self.theme_dark_item.set_checked(false);
            }
            "light" => {
                self.theme_system_item.set_checked(false);
                self.theme_light_item.set_checked(true);
                self.theme_dark_item.set_checked(false);
            }
            _ => {
                self.theme_system_item.set_checked(false);
                self.theme_light_item.set_checked(false);
                self.theme_dark_item.set_checked(true);
            }
        }
    }

    /// Update the theme checkbox state (legacy, for backwards compatibility).
    pub fn set_dark_mode(&self, dark_mode: bool) {
        if dark_mode {
            self.set_theme("dark");
        } else {
            self.set_theme("light");
        }
    }

    /// Update the auto-refresh checkbox state.
    pub fn set_auto_refresh(&self, enabled: bool) {
        self.auto_refresh_item.set_checked(enabled);
    }

    /// Update the refresh interval selection.
    pub fn set_refresh_interval(&self, seconds: u16) {
        self.interval_30s_item.set_checked(seconds == 30);
        self.interval_1m_item.set_checked(seconds == 60);
        self.interval_5m_item.set_checked(seconds == 300);
        self.interval_10m_item.set_checked(seconds == 600);
    }

    /// Update the notifications enabled state.
    pub fn set_notifications_enabled(&self, enabled: bool) {
        self.notifications_item.set_checked(enabled);
    }

    /// Update display toggle states.
    pub fn set_display_options(&self, co2: bool, temp: bool, humidity: bool, pressure: bool) {
        self.show_co2_item.set_checked(co2);
        self.show_temp_item.set_checked(temp);
        self.show_humidity_item.set_checked(humidity);
        self.show_pressure_item.set_checked(pressure);
    }

    /// Enable or disable the scan menu item.
    pub fn set_scan_enabled(&self, enabled: bool) {
        self.scan_item.set_enabled(enabled);
    }

    /// Enable or disable the export menu items (based on whether history is available).
    pub fn set_export_enabled(&self, enabled: bool) {
        self.export_csv_item.set_enabled(enabled);
        self.export_json_item.set_enabled(enabled);
    }

    /// Get the device menu submenu for dynamic updates.
    pub fn device_menu(&self) -> &Submenu {
        &self.device_menu
    }
}
