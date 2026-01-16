//! CO2 level thresholds and categorization.
//!
//! This module provides configurable thresholds for categorizing CO2 levels
//! and other sensor readings into actionable categories.
//!
//! # Example
//!
//! ```
//! use aranet_core::{Thresholds, Co2Level};
//!
//! // Use default thresholds
//! let thresholds = Thresholds::default();
//!
//! // Evaluate a CO2 reading
//! let level = thresholds.evaluate_co2(800);
//! assert_eq!(level, Co2Level::Good);
//!
//! // Get action recommendation
//! println!("{}", level.action());
//! ```

use serde::{Deserialize, Serialize};

use aranet_types::CurrentReading;

/// CO2 level category based on concentration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Co2Level {
    /// Excellent air quality (typically < 600 ppm).
    Excellent,
    /// Good air quality (typically 600-800 ppm).
    Good,
    /// Moderate air quality (typically 800-1000 ppm).
    Moderate,
    /// Poor air quality (typically 1000-1500 ppm).
    Poor,
    /// Very poor air quality (typically 1500-2000 ppm).
    VeryPoor,
    /// Hazardous air quality (typically > 2000 ppm).
    Hazardous,
}

impl Co2Level {
    /// Get a human-readable description of the CO2 level.
    pub fn description(&self) -> &'static str {
        match self {
            Co2Level::Excellent => "Excellent - outdoor air quality",
            Co2Level::Good => "Good - typical indoor air",
            Co2Level::Moderate => "Moderate - consider ventilation",
            Co2Level::Poor => "Poor - ventilation recommended",
            Co2Level::VeryPoor => "Very Poor - ventilate immediately",
            Co2Level::Hazardous => "Hazardous - leave area if possible",
        }
    }

    /// Get the suggested action for this CO2 level.
    pub fn action(&self) -> &'static str {
        match self {
            Co2Level::Excellent | Co2Level::Good => "No action needed",
            Co2Level::Moderate => "Consider opening windows",
            Co2Level::Poor => "Open windows or turn on ventilation",
            Co2Level::VeryPoor => "Immediate ventilation required",
            Co2Level::Hazardous => "Leave area and ventilate thoroughly",
        }
    }
}

/// Configuration for CO2 thresholds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThresholdConfig {
    /// Upper bound for Excellent level.
    pub excellent_max: u16,
    /// Upper bound for Good level.
    pub good_max: u16,
    /// Upper bound for Moderate level.
    pub moderate_max: u16,
    /// Upper bound for Poor level.
    pub poor_max: u16,
    /// Upper bound for Very Poor level.
    pub very_poor_max: u16,
    // Above very_poor_max is Hazardous
}

impl Default for ThresholdConfig {
    fn default() -> Self {
        Self {
            excellent_max: 600,
            good_max: 800,
            moderate_max: 1000,
            poor_max: 1500,
            very_poor_max: 2000,
        }
    }
}

impl ThresholdConfig {
    /// Create strict thresholds suitable for sensitive environments.
    pub fn strict() -> Self {
        Self {
            excellent_max: 450,
            good_max: 600,
            moderate_max: 800,
            poor_max: 1000,
            very_poor_max: 1500,
        }
    }

    /// Create relaxed thresholds for industrial environments.
    pub fn relaxed() -> Self {
        Self {
            excellent_max: 800,
            good_max: 1000,
            moderate_max: 1500,
            poor_max: 2500,
            very_poor_max: 5000,
        }
    }
}

/// Threshold evaluator for sensor readings.
#[derive(Debug, Clone, Default)]
pub struct Thresholds {
    config: ThresholdConfig,
}

impl Thresholds {
    /// Create a new threshold evaluator with the given configuration.
    pub fn new(config: ThresholdConfig) -> Self {
        Self { config }
    }

    /// Create a threshold evaluator with strict thresholds.
    pub fn strict() -> Self {
        Self::new(ThresholdConfig::strict())
    }

    /// Create a threshold evaluator with relaxed thresholds.
    pub fn relaxed() -> Self {
        Self::new(ThresholdConfig::relaxed())
    }

    /// Get the configuration.
    pub fn config(&self) -> &ThresholdConfig {
        &self.config
    }

    /// Evaluate the CO2 level from a reading.
    pub fn evaluate_co2(&self, co2_ppm: u16) -> Co2Level {
        if co2_ppm <= self.config.excellent_max {
            Co2Level::Excellent
        } else if co2_ppm <= self.config.good_max {
            Co2Level::Good
        } else if co2_ppm <= self.config.moderate_max {
            Co2Level::Moderate
        } else if co2_ppm <= self.config.poor_max {
            Co2Level::Poor
        } else if co2_ppm <= self.config.very_poor_max {
            Co2Level::VeryPoor
        } else {
            Co2Level::Hazardous
        }
    }

    /// Evaluate the CO2 level from a CurrentReading.
    pub fn evaluate_reading(&self, reading: &CurrentReading) -> Co2Level {
        self.evaluate_co2(reading.co2)
    }

    /// Check if a CO2 reading exceeds a specific threshold.
    pub fn exceeds_threshold(&self, co2_ppm: u16, level: Co2Level) -> bool {
        match level {
            Co2Level::Excellent => co2_ppm > self.config.excellent_max,
            Co2Level::Good => co2_ppm > self.config.good_max,
            Co2Level::Moderate => co2_ppm > self.config.moderate_max,
            Co2Level::Poor => co2_ppm > self.config.poor_max,
            Co2Level::VeryPoor => co2_ppm > self.config.very_poor_max,
            Co2Level::Hazardous => true, // Already at highest level
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_thresholds() {
        let t = Thresholds::default();
        assert_eq!(t.evaluate_co2(400), Co2Level::Excellent);
        assert_eq!(t.evaluate_co2(700), Co2Level::Good);
        assert_eq!(t.evaluate_co2(900), Co2Level::Moderate);
        assert_eq!(t.evaluate_co2(1200), Co2Level::Poor);
        assert_eq!(t.evaluate_co2(1800), Co2Level::VeryPoor);
        assert_eq!(t.evaluate_co2(2500), Co2Level::Hazardous);
    }

    #[test]
    fn test_strict_thresholds() {
        let t = Thresholds::strict();
        assert_eq!(t.evaluate_co2(400), Co2Level::Excellent);
        assert_eq!(t.evaluate_co2(500), Co2Level::Good);
        assert_eq!(t.evaluate_co2(700), Co2Level::Moderate);
        assert_eq!(t.evaluate_co2(900), Co2Level::Poor);
    }

    #[test]
    fn test_relaxed_thresholds() {
        let t = Thresholds::relaxed();
        assert_eq!(t.evaluate_co2(700), Co2Level::Excellent);
        assert_eq!(t.evaluate_co2(900), Co2Level::Good);
        assert_eq!(t.evaluate_co2(1200), Co2Level::Moderate);
    }

    #[test]
    fn test_boundary_values() {
        let t = Thresholds::default();
        // Exact boundaries
        assert_eq!(t.evaluate_co2(600), Co2Level::Excellent);
        assert_eq!(t.evaluate_co2(601), Co2Level::Good);
        assert_eq!(t.evaluate_co2(800), Co2Level::Good);
        assert_eq!(t.evaluate_co2(801), Co2Level::Moderate);
    }

    #[test]
    fn test_co2_level_descriptions() {
        assert!(Co2Level::Excellent.description().contains("Excellent"));
        assert!(Co2Level::Hazardous.description().contains("Hazardous"));
    }

    #[test]
    fn test_co2_level_actions() {
        assert!(Co2Level::Excellent.action().contains("No action"));
        assert!(Co2Level::VeryPoor.action().contains("Immediate"));
    }

    #[test]
    fn test_exceeds_threshold() {
        let t = Thresholds::default();
        assert!(!t.exceeds_threshold(600, Co2Level::Excellent));
        assert!(t.exceeds_threshold(601, Co2Level::Excellent));
        assert!(!t.exceeds_threshold(1000, Co2Level::Moderate));
        assert!(t.exceeds_threshold(1001, Co2Level::Moderate));
    }
}
