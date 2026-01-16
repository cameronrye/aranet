//! Data validation and bounds checking for sensor readings.
//!
//! This module provides validation utilities to detect anomalous readings
//! and flag potential sensor issues.
//!
//! # Example
//!
//! ```
//! use aranet_core::ReadingValidator;
//! use aranet_core::validation::ValidatorConfig;
//! use aranet_types::{CurrentReading, Status};
//!
//! // Create a validator with default config
//! let validator = ReadingValidator::default();
//!
//! // Create a reading to validate
//! let reading = CurrentReading {
//!     co2: 800,
//!     temperature: 22.5,
//!     pressure: 1013.0,
//!     humidity: 45,
//!     battery: 85,
//!     status: Status::Green,
//!     interval: 300,
//!     age: 60,
//!     captured_at: None,
//!     radon: None,
//!     radiation_rate: None,
//!     radiation_total: None,
//! };
//!
//! let result = validator.validate(&reading);
//! assert!(result.is_valid);
//! assert!(!result.has_warnings());
//! ```

use serde::{Deserialize, Serialize};

use aranet_types::{CurrentReading, DeviceType};

/// Warning types for validation issues.
///
/// This enum is marked `#[non_exhaustive]` to allow adding new warning types
/// in future versions without breaking downstream code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ValidationWarning {
    /// CO2 reading is below minimum expected value.
    Co2TooLow { value: u16, min: u16 },
    /// CO2 reading is above maximum expected value.
    Co2TooHigh { value: u16, max: u16 },
    /// Temperature is below minimum expected value.
    TemperatureTooLow { value: f32, min: f32 },
    /// Temperature is above maximum expected value.
    TemperatureTooHigh { value: f32, max: f32 },
    /// Pressure is below minimum expected value.
    PressureTooLow { value: f32, min: f32 },
    /// Pressure is above maximum expected value.
    PressureTooHigh { value: f32, max: f32 },
    /// Humidity is above 100%.
    HumidityOutOfRange { value: u8 },
    /// Battery level is above 100%.
    BatteryOutOfRange { value: u8 },
    /// CO2 is zero which may indicate sensor error.
    Co2Zero,
    /// All values are zero which may indicate communication error.
    AllZeros,
    /// Radon reading is above maximum expected value.
    RadonTooHigh { value: u32, max: u32 },
    /// Radiation rate is above maximum expected value.
    RadiationRateTooHigh { value: f32, max: f32 },
    /// Radiation total is above maximum expected value.
    RadiationTotalTooHigh { value: f64, max: f64 },
}

impl std::fmt::Display for ValidationWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationWarning::Co2TooLow { value, min } => {
                write!(f, "CO2 {} ppm is below minimum {} ppm", value, min)
            }
            ValidationWarning::Co2TooHigh { value, max } => {
                write!(f, "CO2 {} ppm exceeds maximum {} ppm", value, max)
            }
            ValidationWarning::TemperatureTooLow { value, min } => {
                write!(f, "Temperature {}°C is below minimum {}°C", value, min)
            }
            ValidationWarning::TemperatureTooHigh { value, max } => {
                write!(f, "Temperature {}°C exceeds maximum {}°C", value, max)
            }
            ValidationWarning::PressureTooLow { value, min } => {
                write!(f, "Pressure {} hPa is below minimum {} hPa", value, min)
            }
            ValidationWarning::PressureTooHigh { value, max } => {
                write!(f, "Pressure {} hPa exceeds maximum {} hPa", value, max)
            }
            ValidationWarning::HumidityOutOfRange { value } => {
                write!(f, "Humidity {}% is out of valid range (0-100)", value)
            }
            ValidationWarning::BatteryOutOfRange { value } => {
                write!(f, "Battery {}% is out of valid range (0-100)", value)
            }
            ValidationWarning::Co2Zero => {
                write!(f, "CO2 reading is zero - possible sensor error")
            }
            ValidationWarning::AllZeros => {
                write!(f, "All readings are zero - possible communication error")
            }
            ValidationWarning::RadonTooHigh { value, max } => {
                write!(f, "Radon {} Bq/m³ exceeds maximum {} Bq/m³", value, max)
            }
            ValidationWarning::RadiationRateTooHigh { value, max } => {
                write!(
                    f,
                    "Radiation rate {} µSv/h exceeds maximum {} µSv/h",
                    value, max
                )
            }
            ValidationWarning::RadiationTotalTooHigh { value, max } => {
                write!(
                    f,
                    "Radiation total {} µSv exceeds maximum {} µSv",
                    value, max
                )
            }
        }
    }
}

/// Result of validating a reading.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether the reading passed validation.
    pub is_valid: bool,
    /// List of warnings (may be non-empty even if valid).
    pub warnings: Vec<ValidationWarning>,
}

impl ValidationResult {
    /// Create a successful validation result with no warnings.
    pub fn valid() -> Self {
        Self {
            is_valid: true,
            warnings: Vec::new(),
        }
    }

    /// Create an invalid result with the given warnings.
    pub fn invalid(warnings: Vec<ValidationWarning>) -> Self {
        Self {
            is_valid: false,
            warnings,
        }
    }

    /// Create a valid result with warnings.
    pub fn valid_with_warnings(warnings: Vec<ValidationWarning>) -> Self {
        Self {
            is_valid: true,
            warnings,
        }
    }

    /// Check if there are any warnings.
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }
}

/// Configuration for reading validation.
#[derive(Debug, Clone)]
pub struct ValidatorConfig {
    /// Minimum expected CO2 value (ppm).
    pub co2_min: u16,
    /// Maximum expected CO2 value (ppm).
    pub co2_max: u16,
    /// Minimum expected temperature (°C).
    pub temperature_min: f32,
    /// Maximum expected temperature (°C).
    pub temperature_max: f32,
    /// Minimum expected pressure (hPa).
    pub pressure_min: f32,
    /// Maximum expected pressure (hPa).
    pub pressure_max: f32,
    /// Maximum expected radon value (Bq/m³).
    pub radon_max: u32,
    /// Maximum expected radiation rate (µSv/h).
    pub radiation_rate_max: f32,
    /// Maximum expected radiation total (mSv).
    pub radiation_total_max: f64,
    /// Treat CO2 = 0 as an error.
    pub warn_on_zero_co2: bool,
    /// Treat all zeros as an error.
    pub warn_on_all_zeros: bool,
}

impl Default for ValidatorConfig {
    fn default() -> Self {
        Self {
            co2_min: 300,   // Outdoor ambient is ~400 ppm
            co2_max: 10000, // Very high but possible in some scenarios
            temperature_min: -40.0,
            temperature_max: 85.0,
            pressure_min: 300.0,           // Very high altitude
            pressure_max: 1100.0,          // Sea level or below
            radon_max: 1000,               // WHO action level is 100-300 Bq/m³
            radiation_rate_max: 100.0,     // Normal background is ~0.1-0.2 µSv/h
            radiation_total_max: 100000.0, // Reasonable upper bound for accumulated dose
            warn_on_zero_co2: true,
            warn_on_all_zeros: true,
        }
    }
}

impl ValidatorConfig {
    /// Create new validator config with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set minimum CO2 value (ppm).
    #[must_use]
    pub fn co2_min(mut self, min: u16) -> Self {
        self.co2_min = min;
        self
    }

    /// Set maximum CO2 value (ppm).
    #[must_use]
    pub fn co2_max(mut self, max: u16) -> Self {
        self.co2_max = max;
        self
    }

    /// Set CO2 range (min, max).
    #[must_use]
    pub fn co2_range(mut self, min: u16, max: u16) -> Self {
        self.co2_min = min;
        self.co2_max = max;
        self
    }

    /// Set minimum temperature (°C).
    #[must_use]
    pub fn temperature_min(mut self, min: f32) -> Self {
        self.temperature_min = min;
        self
    }

    /// Set maximum temperature (°C).
    #[must_use]
    pub fn temperature_max(mut self, max: f32) -> Self {
        self.temperature_max = max;
        self
    }

    /// Set temperature range (min, max).
    #[must_use]
    pub fn temperature_range(mut self, min: f32, max: f32) -> Self {
        self.temperature_min = min;
        self.temperature_max = max;
        self
    }

    /// Set minimum pressure (hPa).
    #[must_use]
    pub fn pressure_min(mut self, min: f32) -> Self {
        self.pressure_min = min;
        self
    }

    /// Set maximum pressure (hPa).
    #[must_use]
    pub fn pressure_max(mut self, max: f32) -> Self {
        self.pressure_max = max;
        self
    }

    /// Set pressure range (min, max).
    #[must_use]
    pub fn pressure_range(mut self, min: f32, max: f32) -> Self {
        self.pressure_min = min;
        self.pressure_max = max;
        self
    }

    /// Set whether to warn on CO2 = 0.
    #[must_use]
    pub fn warn_on_zero_co2(mut self, warn: bool) -> Self {
        self.warn_on_zero_co2 = warn;
        self
    }

    /// Set whether to warn on all zeros.
    #[must_use]
    pub fn warn_on_all_zeros(mut self, warn: bool) -> Self {
        self.warn_on_all_zeros = warn;
        self
    }

    /// Set maximum radon value (Bq/m³).
    #[must_use]
    pub fn radon_max(mut self, max: u32) -> Self {
        self.radon_max = max;
        self
    }

    /// Set maximum radiation rate (µSv/h).
    #[must_use]
    pub fn radiation_rate_max(mut self, max: f32) -> Self {
        self.radiation_rate_max = max;
        self
    }

    /// Set maximum radiation total (mSv).
    #[must_use]
    pub fn radiation_total_max(mut self, max: f64) -> Self {
        self.radiation_total_max = max;
        self
    }

    /// Create strict validation config (narrow ranges).
    pub fn strict() -> Self {
        Self {
            co2_min: 350,
            co2_max: 5000,
            temperature_min: -10.0,
            temperature_max: 50.0,
            pressure_min: 800.0,
            pressure_max: 1100.0,
            radon_max: 300, // WHO action level
            radiation_rate_max: 10.0,
            radiation_total_max: 10000.0,
            warn_on_zero_co2: true,
            warn_on_all_zeros: true,
        }
    }

    /// Create relaxed validation config (wide ranges).
    pub fn relaxed() -> Self {
        Self {
            co2_min: 0,
            co2_max: 20000,
            temperature_min: -50.0,
            temperature_max: 100.0,
            pressure_min: 200.0,
            pressure_max: 1200.0,
            radon_max: 5000,
            radiation_rate_max: 1000.0,
            radiation_total_max: 1000000.0,
            warn_on_zero_co2: false,
            warn_on_all_zeros: false,
        }
    }

    /// Create validation config optimized for Aranet4 (CO2 sensor).
    ///
    /// Aranet4 measures CO2, temperature, humidity, and pressure.
    /// This preset uses appropriate ranges for indoor air quality monitoring.
    pub fn for_aranet4() -> Self {
        Self {
            co2_min: 300,   // Outdoor ambient is ~400 ppm
            co2_max: 10000, // Aranet4 max range
            temperature_min: -40.0,
            temperature_max: 60.0, // Aranet4 operating range
            pressure_min: 300.0,
            pressure_max: 1100.0,
            radon_max: 0,             // Not applicable
            radiation_rate_max: 0.0,  // Not applicable
            radiation_total_max: 0.0, // Not applicable
            warn_on_zero_co2: true,
            warn_on_all_zeros: true,
        }
    }

    /// Create validation config optimized for Aranet2 (temperature/humidity sensor).
    ///
    /// Aranet2 measures only temperature and humidity.
    /// CO2 and pressure validation is disabled.
    ///
    /// Note: This preset is based on device specifications. Actual testing
    /// with an Aranet2 device may reveal adjustments needed.
    pub fn for_aranet2() -> Self {
        Self {
            co2_min: 0,     // Not applicable - disable CO2 validation
            co2_max: 65535, // Not applicable
            temperature_min: -40.0,
            temperature_max: 60.0,
            pressure_min: 0.0,        // Not applicable
            pressure_max: 2000.0,     // Not applicable
            radon_max: 0,             // Not applicable
            radiation_rate_max: 0.0,  // Not applicable
            radiation_total_max: 0.0, // Not applicable
            warn_on_zero_co2: false,  // CO2 is not measured
            warn_on_all_zeros: false,
        }
    }

    /// Create validation config optimized for AranetRn+ (radon sensor).
    ///
    /// AranetRn+ measures radon, temperature, humidity, and pressure.
    /// CO2 validation is disabled.
    ///
    /// Note: This preset is based on device specifications. Actual testing
    /// with an AranetRn+ device may reveal adjustments needed.
    pub fn for_aranet_radon() -> Self {
        Self {
            co2_min: 0,     // Not applicable
            co2_max: 65535, // Not applicable
            temperature_min: -40.0,
            temperature_max: 60.0,
            pressure_min: 300.0,
            pressure_max: 1100.0,
            radon_max: 1000,          // WHO action level is 100-300 Bq/m³
            radiation_rate_max: 0.0,  // Not applicable
            radiation_total_max: 0.0, // Not applicable
            warn_on_zero_co2: false,
            warn_on_all_zeros: false,
        }
    }

    /// Create validation config optimized for Aranet Radiation sensor.
    ///
    /// Aranet Radiation measures gamma radiation rate and accumulated dose.
    /// CO2 and radon validation is disabled.
    ///
    /// Note: This preset is based on device specifications. Actual testing
    /// with an Aranet Radiation device may reveal adjustments needed.
    pub fn for_aranet_radiation() -> Self {
        Self {
            co2_min: 0,     // Not applicable
            co2_max: 65535, // Not applicable
            temperature_min: -40.0,
            temperature_max: 60.0,
            pressure_min: 300.0,
            pressure_max: 1100.0,
            radon_max: 0,                  // Not applicable
            radiation_rate_max: 100.0,     // Normal background is ~0.1-0.2 µSv/h
            radiation_total_max: 100000.0, // Reasonable upper bound
            warn_on_zero_co2: false,
            warn_on_all_zeros: false,
        }
    }

    /// Create validation config for a specific device type.
    ///
    /// Automatically selects the appropriate preset based on the device type:
    /// - [`DeviceType::Aranet4`] → [`for_aranet4()`](Self::for_aranet4)
    /// - [`DeviceType::Aranet2`] → [`for_aranet2()`](Self::for_aranet2)
    /// - [`DeviceType::AranetRadon`] → [`for_aranet_radon()`](Self::for_aranet_radon)
    /// - [`DeviceType::AranetRadiation`] → [`for_aranet_radiation()`](Self::for_aranet_radiation)
    /// - Unknown types → default config
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_core::validation::ValidatorConfig;
    /// use aranet_types::DeviceType;
    ///
    /// let config = ValidatorConfig::for_device(DeviceType::Aranet4);
    /// assert_eq!(config.co2_max, 10000);
    ///
    /// let config = ValidatorConfig::for_device(DeviceType::AranetRadon);
    /// assert_eq!(config.radon_max, 1000);
    /// ```
    #[must_use]
    pub fn for_device(device_type: DeviceType) -> Self {
        match device_type {
            DeviceType::Aranet4 => Self::for_aranet4(),
            DeviceType::Aranet2 => Self::for_aranet2(),
            DeviceType::AranetRadon => Self::for_aranet_radon(),
            DeviceType::AranetRadiation => Self::for_aranet_radiation(),
            _ => Self::default(),
        }
    }
}

/// Validator for sensor readings.
#[derive(Debug, Clone, Default)]
pub struct ReadingValidator {
    config: ValidatorConfig,
}

impl ReadingValidator {
    /// Create a new validator with the given configuration.
    pub fn new(config: ValidatorConfig) -> Self {
        Self { config }
    }

    /// Get the configuration.
    pub fn config(&self) -> &ValidatorConfig {
        &self.config
    }

    /// Validate a sensor reading.
    pub fn validate(&self, reading: &CurrentReading) -> ValidationResult {
        let mut warnings = Vec::new();

        // Check for all zeros (use approximate comparison for floats)
        if self.config.warn_on_all_zeros
            && reading.co2 == 0
            && reading.temperature.abs() < f32::EPSILON
            && reading.pressure.abs() < f32::EPSILON
            && reading.humidity == 0
        {
            warnings.push(ValidationWarning::AllZeros);
            return ValidationResult::invalid(warnings);
        }

        // Check CO2
        if reading.co2 > 0 {
            if reading.co2 < self.config.co2_min {
                warnings.push(ValidationWarning::Co2TooLow {
                    value: reading.co2,
                    min: self.config.co2_min,
                });
            }
            if reading.co2 > self.config.co2_max {
                warnings.push(ValidationWarning::Co2TooHigh {
                    value: reading.co2,
                    max: self.config.co2_max,
                });
            }
        } else if self.config.warn_on_zero_co2 {
            warnings.push(ValidationWarning::Co2Zero);
        }

        // Check temperature
        if reading.temperature < self.config.temperature_min {
            warnings.push(ValidationWarning::TemperatureTooLow {
                value: reading.temperature,
                min: self.config.temperature_min,
            });
        }
        if reading.temperature > self.config.temperature_max {
            warnings.push(ValidationWarning::TemperatureTooHigh {
                value: reading.temperature,
                max: self.config.temperature_max,
            });
        }

        // Check pressure (skip if 0, might be Aranet2)
        if reading.pressure > 0.0 {
            if reading.pressure < self.config.pressure_min {
                warnings.push(ValidationWarning::PressureTooLow {
                    value: reading.pressure,
                    min: self.config.pressure_min,
                });
            }
            if reading.pressure > self.config.pressure_max {
                warnings.push(ValidationWarning::PressureTooHigh {
                    value: reading.pressure,
                    max: self.config.pressure_max,
                });
            }
        }

        // Check humidity
        if reading.humidity > 100 {
            warnings.push(ValidationWarning::HumidityOutOfRange {
                value: reading.humidity,
            });
        }

        // Check battery
        if reading.battery > 100 {
            warnings.push(ValidationWarning::BatteryOutOfRange {
                value: reading.battery,
            });
        }

        // Check radon (if present)
        if let Some(radon) = reading.radon
            && radon > self.config.radon_max
        {
            warnings.push(ValidationWarning::RadonTooHigh {
                value: radon,
                max: self.config.radon_max,
            });
        }

        // Check radiation rate (if present)
        if let Some(rate) = reading.radiation_rate
            && rate > self.config.radiation_rate_max
        {
            warnings.push(ValidationWarning::RadiationRateTooHigh {
                value: rate,
                max: self.config.radiation_rate_max,
            });
        }

        // Check radiation total (if present)
        if let Some(total) = reading.radiation_total
            && total > self.config.radiation_total_max
        {
            warnings.push(ValidationWarning::RadiationTotalTooHigh {
                value: total,
                max: self.config.radiation_total_max,
            });
        }

        if warnings.is_empty() {
            ValidationResult::valid()
        } else {
            // Determine if any warnings are critical
            let has_critical = warnings.iter().any(|w| {
                matches!(
                    w,
                    ValidationWarning::AllZeros
                        | ValidationWarning::Co2TooHigh { .. }
                        | ValidationWarning::TemperatureTooHigh { .. }
                        | ValidationWarning::RadonTooHigh { .. }
                        | ValidationWarning::RadiationRateTooHigh { .. }
                )
            });

            if has_critical {
                ValidationResult::invalid(warnings)
            } else {
                ValidationResult::valid_with_warnings(warnings)
            }
        }
    }

    /// Quick check if a CO2 value is within expected range.
    pub fn is_co2_valid(&self, co2: u16) -> bool {
        co2 >= self.config.co2_min && co2 <= self.config.co2_max
    }

    /// Quick check if a temperature value is within expected range.
    pub fn is_temperature_valid(&self, temp: f32) -> bool {
        temp >= self.config.temperature_min && temp <= self.config.temperature_max
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aranet_types::Status;

    fn make_reading(co2: u16, temp: f32, pressure: f32, humidity: u8) -> CurrentReading {
        CurrentReading {
            co2,
            temperature: temp,
            pressure,
            humidity,
            battery: 80,
            status: Status::Green,
            interval: 300,
            age: 60,
            captured_at: None,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        }
    }

    #[test]
    fn test_valid_reading() {
        let validator = ReadingValidator::default();
        let reading = make_reading(800, 22.5, 1013.2, 50);
        let result = validator.validate(&reading);
        assert!(result.is_valid);
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_co2_too_high() {
        let validator = ReadingValidator::default();
        let reading = make_reading(15000, 22.5, 1013.2, 50);
        let result = validator.validate(&reading);
        assert!(!result.is_valid);
        assert!(
            result
                .warnings
                .iter()
                .any(|w| matches!(w, ValidationWarning::Co2TooHigh { .. }))
        );
    }

    #[test]
    fn test_all_zeros() {
        let validator = ReadingValidator::default();
        let reading = make_reading(0, 0.0, 0.0, 0);
        let result = validator.validate(&reading);
        assert!(!result.is_valid);
        assert!(
            result
                .warnings
                .iter()
                .any(|w| matches!(w, ValidationWarning::AllZeros))
        );
    }

    #[test]
    fn test_humidity_out_of_range() {
        let validator = ReadingValidator::default();
        let reading = make_reading(800, 22.5, 1013.2, 150);
        let result = validator.validate(&reading);
        assert!(result.has_warnings());
        assert!(
            result
                .warnings
                .iter()
                .any(|w| matches!(w, ValidationWarning::HumidityOutOfRange { .. }))
        );
    }

    #[test]
    fn test_for_device_aranet4() {
        let config = ValidatorConfig::for_device(DeviceType::Aranet4);
        assert_eq!(config.co2_min, 300);
        assert_eq!(config.co2_max, 10000);
        assert!(config.warn_on_zero_co2);
    }

    #[test]
    fn test_for_device_aranet2() {
        let config = ValidatorConfig::for_device(DeviceType::Aranet2);
        assert_eq!(config.co2_min, 0); // CO2 validation disabled
        assert!(!config.warn_on_zero_co2);
    }

    #[test]
    fn test_for_device_aranet_radon() {
        let config = ValidatorConfig::for_device(DeviceType::AranetRadon);
        assert_eq!(config.radon_max, 1000);
        assert!(!config.warn_on_zero_co2);
    }

    #[test]
    fn test_for_device_aranet_radiation() {
        let config = ValidatorConfig::for_device(DeviceType::AranetRadiation);
        assert_eq!(config.radiation_rate_max, 100.0);
        assert_eq!(config.radiation_total_max, 100000.0);
        assert!(!config.warn_on_zero_co2);
    }
}
