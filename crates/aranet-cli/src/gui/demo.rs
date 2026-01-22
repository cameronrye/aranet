//! Demo mode for generating screenshots with mock data.
//!
//! This module provides deterministic mock data for reproducible screenshots.

use aranet_core::settings::DeviceSettings;
use aranet_core::BluetoothRange;
use aranet_types::{CurrentReading, DeviceType, HistoryRecord, Status};
use time::{Duration, OffsetDateTime};

use super::types::{ConnectionState, DeviceState};

/// Generate demo devices with realistic mock data for screenshots.
pub fn create_demo_devices() -> Vec<DeviceState> {
    vec![
        create_aranet4_device(),
        create_radon_device(),
        create_aranet2_device(),
    ]
}

/// Create an Aranet4 device with typical office readings.
fn create_aranet4_device() -> DeviceState {
    let reading = CurrentReading {
        co2: 847,
        temperature: 22.5,
        pressure: 1013.2,
        humidity: 45,
        battery: 87,
        status: Status::Green,
        interval: 300,
        age: 120,
        captured_at: Some(OffsetDateTime::now_utc()),
        radon: None,
        radon_avg_24h: None,
        radon_avg_7d: None,
        radon_avg_30d: None,
        radiation_rate: None,
        radiation_total: None,
    };

    let settings = DeviceSettings {
        bluetooth_range: BluetoothRange::Standard,
        smart_home_enabled: true,
        temperature_unit: aranet_core::settings::TemperatureUnit::Celsius,
        radon_unit: aranet_core::settings::RadonUnit::BqM3,
        buzzer_enabled: true,
        auto_calibration_enabled: true,
    };

    let history = generate_co2_history(24 * 12); // 24 hours at 5-min intervals

    DeviceState {
        id: "DEMO-ARANET4-001".to_string(),
        name: Some("Living Room".to_string()),
        device_type: Some(DeviceType::Aranet4),
        rssi: Some(-45),
        connection: ConnectionState::Connected,
        reading: Some(reading),
        previous_reading: None,
        history,
        syncing_history: false,
        settings: Some(settings),
        reading_from_cache: false,
    }
}

/// Create an AranetRn+ device with typical basement readings.
fn create_radon_device() -> DeviceState {
    let reading = CurrentReading {
        co2: 0,
        temperature: 18.5,
        pressure: 1012.8,
        humidity: 55,
        battery: 92,
        status: Status::Green,
        interval: 3600,
        age: 1800,
        captured_at: Some(OffsetDateTime::now_utc()),
        radon: Some(85),
        radon_avg_24h: Some(80),
        radon_avg_7d: Some(75),
        radon_avg_30d: Some(70),
        radiation_rate: None,
        radiation_total: None,
    };

    DeviceState {
        id: "DEMO-RADON-001".to_string(),
        name: Some("Basement".to_string()),
        device_type: Some(DeviceType::AranetRadon),
        rssi: Some(-52),
        connection: ConnectionState::Connected,
        reading: Some(reading),
        previous_reading: None,
        history: generate_radon_history(24 * 7), // 7 days at 1-hour intervals
        syncing_history: false,
        settings: None,
        reading_from_cache: false,
    }
}

/// Create an Aranet2 device with typical readings.
fn create_aranet2_device() -> DeviceState {
    let reading = CurrentReading {
        co2: 0,
        temperature: 24.2,
        pressure: 0.0,
        humidity: 42,
        battery: 78,
        status: Status::Green,
        interval: 300,
        age: 60,
        captured_at: Some(OffsetDateTime::now_utc()),
        radon: None,
        radon_avg_24h: None,
        radon_avg_7d: None,
        radon_avg_30d: None,
        radiation_rate: None,
        radiation_total: None,
    };

    DeviceState {
        id: "DEMO-ARANET2-001".to_string(),
        name: Some("Bedroom".to_string()),
        device_type: Some(DeviceType::Aranet2),
        rssi: Some(-60),
        connection: ConnectionState::Disconnected,
        reading: Some(reading),
        previous_reading: None,
        history: Vec::new(),
        syncing_history: false,
        settings: None,
        reading_from_cache: true,
    }
}

/// Generate realistic CO2 history with daily patterns.
fn generate_co2_history(count: usize) -> Vec<HistoryRecord> {
    let mut history = Vec::with_capacity(count);
    let now = OffsetDateTime::now_utc();
    let interval_mins = 5i64;

    for i in 0..count {
        let offset = Duration::minutes((count - 1 - i) as i64 * interval_mins);
        let timestamp = now - offset;

        // Simulate daily CO2 pattern: higher during day, lower at night
        let hour = timestamp.hour() as f32;
        let base_co2 = if (8.0..22.0).contains(&hour) {
            700.0 + (hour - 8.0) * 30.0 // Rising during day
        } else {
            450.0 // Low at night
        };

        // Add some noise
        let noise = ((i * 17) % 100) as f32 - 50.0;
        let co2 = (base_co2 + noise).clamp(400.0, 1200.0) as u16;

        history.push(HistoryRecord {
            timestamp,
            co2,
            temperature: 21.0 + (i % 30) as f32 * 0.1,
            pressure: 1013.0 + (i % 10) as f32 * 0.2,
            humidity: 40 + (i % 20) as u8,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        });
    }

    history
}

/// Generate realistic radon history.
fn generate_radon_history(count: usize) -> Vec<HistoryRecord> {
    let mut history = Vec::with_capacity(count);
    let now = OffsetDateTime::now_utc();
    let interval_mins = 60i64; // Hourly readings

    for i in 0..count {
        let offset = Duration::minutes((count - 1 - i) as i64 * interval_mins);
        let timestamp = now - offset;

        // Radon varies slowly with some daily pattern
        let base_radon = 75.0 + ((i as f32 * 0.5).sin() * 20.0);
        let noise = ((i * 13) % 30) as f32 - 15.0;
        let radon = (base_radon + noise).clamp(40.0, 150.0) as u32;

        history.push(HistoryRecord {
            timestamp,
            co2: 0,
            temperature: 18.0 + (i % 10) as f32 * 0.1,
            pressure: 1012.0 + (i % 5) as f32 * 0.3,
            humidity: 52 + (i % 10) as u8,
            radon: Some(radon),
            radiation_rate: None,
            radiation_total: None,
        });
    }

    history
}

