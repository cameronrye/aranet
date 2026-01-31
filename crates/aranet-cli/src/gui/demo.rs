//! Demo mode for generating screenshots with mock data.
//!
//! This module provides deterministic mock data for reproducible screenshots.

use aranet_core::BluetoothRange;
use aranet_core::settings::DeviceSettings;
use aranet_types::{CurrentReading, DeviceType, HistoryRecord, Status};
use time::{Duration, OffsetDateTime};

use super::types::{ConnectionState, DeviceState};

/// Generate demo devices with realistic mock data for screenshots.
/// Matches the real devices: Aranet4 17C3C (office) and AranetRn+ 306B8 (radon).
pub fn create_demo_devices() -> Vec<DeviceState> {
    vec![create_aranet4_device(), create_radon_device()]
}

/// Create an Aranet4 device matching real device: Aranet4 17C3C (office).
fn create_aranet4_device() -> DeviceState {
    let reading = CurrentReading {
        co2: 633,
        temperature: 18.0,
        pressure: 1002.6,
        humidity: 19,
        battery: 96,
        status: Status::Green,
        interval: 300,
        age: 8,
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
        id: "921df903-d89b-9c97-6ffa-bb80d7c8e471".to_string(),
        name: Some("Aranet4 17C3C".to_string()),
        device_type: Some(DeviceType::Aranet4),
        rssi: Some(-75),
        connection: ConnectionState::Connected,
        reading: Some(reading),
        previous_reading: None,
        history,
        syncing_history: false,
        settings: Some(settings),
        reading_from_cache: false,
        last_sync: Some(OffsetDateTime::now_utc() - Duration::minutes(5)),
    }
}

/// Create an AranetRn+ device matching real device: AranetRn+ 306B8 (radon).
fn create_radon_device() -> DeviceState {
    // 2.97 pCi/L = ~110 Bq/m³
    let reading = CurrentReading {
        co2: 0,
        temperature: 13.4,
        pressure: 992.5,
        humidity: 23,
        battery: 94,
        status: Status::Yellow,
        interval: 600,
        age: 175,
        captured_at: Some(OffsetDateTime::now_utc()),
        radon: Some(110),
        radon_avg_24h: Some(118),
        radon_avg_7d: Some(103),
        radon_avg_30d: Some(128),
        radiation_rate: None,
        radiation_total: None,
    };

    DeviceState {
        id: "387c18c7-299f-cc32-d01c-6cf29a8d3ca5".to_string(),
        name: Some("AranetRn+ 306B8".to_string()),
        device_type: Some(DeviceType::AranetRadon),
        rssi: Some(-77),
        connection: ConnectionState::Connected,
        reading: Some(reading),
        previous_reading: None,
        history: generate_radon_history(24 * 7), // 7 days at 1-hour intervals
        syncing_history: false,
        settings: None,
        reading_from_cache: false,
        last_sync: Some(OffsetDateTime::now_utc() - Duration::minutes(3)),
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
