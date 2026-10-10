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
//!
//! Readings wait in the client's queue of 100 messages until they are sent.
//! While the broker is unreachable or too slow, a reading that doesn't fit is
//! dropped instead of waiting, with a warning at most once a minute, so the
//! publisher keeps up. When the broker is back, the messages queued at the
//! start of the outage (at most 100) are sent first, then fresh readings.
//! Home Assistant discovery waits for room for up to 10 s.

use std::sync::Arc;
use std::time::Duration;

use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, TlsConfiguration, Transport};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::DropLog;
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
    /// Returns the task's handle, or `None` when MQTT is disabled. The task
    /// stops when the service shuts down.
    pub async fn start(&self) -> Option<JoinHandle<()>> {
        let config = self.state.config.read().await;
        let mqtt_config = config.mqtt.clone();
        drop(config);

        if !mqtt_config.enabled {
            info!("MQTT publisher is disabled");
            return None;
        }

        info!("Starting MQTT publisher to {}", mqtt_config.broker);

        let state = Arc::clone(&self.state);
        let shutdown_rx = self.state.subscribe_shutdown();

        Some(tokio::spawn(run_mqtt_publisher(
            state,
            mqtt_config,
            shutdown_rx,
        )))
    }
}

/// Run the MQTT publisher loop.
async fn run_mqtt_publisher(
    state: Arc<AppState>,
    config: MqttConfig,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
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
        mqtt_options.set_transport(Transport::tls_with_config(TlsConfiguration::Native));
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
    let mut reload_rx = state.collector.subscribe_reload();

    info!(
        "MQTT publisher connected to {} with prefix '{}'",
        config.broker, config.topic_prefix
    );

    // Spawn event loop handler (keep handle so we can abort on shutdown)
    let eventloop_handle = tokio::spawn(async move {
        let mut consecutive_errors: u32 = 0;
        let max_backoff = Duration::from_secs(300); // Cap at 5 minutes
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(ack))) => {
                    if consecutive_errors > 0 {
                        info!(
                            "MQTT reconnected after {} errors: {:?}",
                            consecutive_errors, ack
                        );
                    } else {
                        info!("MQTT connected: {:?}", ack);
                    }
                    consecutive_errors = 0;
                }
                Ok(Event::Incoming(Packet::PingResp)) => {
                    debug!("MQTT ping response received");
                }
                Ok(Event::Outgoing(_)) => {
                    // Outgoing events are normal, no need to log
                }
                Ok(_) => {}
                Err(e) => {
                    consecutive_errors = consecutive_errors.saturating_add(1);
                    let backoff = Duration::from_secs(5)
                        .saturating_mul(2u32.saturating_pow(consecutive_errors.min(6)))
                        .min(max_backoff);
                    if consecutive_errors <= 3 {
                        warn!(
                            "MQTT connection error: {}. Reconnecting in {:?}...",
                            e, backoff
                        );
                    } else if consecutive_errors.is_multiple_of(50) {
                        error!(
                            "MQTT connection error ({} consecutive): {}. Reconnecting in {:?}...",
                            consecutive_errors, e, backoff
                        );
                    }
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    });

    // Publish Home Assistant discovery messages if enabled. They wait in the
    // client's queue until the event loop has connected.
    if config.homeassistant {
        let devices = configured_devices(&state).await;
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                info!("MQTT publisher received stop signal");
                stop_publisher(&client, &eventloop_handle);
                return;
            }
            _ = publish_ha_discovery_bounded(&client, &config, &devices, qos) => {}
        }
    }

    // Readings dropped because the client's queue was full.
    let mut drops = DropLog::default();

    // Main publishing loop
    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                info!("MQTT publisher received stop signal");
                break;
            }
            result = readings_rx.recv() => {
                match result {
                    Ok(event) => {
                        let device_name = configured_device_name(&state, &event.device_id)
                            .await
                            .unwrap_or_else(|| event.device_id.clone());
                        let outcome = publish_reading(&client, &config, &device_name, &event, qos);
                        if outcome == PublishOutcome::Dropped && drops.record() {
                            warn!(
                                "MQTT broker unreachable or slow; dropped {} readings",
                                drops.dropped
                            );
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
            result = reload_rx.changed() => {
                if result.is_ok() && config.homeassistant {
                    let devices = configured_devices(&state).await;
                    tokio::select! {
                        biased;
                        _ = shutdown_rx.changed() => {
                            info!("MQTT publisher received stop signal");
                            break;
                        }
                        _ = publish_ha_discovery_bounded(&client, &config, &devices, qos) => {}
                    }
                }
            }
        }
    }

    stop_publisher(&client, &eventloop_handle);
}

/// Disconnect from the broker and stop the event loop task.
fn stop_publisher(client: &AsyncClient, eventloop: &JoinHandle<()>) {
    // Don't wait: while the broker is unreachable the queue is full, and
    // `disconnect().await` would wait for room forever.
    if let Err(e) = client.try_disconnect() {
        debug!("Error disconnecting MQTT client: {}", e);
    }
    eventloop.abort();

    info!("MQTT publisher stopped");
}

/// Whether a reading went to the MQTT client's queue.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PublishOutcome {
    /// Every topic of the reading was queued.
    Published,
    /// The queue was full (the broker is unreachable or slow) or the event
    /// loop had stopped, so the reading's remaining topics were skipped.
    Dropped,
}

/// Queue a reading's MQTT messages without waiting for room in the client's
/// queue.
///
/// `device_name` is the device's alias or address; it becomes the topic's
/// device segment. At the first message that doesn't fit, the reading's
/// remaining topics are skipped and [`PublishOutcome::Dropped`] is returned,
/// so a stalled broker never stalls the publisher, and the backlog it gets
/// when it is back is at most one queue, not the whole outage.
fn publish_reading(
    client: &AsyncClient,
    config: &MqttConfig,
    device_name: &str,
    event: &ReadingEvent,
    qos: QoS,
) -> PublishOutcome {
    let device_name = sanitize_topic_segment(device_name);
    let reading = &event.reading;

    // Publish individual metrics, filtered by device capabilities.
    // When device type can be determined from the name, use capability checks.
    // When it cannot (e.g. MAC address), fall back to data-driven detection
    // to avoid suppressing valid readings.
    let device_type = aranet_types::DeviceType::from_name(&event.device_id);
    let has_co2 = device_type.map_or(reading.co2 > 0, |dt| dt.has_co2());
    let has_temp = device_type.map_or(reading.temperature != 0.0 || reading.humidity > 0, |dt| {
        dt.has_temperature()
    });
    let has_pressure = device_type.map_or(reading.pressure > 0.0, |dt| dt.has_pressure());

    let status = match reading.status {
        aranet_types::Status::Green => "green",
        aranet_types::Status::Yellow => "yellow",
        aranet_types::Status::Red => "red",
        aranet_types::Status::Error => "error",
        _ => "unknown",
    };

    // (metric, payload) in publishing order: the full JSON reading first.
    let mut messages = vec![("json", serde_json::to_string(reading).unwrap_or_default())];
    if has_co2 {
        messages.push(("co2", reading.co2.to_string()));
    }
    if has_temp {
        messages.push(("temperature", format!("{:.2}", reading.temperature)));
        messages.push(("humidity", reading.humidity.to_string()));
    }
    if has_pressure {
        messages.push(("pressure", format!("{:.2}", reading.pressure)));
    }
    // Battery (all devices have this)
    messages.push(("battery", reading.battery.to_string()));
    messages.push(("status", status.to_string()));
    if let Some(radon) = reading.radon {
        messages.push(("radon", radon.to_string()));
    }
    if let Some(avg) = reading.radon_avg_24h {
        messages.push(("radon_avg_24h", avg.to_string()));
    }
    if let Some(avg) = reading.radon_avg_7d {
        messages.push(("radon_avg_7d", avg.to_string()));
    }
    if let Some(avg) = reading.radon_avg_30d {
        messages.push(("radon_avg_30d", avg.to_string()));
    }
    if let Some(rate) = reading.radiation_rate {
        messages.push(("radiation_rate", format!("{:.4}", rate)));
    }
    if let Some(total) = reading.radiation_total {
        messages.push(("radiation_total", format!("{:.6}", total)));
    }

    for (metric, payload) in messages {
        let topic = format!("{}/{}/{}", config.topic_prefix, device_name, metric);
        if let Err(e) = client.try_publish(topic, qos, config.retain, payload) {
            debug!(
                "MQTT queue refused {} for {}: {}; skipping the rest of the reading",
                metric, event.device_id, e
            );
            return PublishOutcome::Dropped;
        }
    }

    debug!(
        "Published reading for {} to MQTT (CO2={})",
        event.device_id, reading.co2
    );

    PublishOutcome::Published
}

async fn configured_devices(state: &AppState) -> Vec<crate::config::DeviceConfig> {
    let config = state.config.read().await;
    config.devices.clone()
}

async fn configured_device_name(state: &AppState, device_id: &str) -> Option<String> {
    let config = state.config.read().await;
    config
        .devices
        .iter()
        .find(|device| device.address == device_id)
        .map(|device| {
            device
                .alias
                .clone()
                .unwrap_or_else(|| device.address.clone())
        })
}

/// Longest time Home Assistant discovery waits for room in the MQTT client's
/// queue.
pub(crate) const HA_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// [`publish_ha_discovery`], given at most [`HA_DISCOVERY_TIMEOUT`]. Returns
/// whether every discovery message was queued; otherwise warns.
///
/// Discovery messages wait for room in the queue, unlike readings, but a
/// broker that stays unreachable must not stall the publisher. Discovery is
/// published again after the next device change.
async fn publish_ha_discovery_bounded(
    client: &AsyncClient,
    config: &MqttConfig,
    devices: &[crate::config::DeviceConfig],
    qos: QoS,
) -> bool {
    match tokio::time::timeout(
        HA_DISCOVERY_TIMEOUT,
        publish_ha_discovery(client, config, devices, qos),
    )
    .await
    {
        Ok(Ok(())) => true,
        Ok(Err(e)) => {
            warn!("Failed to publish HA discovery: {}", e);
            false
        }
        Err(_) => {
            warn!(
                "HA discovery did not fit in the MQTT queue within {}s (broker unreachable?); \
                 it is published again after the next device change",
                HA_DISCOVERY_TIMEOUT.as_secs()
            );
            false
        }
    }
}

/// Publish Home Assistant MQTT auto-discovery messages for all configured devices.
async fn publish_ha_discovery(
    client: &AsyncClient,
    config: &MqttConfig,
    devices: &[crate::config::DeviceConfig],
    qos: QoS,
) -> Result<(), rumqttc::ClientError> {
    let prefix = &config.ha_discovery_prefix;
    let topic_prefix = &config.topic_prefix;

    for device in devices {
        let device_name =
            sanitize_topic_segment(device.alias.as_deref().unwrap_or(&device.address));
        let display_name = device.alias.as_deref().unwrap_or(&device.address);

        // Determine device type from name/address
        let device_type = aranet_types::DeviceType::from_name(&device.address).or_else(|| {
            device
                .alias
                .as_deref()
                .and_then(aranet_types::DeviceType::from_name)
        });

        let has_co2 = device_type.is_none_or(|dt| dt.has_co2());
        let has_temp = device_type.is_none_or(|dt| dt.has_temperature());
        let has_pressure = device_type.is_none_or(|dt| dt.has_pressure());

        let device_json = serde_json::json!({
            "identifiers": [format!("aranet_{}", device_name)],
            "name": display_name,
            "manufacturer": "SAF Tehnika",
            "model": device_type.map(|dt| dt.to_string()).unwrap_or_else(|| "Aranet".to_string()),
        });

        // Helper to build a single sensor discovery message
        let publish_sensor = |metric: &str,
                              name_suffix: &str,
                              unit: &str,
                              device_class: Option<&str>,
                              state_class: Option<&str>| {
            let unique_id = format!("aranet_{}_{}", device_name, metric);
            let sensor_name = format!("{} {}", display_name, name_suffix);
            let state_topic = format!("{}/{}/{}", topic_prefix, device_name, metric);
            let config_topic = format!("{}/sensor/{}_{}/config", prefix, device_name, metric);

            let mut payload = serde_json::json!({
                "name": sensor_name,
                "unique_id": unique_id,
                "state_topic": state_topic,
                "unit_of_measurement": unit,
                "device": device_json,
            });

            if let Some(dc) = device_class {
                payload["device_class"] = serde_json::json!(dc);
            }
            if let Some(sc) = state_class {
                payload["state_class"] = serde_json::json!(sc);
            }

            (config_topic, payload.to_string())
        };

        // Define sensors to register
        let mut sensors = vec![];

        if has_co2 {
            sensors.push(publish_sensor(
                "co2",
                "CO\u{2082}",
                "ppm",
                Some("carbon_dioxide"),
                Some("measurement"),
            ));
        }

        if has_temp {
            sensors.push(publish_sensor(
                "temperature",
                "Temperature",
                "\u{00b0}C",
                Some("temperature"),
                Some("measurement"),
            ));
            sensors.push(publish_sensor(
                "humidity",
                "Humidity",
                "%",
                Some("humidity"),
                Some("measurement"),
            ));
        }

        if has_pressure {
            sensors.push(publish_sensor(
                "pressure",
                "Pressure",
                "hPa",
                Some("atmospheric_pressure"),
                Some("measurement"),
            ));
        }

        sensors.push(publish_sensor(
            "battery",
            "Battery",
            "%",
            Some("battery"),
            Some("measurement"),
        ));

        // Publish all discovery messages
        for (config_topic, payload) in sensors {
            client
                .publish(&config_topic, qos, true, payload.as_bytes())
                .await?;
        }

        info!("Published HA discovery for device: {}", display_name);
    }

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
/// MQTT also forbids U+0000 and advises against other control characters, so
/// every control character becomes '_' too.
fn sanitize_topic_segment(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '#' | '+' | ' ' | '/' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect()
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

    #[test]
    fn sanitize_topic_segment_replaces_control_characters() {
        // MQTT forbids U+0000 in topic names and advises against other controls.
        assert_eq!(sanitize_topic_segment("Office\u{0}"), "Office_");
        assert_eq!(sanitize_topic_segment("a\nb\r\tc\u{7f}"), "a_b__c_");
        assert_eq!(sanitize_topic_segment("a\u{85}b"), "a_b");
        // Real names keep everything but the characters MQTT needs replaced.
        assert_eq!(
            sanitize_topic_segment("Aranet\u{2622} 30ED1"),
            "Aranet\u{2622}_30ED1"
        );
        assert_eq!(sanitize_topic_segment("Küche"), "Küche");
    }

    /// A reading from an Aranet4, which publishes 7 topics.
    fn aranet4_event() -> ReadingEvent {
        ReadingEvent {
            device_id: "Aranet4 12345".to_string(),
            reading: aranet_store::StoredReading {
                id: 1,
                device_id: "Aranet4 12345".to_string(),
                co2: 800,
                temperature: 22.5,
                humidity: 45,
                pressure: 1013.0,
                battery: 85,
                status: aranet_types::Status::Green,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
                radon_avg_24h: None,
                radon_avg_7d: None,
                radon_avg_30d: None,
                captured_at: time::OffsetDateTime::UNIX_EPOCH,
            },
        }
    }

    /// A reading from an AranetRn+ with every radon value, which publishes
    /// 10 topics.
    fn radon_event() -> ReadingEvent {
        let device_id = "AranetRn+ 12345".to_string();
        ReadingEvent {
            device_id: device_id.clone(),
            reading: aranet_store::StoredReading {
                device_id,
                co2: 0,
                temperature: 21.3,
                humidity: 52,
                pressure: 1009.5,
                battery: 97,
                status: aranet_types::Status::Yellow,
                radon: Some(87),
                radon_avg_24h: Some(91),
                radon_avg_7d: Some(104),
                radon_avg_30d: Some(118),
                ..aranet4_event().reading
            },
        }
    }

    /// A reading from an Aranet Radiation, which publishes 5 topics.
    fn radiation_event() -> ReadingEvent {
        let device_id = "Aranet\u{2622} 30ED1".to_string();
        ReadingEvent {
            device_id: device_id.clone(),
            reading: aranet_store::StoredReading {
                device_id,
                co2: 0,
                temperature: 0.0,
                humidity: 0,
                pressure: 0.0,
                battery: 64,
                radiation_rate: Some(0.12),
                radiation_total: Some(0.0425),
                ..aranet4_event().reading
            },
        }
    }

    /// A client whose queue holds `capacity` requests. Its event loop is
    /// returned but never polled, so nothing connects and the queue never
    /// drains, as while the broker is unreachable.
    fn client_with_queue(capacity: usize) -> (AsyncClient, rumqttc::EventLoop) {
        AsyncClient::new(MqttOptions::new("aranet-test", "127.0.0.1", 1883), capacity)
    }

    /// The messages waiting in the client's queue, oldest first, as (topic
    /// below `topic_base`, payload). Panics unless every one was published
    /// below `topic_base`, with `qos` and `retain`.
    fn queued_messages(
        eventloop: &mut rumqttc::EventLoop,
        topic_base: &str,
        qos: QoS,
        retain: bool,
    ) -> Vec<(String, String)> {
        // Nothing polls the event loop, so everything published is still in
        // the client's queue. `clean` moves it to `pending`.
        eventloop.clean();
        eventloop
            .pending
            .drain(..)
            .map(|request| {
                let publish = match request {
                    rumqttc::Request::Publish(publish) => publish,
                    other => panic!("queued something other than a publish: {other:?}"),
                };
                assert_eq!((publish.qos, publish.retain), (qos, retain), "{publish:?}");
                let metric = publish
                    .topic
                    .strip_prefix(topic_base)
                    .unwrap_or_else(|| panic!("{} is not below {topic_base}", publish.topic));
                let payload = String::from_utf8(publish.payload.to_vec()).expect("a text payload");
                (metric.to_string(), payload)
            })
            .collect()
    }

    /// (metric, payload) pairs in the form `queued_messages` returns.
    fn messages(expected: &[(&str, &str)]) -> Vec<(String, String)> {
        expected
            .iter()
            .map(|(metric, payload)| (metric.to_string(), payload.to_string()))
            .collect()
    }

    /// `count` Aranet4s, each with 5 discovery messages.
    fn aranet4_devices(count: usize) -> Vec<crate::config::DeviceConfig> {
        (0..count)
            .map(|i| crate::config::DeviceConfig {
                address: format!("Aranet4 {i:05}"),
                alias: None,
                poll_interval: 60,
            })
            .collect()
    }

    /// Five Aranet4s: 25 discovery messages.
    fn five_devices() -> Vec<crate::config::DeviceConfig> {
        aranet4_devices(5)
    }

    #[test]
    fn mqtt_publish_reading_does_not_block_when_queue_full() {
        let (client, _eventloop) = client_with_queue(1);

        let outcome = publish_reading(
            &client,
            &MqttConfig::default(),
            "Office",
            &aranet4_event(),
            QoS::AtMostOnce,
        );

        assert_eq!(outcome, PublishOutcome::Dropped);
    }

    #[test]
    fn mqtt_publish_reading_queues_every_topic_when_there_is_room() {
        // Exactly the reading's 7 topics fit.
        let (client, _eventloop) = client_with_queue(7);

        let outcome = publish_reading(
            &client,
            &MqttConfig::default(),
            "Office",
            &aranet4_event(),
            QoS::AtMostOnce,
        );

        assert_eq!(outcome, PublishOutcome::Published);
    }

    #[test]
    fn mqtt_publish_reading_queues_these_messages_for_a_co2_sensor() {
        let (client, mut eventloop) = client_with_queue(100);
        let event = aranet4_event();

        let outcome = publish_reading(
            &client,
            &MqttConfig::default(),
            "Office",
            &event,
            QoS::AtMostOnce,
        );

        assert_eq!(outcome, PublishOutcome::Published);
        let queued = queued_messages(&mut eventloop, "aranet/Office/", QoS::AtMostOnce, false);
        // The first message is the whole reading as JSON.
        let json = serde_json::to_string(&event.reading).unwrap();
        assert_eq!(
            queued,
            messages(&[
                ("json", &json),
                ("co2", "800"),
                ("temperature", "22.50"),
                ("humidity", "45"),
                ("pressure", "1013.00"),
                ("battery", "85"),
                ("status", "green"),
            ])
        );
        let reading: serde_json::Value = serde_json::from_str(&queued[0].1).unwrap();
        assert_eq!(reading["co2"], 800, "{reading}");
    }

    #[test]
    fn mqtt_publish_reading_queues_these_messages_for_a_radon_sensor() {
        let (client, mut eventloop) = client_with_queue(100);
        let event = radon_event();
        // A prefix, retain flag and QoS other than the defaults, and a device
        // without an alias: its address, with the `+` and the space replaced,
        // is the topic's device segment.
        let config = MqttConfig {
            topic_prefix: "home/sensors".to_string(),
            retain: true,
            ..MqttConfig::default()
        };

        let outcome = publish_reading(&client, &config, &event.device_id, &event, QoS::AtLeastOnce);

        assert_eq!(outcome, PublishOutcome::Published);
        let queued = queued_messages(
            &mut eventloop,
            "home/sensors/AranetRn__12345/",
            QoS::AtLeastOnce,
            true,
        );
        let json = serde_json::to_string(&event.reading).unwrap();
        assert_eq!(
            queued,
            messages(&[
                ("json", &json),
                ("temperature", "21.30"),
                ("humidity", "52"),
                ("pressure", "1009.50"),
                ("battery", "97"),
                ("status", "yellow"),
                ("radon", "87"),
                ("radon_avg_24h", "91"),
                ("radon_avg_7d", "104"),
                ("radon_avg_30d", "118"),
            ])
        );
    }

    #[test]
    fn mqtt_publish_reading_queues_these_messages_for_a_radiation_sensor() {
        let (client, mut eventloop) = client_with_queue(100);
        let event = radiation_event();

        let outcome = publish_reading(
            &client,
            &MqttConfig::default(),
            "Lab",
            &event,
            QoS::AtMostOnce,
        );

        assert_eq!(outcome, PublishOutcome::Published);
        let queued = queued_messages(&mut eventloop, "aranet/Lab/", QoS::AtMostOnce, false);
        let json = serde_json::to_string(&event.reading).unwrap();
        assert_eq!(
            queued,
            messages(&[
                ("json", &json),
                ("battery", "64"),
                ("status", "green"),
                ("radiation_rate", "0.1200"),
                ("radiation_total", "0.042500"),
            ])
        );
    }

    #[tokio::test(start_paused = true)]
    async fn mqtt_ha_discovery_is_bounded() {
        let (client, _eventloop) = client_with_queue(1);
        let config = MqttConfig {
            homeassistant: true,
            ..MqttConfig::default()
        };

        let started = tokio::time::Instant::now();
        let published =
            publish_ha_discovery_bounded(&client, &config, &five_devices(), QoS::AtMostOnce).await;

        let waited = started.elapsed();
        assert!(!published);
        assert!(
            waited <= HA_DISCOVERY_TIMEOUT + Duration::from_secs(1),
            "waited {waited:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn mqtt_ha_discovery_is_queued_at_once_when_there_is_room() {
        let (client, _eventloop) = client_with_queue(100);
        let config = MqttConfig {
            homeassistant: true,
            ..MqttConfig::default()
        };

        let started = tokio::time::Instant::now();
        let published =
            publish_ha_discovery_bounded(&client, &config, &five_devices(), QoS::AtMostOnce).await;

        assert!(published);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn mqtt_publisher_stops_promptly_while_broker_unreachable() {
        let (logs, _guard) = crate::test_support::CapturedLogs::start();
        // Nothing listens there, so the event loop never connects and the
        // client's queue of 100 never drains.
        let broker = crate::test_support::closed_port_url("")
            .await
            .replacen("http://", "mqtt://", 1);
        let t = crate::test_support::TestState::with_config(crate::config::Config {
            mqtt: MqttConfig {
                enabled: true,
                broker,
                homeassistant: true,
                ..MqttConfig::default()
            },
            // 125 discovery messages, more than the queue holds.
            devices: aranet4_devices(25),
            ..crate::config::Config::default()
        });

        let publisher = MqttPublisher::new(Arc::clone(&t.state))
            .start()
            .await
            .expect("MQTT is enabled");
        // The first 20 devices' messages fill the queue, and discovery waits
        // for room for the 21st's.
        crate::test_support::wait_until("HA discovery to fill the MQTT queue", || {
            logs.text()
                .contains("Published HA discovery for device: Aranet4 00019")
        })
        .await;

        // Shutdown ends discovery, and the publisher disconnects without
        // waiting for room in the full queue.
        t.state.signal_shutdown();
        tokio::time::timeout(Duration::from_secs(5), publisher)
            .await
            .expect("the publisher stops within 5 s of shutdown")
            .expect("the publisher does not panic");
    }
}
