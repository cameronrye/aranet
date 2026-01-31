//! MQTT publisher for broadcasting Aranet sensor readings.
//!
//! This module provides an MQTT client that subscribes to the internal reading
//! broadcast channel and publishes readings to an MQTT broker.
//!
//! # Topic Structure
//!
//! Readings are published to topics with the following structure:
//!
//! - `{prefix}/{device}/json` - Full reading as JSON
//! - `{prefix}/{device}/co2` - CO2 value (ppm)
//! - `{prefix}/{device}/temperature` - Temperature (°C)
//! - `{prefix}/{device}/humidity` - Humidity (%)
//! - `{prefix}/{device}/pressure` - Pressure (hPa)
//! - `{prefix}/{device}/battery` - Battery level (%)
//!
//! Where `{prefix}` is configurable (default: "aranet") and `{device}` is
//! the device alias or address.
//!
//! # Example Configuration
//!
//! ```toml
//! [mqtt]
//! enabled = true
//! broker = "mqtt://localhost:1883"
//! topic_prefix = "home/sensors"
//! qos = 1
//! retain = true
//! ```
//!
//! # Reconnection
//!
//! The client automatically reconnects if the connection is lost. Connection
//! errors are logged but don't stop the publisher task.

use std::sync::Arc;
use std::time::Duration;

use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::config::MqttConfig;
use crate::state::{AppState, ReadingEvent};

/// MQTT publisher that forwards readings to an MQTT broker.
pub struct MqttPublisher {
    state: Arc<AppState>,
}

impl MqttPublisher {
    /// Create a new MQTT publisher.
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Start the MQTT publisher.
    ///
    /// This spawns a background task that:
    /// 1. Connects to the configured MQTT broker
    /// 2. Subscribes to the internal readings broadcast channel
    /// 3. Publishes each reading to the broker
    ///
    /// Returns immediately; publishing happens in the background.
    pub async fn start(&self) {
        let config = self.state.config.read().await;
        let mqtt_config = config.mqtt.clone();
        drop(config);

        if !mqtt_config.enabled {
            info!("MQTT publisher is disabled");
            return;
        }

        info!("Starting MQTT publisher to {}", mqtt_config.broker);

        let state = Arc::clone(&self.state);
        let stop_rx = self.state.collector.subscribe_stop();

        tokio::spawn(async move {
            run_mqtt_publisher(state, mqtt_config, stop_rx).await;
        });
    }
}

/// Run the MQTT publisher loop.
async fn run_mqtt_publisher(
    state: Arc<AppState>,
    config: MqttConfig,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) {
    // Parse broker URL
    let (host, port, use_tls) = match parse_broker_url(&config.broker) {
        Ok(parsed) => parsed,
        Err(e) => {
            error!("Invalid MQTT broker URL: {}", e);
            return;
        }
    };

    // Configure MQTT client
    let mut mqtt_options = MqttOptions::new(&config.client_id, host, port);
    mqtt_options.set_keep_alive(Duration::from_secs(config.keep_alive));

    // Set credentials if provided
    if let (Some(username), Some(password)) = (&config.username, &config.password) {
        mqtt_options.set_credentials(username, password);
    }

    // Enable TLS if using mqtts://
    if use_tls {
        // For TLS, we use the native-tls transport
        // Note: This requires the broker to have a valid certificate
        mqtt_options.set_transport(rumqttc::Transport::tls_with_default_config());
    }

    let qos = match config.qos {
        0 => QoS::AtMostOnce,
        1 => QoS::AtLeastOnce,
        _ => QoS::ExactlyOnce,
    };

    // Create MQTT client
    let (client, mut eventloop) = AsyncClient::new(mqtt_options, 100);

    // Subscribe to readings broadcast
    let mut readings_rx = state.readings_tx.subscribe();

    info!(
        "MQTT publisher connected to {} with prefix '{}'",
        config.broker, config.topic_prefix
    );

    // Spawn event loop handler
    tokio::spawn(async move {
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(ack))) => {
                    info!("MQTT connected: {:?}", ack);
                }
                Ok(Event::Incoming(Packet::PingResp)) => {
                    debug!("MQTT ping response received");
                }
                Ok(Event::Outgoing(_)) => {
                    // Outgoing events are normal, no need to log
                }
                Ok(_) => {}
                Err(e) => {
                    warn!("MQTT connection error: {}. Reconnecting...", e);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });

    // Main publishing loop
    loop {
        tokio::select! {
            result = readings_rx.recv() => {
                match result {
                    Ok(event) => {
                        if let Err(e) = publish_reading(&client, &config, &event, qos).await {
                            warn!("Failed to publish reading: {}", e);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("MQTT publisher lagged, missed {} readings", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("Readings channel closed, stopping MQTT publisher");
                        break;
                    }
                }
            }
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    info!("MQTT publisher received stop signal");
                    break;
                }
            }
        }
    }

    // Disconnect gracefully
    if let Err(e) = client.disconnect().await {
        debug!("Error disconnecting MQTT client: {}", e);
    }

    info!("MQTT publisher stopped");
}

/// Publish a reading to MQTT topics.
async fn publish_reading(
    client: &AsyncClient,
    config: &MqttConfig,
    event: &ReadingEvent,
    qos: QoS,
) -> Result<(), rumqttc::ClientError> {
    let device_name = sanitize_topic_segment(&event.device_id);
    let prefix = &config.topic_prefix;
    let retain = config.retain;

    // Publish full JSON reading
    let json_topic = format!("{}/{}/json", prefix, device_name);
    let json_payload = serde_json::to_string(&event.reading).unwrap_or_default();
    client
        .publish(&json_topic, qos, retain, json_payload.as_bytes())
        .await?;

    // Publish individual metrics
    let reading = &event.reading;

    // CO2
    let co2_topic = format!("{}/{}/co2", prefix, device_name);
    client
        .publish(&co2_topic, qos, retain, reading.co2.to_string().as_bytes())
        .await?;

    // Temperature
    let temp_topic = format!("{}/{}/temperature", prefix, device_name);
    client
        .publish(
            &temp_topic,
            qos,
            retain,
            format!("{:.2}", reading.temperature).as_bytes(),
        )
        .await?;

    // Humidity
    let humidity_topic = format!("{}/{}/humidity", prefix, device_name);
    client
        .publish(
            &humidity_topic,
            qos,
            retain,
            reading.humidity.to_string().as_bytes(),
        )
        .await?;

    // Pressure
    let pressure_topic = format!("{}/{}/pressure", prefix, device_name);
    client
        .publish(
            &pressure_topic,
            qos,
            retain,
            format!("{:.2}", reading.pressure).as_bytes(),
        )
        .await?;

    // Battery
    let battery_topic = format!("{}/{}/battery", prefix, device_name);
    client
        .publish(
            &battery_topic,
            qos,
            retain,
            reading.battery.to_string().as_bytes(),
        )
        .await?;

    // Status
    let status_topic = format!("{}/{}/status", prefix, device_name);
    let status_str = match reading.status {
        aranet_types::Status::Green => "green",
        aranet_types::Status::Yellow => "yellow",
        aranet_types::Status::Red => "red",
        aranet_types::Status::Error => "error",
        _ => "unknown",
    };
    client
        .publish(&status_topic, qos, retain, status_str.as_bytes())
        .await?;

    // Radon (if available)
    if let Some(radon) = reading.radon {
        let radon_topic = format!("{}/{}/radon", prefix, device_name);
        client
            .publish(&radon_topic, qos, retain, radon.to_string().as_bytes())
            .await?;
    }

    // Radiation rate (if available)
    if let Some(rate) = reading.radiation_rate {
        let rate_topic = format!("{}/{}/radiation_rate", prefix, device_name);
        client
            .publish(&rate_topic, qos, retain, format!("{:.4}", rate).as_bytes())
            .await?;
    }

    // Radiation total (if available)
    if let Some(total) = reading.radiation_total {
        let total_topic = format!("{}/{}/radiation_total", prefix, device_name);
        client
            .publish(
                &total_topic,
                qos,
                retain,
                format!("{:.6}", total).as_bytes(),
            )
            .await?;
    }

    debug!(
        "Published reading for {} to MQTT (CO2={})",
        event.device_id, reading.co2
    );

    Ok(())
}

/// Parse an MQTT broker URL into (host, port, use_tls).
fn parse_broker_url(url: &str) -> Result<(String, u16, bool), String> {
    let (scheme, rest) = if let Some(stripped) = url.strip_prefix("mqtt://") {
        ("mqtt", stripped)
    } else if let Some(stripped) = url.strip_prefix("mqtts://") {
        ("mqtts", stripped)
    } else {
        return Err("Invalid scheme: URL must start with mqtt:// or mqtts://".to_string());
    };

    let use_tls = scheme == "mqtts";
    let default_port = if use_tls { 8883 } else { 1883 };

    // Parse host:port
    let (host, port) = if let Some((h, p)) = rest.rsplit_once(':') {
        let port = p
            .parse::<u16>()
            .map_err(|_| format!("Invalid port: {}", p))?;
        (h.to_string(), port)
    } else {
        (rest.to_string(), default_port)
    };

    if host.is_empty() {
        return Err("Host cannot be empty".to_string());
    }

    Ok((host, port, use_tls))
}

/// Sanitize a device name for use in MQTT topic.
///
/// MQTT topics cannot contain '#' or '+' wildcards, and should avoid spaces.
fn sanitize_topic_segment(s: &str) -> String {
    s.replace(['#', '+', ' ', '/'], "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_broker_url_mqtt() {
        let (host, port, tls) = parse_broker_url("mqtt://localhost:1883").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 1883);
        assert!(!tls);
    }

    #[test]
    fn test_parse_broker_url_mqtts() {
        let (host, port, tls) = parse_broker_url("mqtts://broker.example.com:8883").unwrap();
        assert_eq!(host, "broker.example.com");
        assert_eq!(port, 8883);
        assert!(tls);
    }

    #[test]
    fn test_parse_broker_url_default_port() {
        let (host, port, tls) = parse_broker_url("mqtt://localhost").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 1883);
        assert!(!tls);

        let (host, port, tls) = parse_broker_url("mqtts://secure.example.com").unwrap();
        assert_eq!(host, "secure.example.com");
        assert_eq!(port, 8883);
        assert!(tls);
    }

    #[test]
    fn test_parse_broker_url_invalid_scheme() {
        assert!(parse_broker_url("http://localhost:1883").is_err());
        assert!(parse_broker_url("localhost:1883").is_err());
    }

    #[test]
    fn test_parse_broker_url_empty_host() {
        assert!(parse_broker_url("mqtt://:1883").is_err());
    }

    #[test]
    fn test_sanitize_topic_segment() {
        assert_eq!(sanitize_topic_segment("Aranet4 17C3C"), "Aranet4_17C3C");
        assert_eq!(sanitize_topic_segment("device#1"), "device_1");
        assert_eq!(sanitize_topic_segment("sensor+temp"), "sensor_temp");
        assert_eq!(sanitize_topic_segment("path/to/device"), "path_to_device");
    }

    #[test]
    fn test_sanitize_topic_segment_normal() {
        assert_eq!(sanitize_topic_segment("office"), "office");
        assert_eq!(sanitize_topic_segment("kitchen-sensor"), "kitchen-sensor");
    }
}
