# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **`DeviceNotFoundReason::Ambiguous` and `DeviceNotFoundReason::NoExactMatch`**, returned for a name that two nearby devices share and for a partial name (see Changed)
- **`Device::connect_with_scan_options`**, to choose how long a connect searches for the device
- **`aranet_core::peripheral_name`**, the name aranet uses for a device, read from btleplug's `PeripheralProperties`: the name the device advertises, or on macOS, for a device that hasn't advertised one, the name macOS has stored for it. A program that reads `local_name` itself should call it instead (see Changed)
- **Optional data retention** — `[storage.retention] readings_days` and `history_days` make the service delete older rows at startup and then hourly, in small batches. Off by default. `history_days` also prunes the history cache that `aranet sync` fills, which the next sync may download again. Both limits apply to everything in the database, including readings that the `aranet` CLI, TUI and GUI stored there when they share it (the default, except under the systemd system service and Docker)
- **Setting `security.api_key_enabled = true` without `security.api_key`** now generates a key and saves it to `server.toml`
- **The API key is also accepted as `Authorization: Bearer <key>`**, so Prometheus can scrape an authenticated service with `authorization: { type: Bearer, credentials: … }`. A repeated `X-API-Key` or `Authorization` header, or an `X-API-Key` that isn't plain text, is never read as a key, so a request that sends its key that way is refused
- **New `aranet-store` API** — `Store::open_read_only`; `Store::prune_readings_batch`, `Store::prune_history_batch` and `PRUNE_BATCH_ROWS`, which delete old rows in short transactions; `TomlFile`, `TomlFileError` and `write_private`, the atomic, owner-only config-file updater that keeps comments and unknown keys; `default_data_dir`; and `MAX_QUERY_LIMIT`, now re-exported from the crate root
- **New `aranet-cli` library API** — `Config::update` and `Config::update_at`, which change only the keys the closure changes; `Config::apply_state`; and `state::CliState`, the `cli-state.toml` file that now holds the last connected device
- **New `aranet-service` API** — `load_config`, `check_config` and `LoadedConfig`; `ServerConfig::socket_addr`; `config::generate_api_key`; `middleware::OriginPolicy` and `middleware::origin_guard`; `RetentionConfig` and `config::MAX_RETENTION_DAYS`; `Config::update_file` and `Config::save_devices`; `config::MAX_DEVICE_TEXT_CHARS`; and in `api`, `DEFAULT_QUERY_LIMIT`, `MAX_QUERY_OFFSET`, `ReadingsQuery::page` and `PaginatedResponse::from_rows`, with `MAX_QUERY_LIMIT` now public

### Changed

- **Background Bluetooth work** — aranet-core now creates the Bluetooth manager and adapter, runs scans and finishes disconnects on its own background thread (`aranet-ble`) on every platform, so it works from any number of tokio runtimes. Linux and Windows processes have one more thread, as macOS already had
- **One scan at a time per process** — concurrent device searches (`aranet read -d A -d B`, the GUI's Refresh All, `DeviceManager::connect_all`) wait for the running scan and reuse what it found, instead of failing with `InProgress` on Linux or cutting each other's scans short on macOS. A cancelled `PassiveMonitor` stops at once instead of after its current wait. `DeviceManager::supports_passive_monitoring` waits for a running scan too, and gives up after 15 s instead of 6 s
- **Device identifiers must match exactly** — `-d`/`--device`, `ARANET_DEVICE`, the default device set with `aranet config set device`, aliases, the service's `[[devices]] address` and `Device::connect` accept a MAC address (with or without colons), a macOS device UUID, the device ID that aranet's TUI and GUI store (`hci0/dev_…` on Linux) or the full device name, in any case. A partial name such as `Aranet4` or `2751B` no longer connects to whichever matching device the scan listed first: it fails with "did you mean 'Aranet2 2751B'?" as soon as a scan shows names that contain it, and a name that two nearby devices share fails as ambiguous. An empty identifier is rejected without using Bluetooth, and the CLI prints these errors without its Bluetooth troubleshooting tips; `aranet watch`, which retries after other connection failures, exits on them like the other commands. `aranet read --passive` and `aranet watch --passive` still match the name, address or identifier exactly as `aranet scan` prints them, case included
- **Disconnect and connection checks** — `Device::disconnect` returns `Error::Timeout` after 5 s if the Bluetooth stack never confirms, and finishes in the background if its caller is dropped; `Device::is_connected` returns `false` if the stack doesn't answer within `ConnectionConfig::validation_timeout` (3 s by default); a `Device` dropped outside any tokio runtime now disconnects too, on aranet-core's own runtime, instead of skipping the disconnect
- **Resubscribing** to the same characteristic replaces its callback instead of running both
- **Connects give up on an absent sensor after about 30 s** instead of about 60 s — `Device::connect`, `connect_with_config`, `connect_with_timeout` and `connect_with_adapter` search once, with scans of 5, 10 and 15 s, instead of searching again for as long as the connect timeout. `connect_with_timeout`'s argument is the connect timeout, as it always was; its documentation had called it a scan timeout
- **Linux pairing** — a connection to a sensor that BlueZ doesn't list as paired pairs it first and logs `Paired with <address>`; if pairing fails or times out, aranet logs a warning with the `bluetoothctl` commands that fix it and carries on connecting (`aranet` shows the warning by default and `Paired with` with `--verbose`; `aranet-service` and `aranet server` show neither unless `RUST_LOG` enables them, for example with `RUST_LOG=aranet_core=info`). A sensor that refuses to pair isn't asked again for 10 minutes; after any other failure, the next connection pairs again. aranet can't enter a PIN: a sensor that asks for one either refuses aranet's pairing or, like an Aranet2 with BlueZ 5.82, accepts it without the PIN and then rejects that pairing at every later connection. Pair such a sensor once by hand: stop aranet, remove aranet's pairing if it made one (`bluetoothctl remove <address>`), then pair it in an interactive `bluetoothctl` session, with `scan on` before `pair`, entering its PIN; aranet then uses that pairing
- **`ReconnectingDevice` retries** — it reconnects only after connection errors (not connected, timeouts, connection failures and Bluetooth link errors); other errors, such as a characteristic the device doesn't have, are returned at once. Operations that fail together share one reconnect; `cancel_reconnect()` and `disconnect()` stop a reconnect wait at once, and a reconnect already in progress no longer undoes `disconnect()`. A `with_device` closure can run a second time, but only after a connection error. `reconnect()`, and so `AranetDevice::connect()`, called while another reconnect is running waits for it instead of starting its own, and returns `Error::NotConnected` if that one gave up or `disconnect()` stopped it, or `Error::Cancelled` after `cancel_reconnect()`; operations waiting for a reconnect do the same. The old connection is closed when a reconnect starts, so while it runs, and after it fails, `ReconnectingDevice::name()` returns `None` and `address()` returns the identifier
- **`DeviceManager`** — devices that were disconnected, evicted or removed stay that way until `connect()` is called; automatic reconnects back off per device using its `ReconnectOptions` and stop after `max_attempts` with one `DeviceEvent::Error`; `ManagerConfig::default().default_reconnect_options` is now `ReconnectOptions::unlimited()`; health checks run concurrently, a failed reconnect shortens the adaptive check interval even when other devices are healthy, and the monitor stops as soon as it is cancelled; a second `connect()` of a device waits for the first and returns its real result; a `connect()` that hasn't connected yet when `disconnect()`, `disconnect_all()`, `evict_lowest_priority()` or `remove_device()` is called for the same device returns `Error::Cancelled` and releases the sensor, instead of `Ok` or adding the device back; these four send `Disconnected`, and `remove_device()` removes the device, even when closing the connection fails or the call is cancelled, and still return a failed close's error; `max_concurrent_connections` counts connects in progress, so `connect_all` respects it; `connect()` on a device whose connection has dropped reconnects instead of returning `Ok`; `add_device`, `add_device_with_options` and `add_device_with_priority` reject invalid `ReconnectOptions`; passive readings from the hybrid monitor no longer reset a device's reconnect backoff
- **`ConnectionConfig::discovery_timeout` on Linux** — while BlueZ is still discovering the services of a device it has just connected, a connect waits at least 20 s for it, even when `discovery_timeout` is shorter (`ConnectionConfig::fast()` sets 5 s)
- **`ConnectionConfig::discovery_timeout` on macOS** — by btleplug's source code, version 0.13 reports a macOS connection as made as soon as the link is up and discovers the services in a step of its own, so `discovery_timeout` (10 s by default, 8 s in the TUI) now limits that discovery, where the link and the discovery shared the connect timeout before; a connect has more time in all (25 s where it had 15 s by default, 40 s where it had 30 s in `aranet watch`, 18 s where it had 10 s in the TUI), and a discovery that alone takes longer than `discovery_timeout` fails with "Operation 'discover services' timed out". No `aranet` option or config key sets `discovery_timeout`: the `--timeout` option and the `timeout` key of `aranet watch` now limit only the time the link takes to come up
- **Bluetooth errors on macOS** — by btleplug's source code, an error that macOS reports for a read, a write or a subscribe is now returned as a "Bluetooth error". In 0.2.1 a read or a write ran into its time limit and failed as a timeout. A read that needs pairing can still time out while macOS shows its pairing window
- **Two new failures on macOS** — by btleplug's source code, once macOS reports that a sensor's services changed, reads and writes on that connection fail with "Service no longer available" until the next connect, and one that is in progress at that moment fails with "Service invalidated; rediscovery required"; and when the characteristics of a service can't be discovered, the connect still succeeds and reads then fail with "Characteristic not found", which `ReconnectingDevice` returns without reconnecting (`aranet watch` and the service connect again at their next poll)
- **Device names on macOS** — a sensor whose name stored by macOS differs from the name it advertises was listed as `<stored name> [<advertised name>]`, for example `Kitchen [Aranet4 1A2B3]`. btleplug 0.13 no longer builds that form: the sensor is listed under its advertised name alone, `Aranet4 1A2B3`, and `-d Kitchen` or `-d "Kitchen [Aranet4 1A2B3]"` no longer finds it. Use the advertised name or the identifier that `aranet scan` prints. The rule for names on Linux and Windows is unchanged
- **Linux: Bluetooth adapters** — by btleplug's source code and that of its BlueZ library, bluez-async 0.9, adapters that BlueZ reports with a `bluetooth:` modalias are now listed, where only those with a `usb:` modalias were before. BlueZ (5.82, by its source code) gives every adapter of a host the same modalias, built from `DeviceID` in `/etc/bluetooth/main.conf` (`usb:…` unless that is changed), so this matters only on a host that sets `DeviceID = bluetooth:…`: there 0.2.1 found no adapter and failed with "no Bluetooth adapter available", and its adapters are now found. On every other host the adapters listed are the same as before
- **Breaking (library):** `aranet-core` now builds on btleplug 0.13 (0.13.5 or a later 0.13 release; 0.2.1 used 0.11) and so, on Linux, on bluez-async 0.9 (was 0.8). btleplug is a public dependency of `aranet-core` through `Error::Bluetooth`, `DiscoveredDevice::id`, `scan::get_adapter`, `scan::scan_with_adapter`, the `scan::find_device…` functions, `Device::connect_with_adapter`, `Device::from_peripheral…`, `format_peripheral_id`, `create_identifier` and the new `peripheral_name`, so a program that uses these with its own btleplug dependency must move to btleplug 0.13 too, and bumping it is a breaking change. `btleplug::Error`, which `Error::Bluetooth` carries, has a new variant, `NoAdapterAvailable`. Code that reads `PeripheralProperties::local_name` itself gets a different value on every platform: BlueZ's alias on Linux, the system's name for the device after a connect on Windows, and on macOS no longer the combined form (see "Device names on macOS"); `aranet_core::peripheral_name` returns the name aranet uses. On macOS, btleplug 0.13.5 logs `Shouldn't get anything but Ok!` at error level after every disconnect that macOS confirms, under the log target `btleplug::corebluetooth::peripheral`; the disconnect has succeeded, and the line can be ignored. `aranet`, `aranet-service` and the GUI leave that target out of their default log output: `aranet --verbose`, and a `RUST_LOG` of your own in `aranet` and the GUI, can still show the line, while `aranet-service` and `aranet server` keep the target off whatever `RUST_LOG` says. A program with its own logger must filter it out itself
- **Breaking (library):** `aranet-service`'s Rust API changed for 0.3.0:
  - **`Collector::stop`** returns `CollectorStopResult`, and **`CollectorError`** is `#[non_exhaustive]` with a new `Timeout(Duration)` variant
  - **`PrometheusPusher::start`** returns `Option<JoinHandle<()>>`
  - **`AppState.store`** is private, and the closures passed to `with_store_read`/`with_store_write` must be `Send + 'static`
  - **`PaginationMeta.limit`** is a `u32`, and **`StorageConfig`** has a new `retention` field
  - **`WebhookDispatcher::start`, `InfluxDbWriter::start` and `MqttPublisher::start`** return `Option<JoinHandle<()>>`
  - **`run()`** fails on a config file that exists but can't be read
  - **`SecurityConfig`** has a new `allowed_hosts` field, and **`app()`** adds the CORS and Host/Origin layers
  - **`SecurityConfig`** has a new `allow_unauthenticated_network` field, and **`MdnsAdvertiser::start`** takes the bound `SocketAddr`
  - **`RateLimitState::new`** takes `&SecurityConfig`, `RateLimitState` is generic over a clock (with a default), and `cleanup()` is synchronous and takes no arguments; `check`, `with_clock`, `len`, `is_empty` and `RateLimitKey` are new; `governor` 0.10 is now a public dependency of `aranet-service`, so bumping it is a breaking change
  - **`Config::save`** writes only the keys that differ from what the file holds, and creates no file when nothing differs from the defaults
  - **`AppState::with_store_read`** runs its closure on a read-only connection when the database is a file, so a closure that writes fails
  - **`RateLimitState::default()`** carries `SecurityConfig::default()`'s limits, and `middleware::rate_limit` takes its limits from the state, not from the config it is given
- **`aranet-store` library** — `toml` 0.9 is now a public dependency of `aranet-store` through `TomlFileError`, so bumping it is a breaking change; `Store::database_path()` is `None` for a store opened on `:memory:` or another in-memory name; `Store::prune_readings` and `Store::prune_history` commit every 5000 rows, so a prune that fails part-way keeps the batches it finished; `ReadingQuery` and `HistoryQuery` return rows with the same timestamp in the order of their ids
- **`/api/status` and `/metrics` poll counts** now count since the service started. Adding, removing or renaming a sensor no longer resets them, re-polls the other sensors or resets the collector uptime; changing a sensor's poll interval restarts only that sensor
- **Data endpoints** (`/api/readings`, `/api/devices/{id}/readings`, `/api/devices/{id}/history`) return the newest 1000 rows when `limit` is omitted (maximum 10000) and set `has_more` when there are more; `pagination.limit` is always a number. `limit=0` and `offset` above 1000000 are rejected with 400 (use `since`/`until` to go further back), and rows with the same timestamp come back in a fixed order
- **Connecting to a sensor no longer rewrites `config.toml`**: the last connected device is kept in `cli-state.toml` in the data directory. A `last_device` already in `config.toml` is used until the next connection. `aranet config show` no longer prints `last_device` and hides `gui.service_api_key`, and `aranet config init` writes a commented template
- **The default `security.cors_origins`** is now empty; values already saved in `server.toml` stay in effect (the service warns at startup about the old default while authentication is off)
- **Breaking:** The service refuses to listen on a non-loopback address without API-key authentication. Enable `security.api_key_enabled`, bind to `127.0.0.1`, or set `security.allow_unauthenticated_network = true` if a firewall or authenticating proxy protects the port. mDNS advertises the service only when authentication is on
- **Rate limiting** allows a burst of `rate_limit_requests` and then one request every `rate_limit_window_secs / rate_limit_requests`; IPv6 clients are limited per /64, so the IPv6 clients of the service's own network share one budget, as do all link-local clients, while each IPv4 client has its own; a throttled client is never forgotten early, and `Retry-After` is never 0. `rate_limit_window_secs` above 86400 and `rate_limit_max_entries = 0` are rejected, and `rate_limit_max_entries` is now the number of tracked clients above which idle clients are swept early rather than a hard cap
- **Breaking:** `sudo aranet-service service install` installs only on systemd, as the unprivileged `aranet.service` with its state in `/var/lib/aranet`, and refuses a binary that non-root users can modify, or a `--config` path that contains `..` or lies in `/home`, `/root`, `/run/user`, `/tmp` or `/var/tmp` (also through a symlink), which the unit hides from the service, or a `--database` outside `/var/lib/aranet`. System-level installs are refused on macOS (use `--user`), Windows (start it at logon with Task Scheduler), OpenRC and rc.d (write your own service file). The systemd unit is now `aranet.service` (was `rye-aranet.service`); installing removes the old one. On macOS, remove a LaunchDaemon that 0.2.x installed with `sudo aranet-service service uninstall`, then run `aranet-service service install --user`
- **Breaking (Docker):** The compose files publish their ports on `127.0.0.1` only (they published on every interface, and the root file used host networking). To reach the stack from other machines, change the `ports:` entries and turn on API-key authentication (see `docker/README.md`). The image's entrypoint now passes `--config /app/server.toml` itself: remove `--config` from your own `command:` or `docker run` arguments, or a command such as `["--config", "/app/server.toml"]` stops the container with a usage error

### Deprecated

- **`aranet_core::bluez_agent::ensure_agent`** does nothing and will be removed in 0.4.0; aranet-core now pairs devices itself while connecting
- **`aranet_cli::config::Config::save`** — use `Config::update`, which changes only the keys it is given, keeps comments and never overwrites a file that doesn't parse

### Removed

- **Breaking (library):** `aranet_service::middleware::sanitize_device_name` and `validate_device_id`, which nothing called
- **Breaking (library):** `aranet_service::AppState::save_config`; the service saves only its device list, through `Config::save_devices`
- **Breaking (library):** `aranet_service::Collector::task_count`
- **Breaking (library):** `aranet_service::state::CollectorState::try_start`, `signal_stop`, `reset_stop`, `subscribe_stop`, `wait_for_device_tasks`, `spawn_device_task`, `set_reload_watcher` and `wait_for_reload_watcher`, and its public `device_tasks` and `reload_watcher` fields; `CollectorState::set_running` is no longer public. Start and stop the collector with `Collector::start` and `Collector::stop`
- **Breaking (library):** `aranet_service::middleware::RateLimitState::check_rate_limit`, and the async `RateLimitState::cleanup(window, max)`
- **The root `Dockerfile` and `docker/server.toml.example`**; build with `docker build -f docker/Dockerfile .`. The image no longer contains `/app/server.toml.example`: start from `/app/server.toml`, the commented config the image now reads (`docker/server.toml` in the repository)

### Fixed

- **Linux** — Bluetooth calls no longer stall for 30 s and then fail once the tokio runtime that first used Bluetooth has shut down (between `#[tokio::test]`s, or in programs that build a runtime per call)
- **Scans** stop as soon as their caller is cancelled or times out (a service shutdown or a timed-out poll, the GUI's connect timeout, a cancelled passive monitor) instead of leaving the adapter scanning; on Linux a scan left running made every later scan in the process fail with `org.bluez.Error.InProgress`
- **Failed, timed-out or cancelled connects release the sensor** — a connect that fails, times out or is cancelled (Esc in the TUI, the GUI's timeout, a service shutdown or a timed-out poll) now disconnects, so the phone app or the next attempt can connect; before, the sensor could stay connected to the computer. Such a connect can take up to 5 s longer when the Bluetooth stack never confirms the disconnect, and a disconnect left to finish while the process exits (a service shutdown that drops a connect still in progress) is best effort. On Linux this includes a connect that BlueZ refuses because another program is pairing or connecting the sensor, and the disconnect cancels that pairing or connect: stop aranet while pairing a sensor by hand
- **Linux** — a connect whose service discovery stays empty now fails with a retryable connection error, instead of returning a device whose every read failed with "Characteristic not found (searched in 0 services)"
- **No more hangs when a sensor disappears** (macOS) — disconnecting, checking the connection, subscribing and reading RSSI are now time-limited, so `aranet watch`, the TUI, the GUI and the service report an error within seconds instead of freezing
- **Notification callbacks** — `Device::unsubscribe_from_notifications` stops the callback: it is never called after unsubscribe returns. A V1 history download that fails to send its request no longer leaves a background task behind; one that is cancelled part-way still keeps its task until the device disconnects
- **`aranet watch`** closes a lost connection before reconnecting, so its cleanup no longer drops the new connection and sends `watch` into a reconnect loop
- **Connection checks** — `Device::validate_connection` and `is_connection_alive` read the current-readings characteristic instead of the battery level, which needs pairing, so the TUI connects to unpaired sensors that can report readings (and, on Linux with BlueZ older than 5.80, whose battery plugin hides the battery level, to any sensor at all), and a connection check never starts a pairing that a reading wouldn't
- **Linux** — restarting `bluetoothd` no longer leaves a running aranet unable to pair until aranet is restarted
- **`ReconnectingDevice`** closes the old connection before reconnecting, so the old connection's cleanup no longer tears down the new one; it no longer panics when its attempts run out if `max_delay` is `Duration::MAX`
- **`DeviceManager` health monitor** replaces dropped and unresponsive connections instead of keeping a dead handle, and emits `Disconnected`, `ReconnectStarted` and `ReconnectSucceeded`. A cancelled or timed-out `DeviceManager::connect` no longer leaves the device unable to connect, and a `connect()` made while a cancelled `disconnect()` or `remove_device()` of the same device is still closing its connection waits for it instead of losing its new connection to it, and so does one made while a cancelled `connect()` is closing a connection that it had made or found dropped
- **Linux: first connections to an Aranet4** — connecting to a sensor whose services BlueZ hasn't cached yet (a new computer or sensor, or after `bluetoothctl remove`) no longer fails while BlueZ is still discovering them, with "Service discovery timed out" after 5 s or, when the connection itself took more than about 10 s to come up, with "Operation 'connect to device' timed out": aranet now waits for BlueZ to finish, for `ConnectionConfig::discovery_timeout` but at least 20 s, and disconnects if it doesn't. It logs `BlueZ is still discovering services; waiting up to 20s in all` when it starts waiting (`aranet` shows it with `--verbose`; `aranet-service` and `aranet server` only when `RUST_LOG` enables it, for example with `RUST_LOG=aranet_core=info`)
- **On macOS**, `aranet-service` and other long-running programs that connect repeatedly no longer grow by about 1-2 MB an hour: a device search now asks macOS only for devices that advertise an Aranet service, as every current Aranet sensor does (the growth comes from btleplug, which keeps every Bluetooth device it has seen; `aranet scan` and the passive monitor still scan every device)
- **Service collector** — starting, stopping and reloading the collector can no longer leave duplicate pollers, a stopped collector that keeps polling, or a running collector that polls nothing. `POST /api/collector/stop` returns within about a second even during a Bluetooth read, stopping a collector with no sensors also keeps it from starting when one is added, and shutdown stops a collector started through the API. A Bluetooth poll that hangs is abandoned after 180 s
- **Unreachable sensors** no longer keep the Bluetooth adapter busy: after repeated failures a sensor is retried at most every 5 minutes (or its poll interval, if longer), so the other sensors keep their schedule, and polls never run back to back to catch up
- **Desktop notifications** (`notifications` feature, including the Docker image) no longer stall API edits and integrations while an alert is shown, and use the sensor's current alias
- **Service API** stays responsive while `aranet sync`, the TUI, the GUI or `aranet cache prune` writes to the database or a large query runs, and the service keeps at most five database connections open. `--database :memory:` now serves the readings it collects
- **`aranet cache prune`** deletes in batches, so it no longer stalls a running service
- **Config files** keep their comments, layout and unknown keys when the service API, the CLI or the GUI changes them. The service writes only the device list (and a generated API key), so `--bind`/`--database` and default values no longer end up in `server.toml`. A file that doesn't parse is never overwritten, and saves are atomic and keep the file's owner
- **Webhooks** — a slow or unreachable endpoint no longer delays other alerts or makes the dispatcher miss readings; each request now times out after 10 s (was 30 s), and at most 16 alerts are delivered at once (one over that limit is dropped, with a warning)
- **InfluxDB export** — readings are batched and retried with backoff instead of being dropped on the first failed write; up to 10000 lines are kept during an outage
- **MQTT** — while the broker is unreachable, live readings are dropped with a warning instead of freezing the publisher; on reconnect, at most 100 messages queued at the start of the outage are sent before fresh readings
- **Service startup** — `server.bind` must be an `IP:port` address (`localhost:8080` is rejected with a clear message) and a busy port is reported before the database is opened or Bluetooth is used; a config file that exists but can't be read stops startup instead of being silently ignored; `storage.path` values starting with `~/` expand to the home directory (earlier versions created a directory named `~` in the working directory; a warning at every start says how to keep that data); `aranet server --daemon` reports configuration errors instead of printing that it started
- **mDNS** registration works on hosts whose name doesn't end in `.local` (most Linux and Windows machines), and macOS browsers list the service again. It is advertised as `Aranet Service on <name>` with the host name `aranet-<name>.local`, so it never takes over the machine's own `.local` name
- **Docker** — the image reads `/app/server.toml`, so configured sensors, integrations and `/metrics` work, including with `--no-collector`; it runs without privileged mode or host networking (BLE through a read-only `/run/dbus` mount) and publishes its ports on `127.0.0.1` only
- **`distribution/systemd/aranet.service`** starts without a manually created user or directory, reads `/var/lib/aranet/server.toml`, and drops the Bluetooth capabilities it never needed; `aranet-service service status` reports a service that isn't installed
- **`ARANET_DATA_DIR` set to an empty string** counts as unset, instead of putting the database (and now `cli-state.toml`) in the current directory

### Security

- **Linux BlueZ agent** — aranet no longer keeps a BlueZ agent registered, or asks to be the default agent, while it runs. When it connects to a sensor that isn't paired yet, it pairs that sensor explicitly (`Device1.Pair`) through an agent that it registers on its own D-Bus connection for that one pairing; the agent approves only that sensor and still rejects `AuthorizeService`. Sensors that are already paired, and scans, never register an agent. While aranet runs, the desktop's Bluetooth agent now handles pairing requests from other devices, so 0.2.1's advice to stop aranet before pairing other devices no longer applies
- **Config files** that can hold API keys and passwords are written owner-only (0600, new directories 0700) from the moment they are created, instead of briefly world-readable
- **Device aliases and addresses** containing control characters or backslashes, or longer than 64 characters, are rejected (HTTP 400, or a validation error at startup for `server.toml`), and InfluxDB tags and the measurement name are fully escaped, so an alias can no longer add points to an InfluxDB bucket or forge log lines. The service also refuses to start when the enabled InfluxDB exporter's `measurement` starts with `_` or `#` or contains a control character or a backslash, or when the enabled MQTT publisher's `topic_prefix` (or, with `homeassistant = true`, `ha_discovery_prefix`) contains `+`, `#` or a control character. A `measurement` whose spaces or commas were escaped by hand (`air\ quality`) must now be written plain (`air quality`): the service escapes it, and readings continue in the same measurement
- **Webhook, InfluxDB and push gateway URLs** no longer appear in the logs, including inside error messages: they are shown as `https://host/…` with the endpoint's number, because many webhook URLs contain their secret. The MQTT broker URL is logged without any `user:password@` part
- **Webhook, InfluxDB and push gateway requests no longer follow HTTP redirects** — a redirect could send an alert, a batch of readings or custom webhook headers to another host, including the service's own API. An endpoint that answers 301, 302, 307 or 308 (http to https, a trailing slash) now counts as failed, with a log line that says so, and the InfluxDB lines sent to it are dropped rather than retried: configure its final URL
- **DNS rebinding and cross-site requests** — while API-key authentication is off, the service rejects requests whose `Host` isn't local, an IP address, this machine's name or listed in the new `security.allowed_hosts` (421), and browser requests from other origins, including WebSocket connections (403)
- **Dashboard** — an API key passed as `?token=` (or `#token=`) is removed from the address bar and history entry, never sent as a `Referer`, and keys containing `+` work

## [0.2.1] - 2026-09-26

### Added

- **Linux ARM64 builds** — prebuilt CLI and GUI for 64-bit ARM Linux (`aarch64-unknown-linux-gnu`, glibc 2.35 or newer, such as Raspberry Pi OS Bookworm), offered by the install scripts
- **Release install test** — every release is installed with its own install scripts on each platform before it is published, and the documented one-liners are checked again once it is live; each macOS DMG now has a `.sha256`

### Changed

- **`aranet sync` with no device and no default device configured** now falls back to the last-used device, then (when `behavior.load_cache` is on, the default) the most recently seen device in the local database, as `read` and `status` do. It used to scan and prompt for a device, or fail with "No device specified" when not run interactively
- **Where `aranet sync` stores history** — 0.2.1 stores history under the device address. History that 0.2.0 synced under any other identifier (a device name or part of one, a MAC address typed in lowercase or without colons, or part of a macOS device UUID) stays under that ID, and `report`, `history` and `cache` queries for the old ID stop receiving new records. The first sync after upgrading downloads the device's whole history buffer again under the address, and `aranet sync --all` syncs the sensor once for each ID it is stored under. The `device` field in single-device `aranet sync --format json` output is now also this address rather than the identifier given on the command line; `aranet sync --all` output is unchanged
- **History deduplication window** — records within 30 s of a record already stored for the same device are skipped on every history write, including `aranet cache import` (so an import can now report skipped duplicates that are not exact timestamp matches). Aranet devices measure at most once a minute, so real records are never this close. Duplicate rows already in the database are not removed. Rows that 0.2.0 stored were shifted by the length of their download, which can exceed 30 s, so the first download after upgrading that overlaps them (such as `aranet history`, which saves the whole buffer) may still add a copy
- **`aranet set smart-home` without a value** is now an error. 0.2.0 release builds read it as `false`, a request to turn Smart Home off
- **Service WebSocket** — when the API key is sent as `?token=`, a request with more than one `token` parameter is now rejected. 0.2.0 used the first one
- **The prebuilt `aranet` CLI no longer includes `aranet gui`** — install `aranet-gui` or the macOS app for the desktop GUI (`cargo install aranet-cli --features gui` still builds it in)
- **Leaner `aranet-cli` builds** — `aranet-service` is compiled only with the `cli` feature (it runs `aranet server`), so `aranet-tui`, `aranet-gui` and TUI-only builds no longer compile the HTTP server; the unused `axum` and `tower-http` dependencies were removed
- **HTTP clients** — outgoing requests (webhooks, InfluxDB, Prometheus push gateway, service client) use HTTP/1.1, and `aranet server` no longer accepts cleartext HTTP/2 (the standalone `aranet-service` never did). The CLI, TUI and GUI no longer read macOS/Windows system proxy settings; `HTTP_PROXY`, `HTTPS_PROXY` and `NO_PROXY` still apply. When a webhook, InfluxDB or the Prometheus push gateway returns an error, `aranet server` now reads the response body in its error message as UTF-8, whatever charset the response declares, as the standalone `aranet-service` already did
- **Releases** — all seven crates take their version from the workspace manifest and are released with cargo-release: one `chore: release vX.Y.Z` commit, one `vX.Y.Z` tag on exactly that commit, and crates published to crates.io from that tag after the release binaries are built (CONTRIBUTING.md, "Releasing")
- **GitHub Release titles and notes** now come from this changelog
- **Homebrew** — one workflow (`.github/workflows/homebrew.yml`) updates the `cameronrye/aranet` tap after each release, and refuses to publish if a release asset is missing or doesn't match its published `.sha256`; pre-releases no longer touch the tap
- **Screenshots** — the screenshot workflow pushes its updates to a `screenshots/<tag>` branch for review instead of committing to `main`

### Removed

- Unused Homebrew formula sources (`distribution/homebrew/`) and the `aranet-cli.rb` / `aranet-gui.rb` release assets that no tap installed

### Fixed

- **Aranet2 and Aranet Radiation advertisements** are decoded at the offsets real devices use; Radiation dose rate is no longer reported 10× too high
- **History download** no longer drops the newest record, and no longer spins forever if a device repeats a packet
- **Duplicate history rows** — re-syncing a device no longer inserts copies of records already stored. History timestamps are anchored when the device is queried, and a record within 30 s of one already stored for the same device is skipped as a duplicate (see Changed)
- **`aranet sync`** resolves aliases like `read` and `status` do, and stores history under the connected device's address like other commands (see Changed)
- **`aranet status`** shows radon in pCi/L correctly (previously printed the Bq/m³ value with a pCi/L label); **`aranet report`** labels the radon threshold it actually counts, and its radon "time above" percentage now counts only records that have a radon value
- **`aranet set smart-home true|false`** is accepted (see Changed)
- **Service API** — `offset` without `limit` no longer returns 500; a rejected device update no longer corrupts the running configuration
- **Service WebSocket** — API keys containing `+`, `/` or `=` now work from the dashboard, which URL-encodes the `?token=` value. Clients that send the key unencoded keep working (see Changed)
- **macOS** — no longer leaks an OS thread on every Bluetooth connection. The shared Bluetooth adapter runs on a background thread of its own, so it keeps working for programs that use more than one tokio runtime, and it is replaced if its CoreBluetooth thread stops
- **Install scripts** — `aranet-cli-installer.sh`/`.ps1` and `aranet-gui-installer.sh`/`.ps1` offer every published platform again, and the macOS installs no longer fail with a checksum mismatch
- **Checksums** — the `.sha256` files and `sha256.sum` match the signed macOS archives, and `sha256.sum` lists every archive
- **Linux CLI** no longer needs GTK or libxdo installed
- **Windows GUI** no longer opens a console window; as a result, `aranet-gui --help` and `--version` print nothing in a Windows terminal (use `aranet` there)
- **macOS app** has its icon, minimum macOS version (11.0) and Bluetooth usage descriptions; the `aranet-gui` binary in the macOS tarballs is Developer ID signed and notarized like the CLI
- **`aranet-cli` with only the `tui` feature** (`cargo install aranet-cli --no-default-features --features tui`) compiles; the `aranet` binary now uses the library's TUI instead of compiling a second copy of it

### Security

- **Linux BlueZ agent** — aranet was already BlueZ's default agent while running, and 0.2.0 approved every pairing and authorization request from any device. It now approves pairing, confirmation and authorization only for devices aranet has connected to (or tried to) since it started, and always rejects `AuthorizeService`, which lets a remote device use the host's own Bluetooth profiles (such as HID input). As before, these requests don't reach the desktop's agent while aranet runs; pair other devices from the desktop's Bluetooth settings, or stop aranet first
- **Service logs** no longer record request query strings, which could include the API key
- **Dependencies updated** to clear the 9 RustSec vulnerability advisories that affect 0.2.0's lockfile: `h2` (RUSTSEC-2026-0258), `quick-xml` (RUSTSEC-2026-0194, RUSTSEC-2026-0195), `quinn-proto` (RUSTSEC-2026-0185), `rustls` (RUSTSEC-2026-0285), `rustls-webpki` (RUSTSEC-2026-0098, RUSTSEC-2026-0099, RUSTSEC-2026-0104) and `webbrowser` (RUSTSEC-2026-0257). This also replaces the unsound `anyhow`, `event-listener`, `lru`, `memmap2` and `rand` releases and the yanked `spin` 0.9.8
- **One TLS stack for HTTP**: webhooks, InfluxDB export, the Prometheus push gateway and the TUI/GUI service client now use rustls on every platform, trusting the OS certificate store plus the bundled Mozilla roots. The Linux `aranet` binary no longer links OpenSSL. MQTT over TLS (`mqtts://`, the `mqtt` feature of `aranet-service`) still uses the platform TLS library until `rumqttc` ships a patched `rustls-webpki`
- **GitHub Actions** — every hand-written workflow gives its `GITHUB_TOKEN` only the permissions each job needs (dist's generated `release.yml` still sets `contents: write` for the whole workflow), every action is pinned to a full commit SHA, and no checkout leaves the token in its git config
- **Homebrew tap token** — no longer handed to a third-party action

### Known issues

- **Aranet4 on Linux** — connecting to an Aranet4 can fail with "Service discovery timed out", most often the first time. On Linux, the Bluetooth library aranet uses gives BlueZ only 5 seconds to discover the sensor's services, and the Aranet4 can take longer. A failed attempt can also leave the sensor connected until BlueZ drops the link. 0.2.0 behaves the same; the next release fixes both

## [0.2.0] - 2026-03-28

### Added

- **Docker support** - Multi-stage `Dockerfile` and `docker-compose.yml` at the repo root for containerized deployment with BLE passthrough, health checks, and persistent storage
- **Homebrew formula automation** - GitHub Actions workflow (`.github/workflows/homebrew.yml`) automatically publishes updated Homebrew formulae to the `cameronrye/homebrew-aranet` tap on each release
- **CLI cached reports** - `aranet report` summarizes cached daily, weekly, or monthly history in text or JSON without reconnecting to devices
- **Webhook notifications** - HTTP POST alerts when CO2, radon, or battery thresholds are exceeded, with configurable endpoints, per-device cooldowns, and custom headers for Slack/Discord/PagerDuty integration
- **InfluxDB export** - Real-time sensor data export to InfluxDB v2 using line protocol, with configurable organization, bucket, and measurement names
- **mDNS service discovery** - Automatic network advertisement via `_aranet._tcp.local.` and `_http._tcp.local.` so clients can discover the service without manual IP configuration
- **Grafana dashboard template** - Pre-built dashboard (`grafana/aranet-dashboard.json`) with panels for CO2 gauges, environment time series, radon/radiation, battery levels, and collector statistics
- **Improved web dashboard** - Tab-based UI with Overview (live sparklines), History (interactive charts with statistics), and Service Status (collector health and per-device poll stats)
- **Home Assistant MQTT auto-discovery** - `mqtt.homeassistant = true` publishes discovery payloads while `mqtt.ha_discovery_prefix` controls the HA discovery namespace
- **E2E API tests** - 27 integration tests covering health, devices, readings, configuration, authentication, Prometheus metrics, and broadcast channel integration
- **BLE adapter serialization** - Semaphore-based locking ensures only one device uses the Bluetooth adapter at a time, preventing connection contention
- **Staggered device startup** - Devices start polling with a 5-second stagger to avoid adapter contention on boot
- **Cached BLE manager** - Shared `Manager` instance with automatic reset on D-Bus connection failure
- **Poll duration metric** - New `aranet_device_poll_duration_ms` Prometheus gauge tracks how long each device poll takes
- **Staleness detection** - `/api/devices/:id/current` now returns `age_seconds` and `stale` fields
- **`DeviceType` capability methods** - `has_co2()`, `has_temperature()`, `has_humidity()`, `has_pressure()` for querying sensor capabilities
- **Aranet Radiation ☢ symbol detection** - `DeviceType::from_name` now recognizes the Unicode ☢ (U+2622) in device names

### Changed

- **Aranet2 GATT protocol** - Fixed parsing to use correct 12-byte format with proper field ordering (header, interval, age, battery, temp, humidity, status flags)
- **Aranet2 status bit extraction** - Status is now read from bits[2:3] (temperature status) instead of bits[0:1]
- **Aranet2 humidity parsing** - Humidity is now parsed as u16 LE divided by 10 (was u8 raw)
- **BlueZ agent** - Now registers as default agent with retry cap (max 3 attempts) and permanent failure state
- **Prometheus metrics** - Metrics are now filtered by device capabilities (e.g., Aranet2 won't emit `aranet_co2_ppm`)
- **Prometheus device labels** - Device labels now use configured aliases when available
- **MQTT publishing** - Published topics now use configured aliases when available and include radon averages when present
- **Passive monitor** - Retries adapter acquisition on startup instead of exiting immediately; logs all consecutive errors
- **Service discovery retry** - Automatically disconnects and retries when BlueZ returns 0 services (stale state)
- **Connection timeouts** - `challenging_environment()` preset now uses 90s connection / 30s discovery timeouts

### Breaking

- **`GET /api/devices/:id/current`** now returns `CurrentReadingResponse` with `age_seconds` and `stale` fields alongside the reading (previously returned raw `StoredReading`)
- **WebSocket query-token auth** - the `token` query parameter is now accepted only on `/api/ws`; REST and metrics requests must send `X-API-Key`

### Internal

- Bumped Rust edition to 2024 with minimum supported Rust version 1.90
- Removed legacy advertisement parsers (v1 format) — only v2 parsers are used
- Consolidated GATT reading parsers in `aranet-core` to delegate to `aranet-types::CurrentReading`
- Extracted `spawn_staggered_device_tasks()` helper to deduplicate collector startup logic
- Synchronized all crate versions to 0.2.0

## [0.1.13] - 2026-03-25

### Added

- **BlueZ Agent** - Register a BlueZ agent on Linux to prevent BLE hangs during service discovery

### Fixed

- Fix BLE connection hang on Linux caused by unhandled BlueZ pairing/authorization requests
- Update vulnerable dependencies: `bytes` 1.11.1, `quinn-proto` 0.11.14, `rustls-webpki` 0.103.10, `time` 0.3.47
- Ignore unpatchable `rustls-webpki` 0.102.x advisory (pinned by `rumqttc`)

## [0.1.12] - 2026-02-01

### Added

- **Diagnostics Module** - Comprehensive Bluetooth diagnostics and troubleshooting
  - `DiagnosticsCollector` for gathering adapter info, connection stats, and recent errors
  - `AdapterInfo` with state detection (Available, PoweredOff, NotFound)
  - `ConnectionStats` tracking success rates, connection times, and disconnection reasons
  - `ErrorCategory` classification with recorded error history
  - Global diagnostics collector accessible via `global_diagnostics()`

- **Passive Monitoring** - Monitor devices via BLE advertisements without connecting
  - `PassiveMonitor` for low-power, connectionless monitoring
  - Support for more devices than the BLE connection limit
  - Configurable deduplication and device filtering
  - `PassiveReading` with device ID, name, RSSI, and parsed advertisement data

- **Platform Module** - Platform-specific BLE configuration and tuning
  - `Platform` detection (macOS, Linux, Windows) with appropriate defaults
  - `PlatformConfig` with recommended timeouts and scan durations
  - Cross-platform device aliasing with `DeviceAlias` and `AliasStore`
  - Documentation of macOS UUID behavior and cross-platform considerations

- **Retry Logic** - Configurable retry policies for BLE operations
  - `RetryPolicy` with immediate, fixed, and exponential backoff strategies
  - Configurable max attempts, delays, and jitter
  - Integration with connection and read operations

- **History Improvements**
  - `HistoryCheckpoint` for resumable history downloads
  - `PartialHistoryData` for incremental sync with checkpoint tracking
  - History sync progress events with record counts

- **Device Connection Improvements**
  - `ConnectionConfig` for configurable connection settings
  - `SignalQuality` enum (Excellent, Good, Fair, Poor, Unknown) for RSSI classification
  - Improved connection state tracking

- **Device Manager Improvements**
  - `AdaptiveInterval` for smart polling that adjusts based on device activity
  - `DevicePriority` for prioritizing certain devices during polling

- **Error Context** - User-friendly error messages with actionable suggestions
  - `ErrorContext` with message, retryable flag, and suggestion
  - Automatic classification of errors from `aranet_core::Error`
  - Suggestions for common issues (out of range, Bluetooth errors, etc.)

- **New Commands**
  - `CancelOperation` - Cancel long-running scans, syncs, and other operations
  - `StartBackgroundPolling` / `StopBackgroundPolling` - Automatic device polling
  - System service management from GUI/TUI

- **GUI Service Panel** - Manage aranet-service from the desktop application
  - View service status (installed, running)
  - Install/uninstall system service (user-level, no admin required)
  - Start/stop service controls
  - Device collection statistics

- **API Improvements**
  - `/api/health/detailed` endpoint with database, collector, and platform diagnostics
  - Improved Prometheus metrics with better lock management
  - Enhanced error responses with context

### Changed

- Improved Prometheus metrics collection performance (shorter lock duration)
- Enhanced history sync events with progress tracking
- Better error propagation with context information
- Synchronized all crate versions to 0.1.12

### Fixed

- Lock contention in Prometheus metrics endpoint

## [0.1.11] - 2026-01-31

### Added

- **MQTT Integration** - Publish sensor readings to MQTT brokers for IoT integration
  - Real-time publishing of all sensor readings to configurable topics
  - Topic structure: `{prefix}/{device}/co2`, `/temperature`, `/humidity`, `/pressure`, `/battery`, `/status`, `/json`
  - Support for radon (`/radon`) and radiation (`/radiation_rate`, `/radiation_total`) when available
  - TLS support via `mqtts://` scheme
  - Configurable QoS levels (0, 1, 2), retain flag, and authentication
  - Automatic reconnection with 5-second retry intervals
  - Feature flag: `mqtt` (included in `full` feature)

- **Prometheus Metrics Endpoint** - Export sensor data for monitoring and alerting
  - `/metrics` endpoint with Prometheus text format (0.0.4)
  - Sensor metrics: `aranet_co2_ppm`, `aranet_temperature_celsius`, `aranet_humidity_percent`, `aranet_pressure_hpa`, `aranet_battery_percent`, `aranet_reading_age_seconds`
  - Radon/radiation metrics when available: `aranet_radon_bqm3`, `aranet_radiation_rate_usvh`, `aranet_radiation_total_msv`
  - Collector statistics: `aranet_collector_running`, `aranet_collector_uptime_seconds`, `aranet_device_poll_success_total`, `aranet_device_poll_failure_total`
  - Optional push gateway support with configurable interval
  - Feature flag: `prometheus` (included in `full` feature)

- **Service Management Commands** - Control aranet-service from CLI
  - `aranet-service service install [--user]` - Install as system/user service
  - `aranet-service service uninstall [--user]` - Uninstall service
  - `aranet-service service start/stop/status [--user]` - Control service

- **REST API Enhancements**
  - `POST /api/collector/start` and `POST /api/collector/stop` - Control background collector
  - `GET /api/status` - Full service status with collector state and device statistics
  - `GET/PUT /api/config` - Runtime configuration management
  - `POST/PUT/DELETE /api/config/devices` - Dynamic device management

- **GUI Improvements**
  - Multi-panel architecture: Device List, Device Detail, History, Comparison, Alerts, Service, Settings
  - Export functionality (CSV/JSON) for historical data
  - Comparison view for side-by-side multi-device analysis
  - Alert system with threshold management and alert history
  - Service panel for background collector control
  - macOS native menu bar integration
  - System tray with quick access to readings
  - Light/dark theme support

- **TUI Service Tab** - Monitor and control aranet-service from terminal dashboard
  - Service status display with collector state and uptime
  - Per-device polling statistics
  - Start/stop collector controls

- **Service Client Library** - `service-client` feature in aranet-core
  - Type-safe HTTP client for aranet-service API
  - Service status, collector control, and configuration management

- Downloads page on website with direct download links for all platforms
- New icon assets with multiple resolutions (16px to 1024px)
- `docs/ARCHITECTURE.md` with comprehensive technical documentation
- Troubleshooting guide on website
- Entitlements for Bluetooth access on macOS GUI app

### Changed

- Updated screenshots in README and website
- Improved CI workflow for screenshots and site deployment
- Synchronized all crate versions to 0.1.11

### Removed

- `aranet-wasm` crate removed from workspace (was not published)
- `ROADMAP.md` removed in favor of website roadmap page

### Fixed

- Icon asset included in aranet-cli for crates.io publishing
- Screenshots and deploy-site workflow failures
- Removed references to non-existent CLI flags from troubleshooting documentation

## [0.1.9] - 2026-01-22

### Changed

- Replaced unmaintained `atty` crate with `std::io::IsTerminal` (resolves RUSTSEC-2024-0375)
- Updated `rusqlite` from 0.33 to 0.35
- Updated multiple dependencies to latest versions
- Marked `aranet-gui` and `aranet-wasm` as `publish = false` (not ready for release)

### Added

- CONTRIBUTING.md with contribution guidelines
- SECURITY.md with vulnerability reporting process
- GitHub issue templates for bug reports and feature requests
- Pull request template

## [0.1.8] - 2026-01-19

### Added

- **TUI Dashboard Enhancements** - Complete overhaul with 44 new features
  - **Navigation**: Tab/Shift+Tab for tabs, j/k/arrows for devices, vim-style keybindings
  - **Auto-refresh**: Readings update automatically based on device interval
  - **Trend indicators**: Up/down/stable arrows next to readings
  - **Statistics**: Min/Max/Avg CO2 stats, radon 1-day/7-day averages
  - **Alerts**: CO2/radon threshold alerts with Info/Warning/Critical severity levels
  - **Alert history**: View past alerts with 'a' key, sticky alerts with 'A'
  - **Terminal bell**: Audio alert on threshold breach (toggle with 'b')
  - **Sparkline charts**: CO2/radon history with min/max labels and time axis
  - **Full-screen chart**: Press 'g' for expanded chart view
  - **Multiple metrics**: Stack temperature/humidity on chart with T/H keys
  - **Time range filter**: 0=all, 1=today, 2=24h, 3=7d, 4=30d
  - **Scrollable history**: PgUp/PgDn to scroll through records
  - **Export history**: Press 'e' to export visible history to CSV
  - **Device filter**: Cycle filter with 'f' (All/Aranet4/Radon/Radiation/Connected)
  - **Comparison view**: Side-by-side device readings with 'v', cycle with '<'/'>'
  - **Connect all**: Connect to all known devices with 'C'
  - **Device alias**: Set friendly names with 'n' key
  - **Settings editing**: Change interval with Enter, BLE range with 'B', Smart Home with 'I'
  - **Threshold config**: Adjust CO2/radon thresholds with +/- keys
  - **Theme support**: Toggle light/dark theme with 't' key
  - **Responsive layout**: Auto-hide sidebar on narrow terminals, toggle with '['
  - **Wider sidebar**: Toggle sidebar width with ']' key
  - **Mouse support**: Click to select devices, tabs, and buttons
  - **RSSI signal strength**: Visual signal bars for connected devices
  - **Device uptime**: Shows how long device has been connected
  - **Battery warning**: Alert when battery drops below 20%
  - **Reading age warning**: Highlight stale readings (> 2x interval)
  - **Loading spinners**: Visual feedback during connect/sync operations
  - **Status messages**: Queue of messages with auto-dismiss timeout
  - **Confirmation dialogs**: Y/N prompts before destructive actions
  - **Error details**: View full error with 'E' key
  - **Help overlay**: Press '?' for organized keyboard shortcuts cheatsheet
  - **Header bar**: Shows connected count, avg CO2, alert count, indicators
  - **ASCII-only output**: All indicators use pure ASCII characters for compatibility

## [0.1.7] - 2026-01-18

### Added

- **aranet-store crate** - New SQLite-based local data persistence layer
  - Store current readings and device metadata
  - Cache history records from devices for offline access
  - Incremental sync support (only download new records)
  - Query by device, time range, with pagination
  - Automatic deduplication of history records
  - Platform-specific database locations:
    - Linux: `~/.local/share/aranet/data.db`
    - macOS: `~/Library/Application Support/aranet/data.db`
    - Windows: `C:\Users\<user>\AppData\Local\aranet\data.db`

- **CLI sync command** - Download device history to local database
  - `aranet sync --device <ADDRESS>` for incremental sync
  - `aranet sync --device <ADDRESS> --full` for complete re-download
  - Progress bar during history download

- **CLI cache command** - Query cached data without device connection
  - `aranet cache devices` - List all cached devices
  - `aranet cache stats` - Show cache statistics (readings, history counts)
  - `aranet cache history` - Query cached history with filters
  - `aranet cache info` - Show database path and size

### Changed

- All crate versions bumped to 0.1.7

## [0.1.6] - 2026-01-18

### Added

- **Code coverage with cargo-llvm-cov** - CI now reports test coverage via Codecov
- **Property-based testing with proptest** - Fuzz testing for all byte parsers
  - aranet-types: CurrentReading parser fuzzing
  - aranet-core: All device parsers (Aranet4, Aranet2, Radon, Radiation)
  - aranet-core: Advertisement parsing fuzzing
- **GUI tests** - Component tests for AppState (6 new tests)
- **TUI tests** - Component tests for App key handling (6 new tests)
- **Expanded MockDevice tests** - Comprehensive coverage for history, settings, calibration

### Fixed

- **Aranet Radiation advertisement parser panic** - Fixed crash on malformed data (found by proptest)
  - Corrected minimum byte length check from 19 to 21 bytes

### Changed

- Test count increased from 268 to 310+ tests
- All test modules now have comprehensive inline documentation

## [0.1.5] - 2026-01-18

### Added

- **Multi-device passive watch mode**
  - `aranet watch --passive` now monitors ALL devices broadcasting advertisements
  - Each reading clearly shows device name: `[AranetRn+ 306B8]`
  - No longer defaults to last connected device when watching all devices
  - Supports CSV, JSON, and text output formats with device identification

### Changed

- Improved watch output formatting with clearer device identification
- Passive mode header now shows "Watching: all devices (passive)"
- Consistent separator line formatting across watch modes

## [0.1.4] - 2026-01-17

### Fixed

- Fix clippy warnings for Rust 2024 edition compliance
- Resolve collapsible if/else-if blocks in CLI styling code
- Remove unnecessary `.clone()` on Copy types in tests
- Fix manual range contains and clamp patterns

## [0.1.3] - 2026-01-17

### Added

- **Rich CLI styling** (now the default)
  - Spinners for long-running operations (scan, connect, history download)
  - Color-coded sensor values based on thresholds (CO2, radon, battery, humidity, temperature)
  - Table formatting with `tabled` for history, info, alias, and scan output
  - Trend indicators in watch mode (up/down/stable arrows)
  - `--style` flag: `rich` (default), `minimal`, or `plain` for scripting
  - `--brief` flag for status command (compact one-line output)
  - Device name headers in read output
  - Air quality summary labels (Excellent, Good, Fair, Poor)
- **Pressure unit conversion** (`--inhg` / `--hpa` flags)
  - Display pressure in inches of mercury (inHg) with `--inhg`
  - Explicitly request hPa with `--hpa` (default)
  - Configurable via config file (`inhg = true`)
- **`doctor` command** for BLE diagnostics
  - Checks Bluetooth adapter availability and permissions
  - Scans for devices to verify BLE functionality
  - Platform-specific troubleshooting tips (macOS, Linux, Windows)
  - Numbered progress steps with colored status indicators
- **`alias` command** for device management
  - `alias list` - Show all saved device aliases (now with table formatting)
  - `alias set <name> <address>` - Create a friendly name for a device
  - `alias remove <name>` - Delete an alias
  - Use aliases anywhere a device address is expected
- **Passive read mode** (`--passive` flag on `read`)
  - Read sensor data from BLE advertisements without connecting
  - Requires Smart Home integration enabled on the device
  - Faster readings when device data is advertised
- **Multi-device read support**
  - Specify multiple devices: `aranet read -d device1 -d device2`
  - Comma-separated: `aranet read -d device1,device2`
  - Parallel reading from all devices
  - Combined output in text, JSON, and CSV formats
- **Interactive device picker**
  - When no device is specified, scan and present a selection menu
  - Works for `read`, `history`, `info`, `status`, `watch` commands
- **History date filters** (`--since` / `--until`)
  - Filter history by date range
  - Supports RFC3339 format (e.g., `2026-01-15T10:30:00Z`)
  - Supports date-only format (e.g., `2026-01-15`)
- **Progress bars for history download**
  - Visual progress indicator with percentage and current parameter
  - Shows download progress across all history parameters

### Changed

- Default style mode changed from `minimal` to `rich`
- Replaced unmaintained `atty` crate with `std::io::IsTerminal`
- Refactored `cmd_history` to use `HistoryArgs` struct (clippy compliance)

### Fixed

- macOS device identifier now uses CoreBluetooth UUID instead of placeholder address

## [0.1.2] - 2026-01-16

### Added

- **Full AranetRn+ (Radon) sensor support**
  - Current readings: radon (Bq/m³), temperature, pressure, humidity, battery, interval, age
  - History download: 4-byte radon values (param 10), humidity in tenths (param 5)
  - `HistoryRecord.radon` field for radon history data
  - `HistoryParam::Radon` and `HistoryParam::Humidity2` enum variants
- Complete BLE communication stack with btleplug 0.11
  - Device scanning and discovery
  - Connection management with auto-reconnection
  - Current readings for all device types
  - History download (V1 notification-based, V2 read-based)
  - Device settings read/write (interval, Bluetooth range, Smart Home)
- Multi-device support (Aranet4, Aranet2, AranetRn+, Aranet Radiation)
- Core types (CurrentReading, DeviceInfo, HistoryRecord, Status, DeviceType)
- BLE UUIDs for Aranet devices (both old and new firmware)
- **CLI fully implemented** with all core commands:
  - `scan` — Discover nearby Aranet devices
  - `read` — Read current sensor measurements
  - `status` — Quick one-line reading with colored CO₂ status
  - `info` — Display device information
  - `history` — Download historical data (text, JSON, CSV)
  - `set` — Modify device settings (interval, range, smart_home)
  - `watch` — Continuous monitoring with auto-reconnect
  - `config` — Manage configuration file (`~/.config/aranet/config.toml`)
  - `completions` — Generate shell completions (bash, zsh, fish, PowerShell)
- TUI app shell with ratatui
- GUI app shell with egui/eframe
- WASM module scaffolding
- Error types with thiserror
- Real-time streaming with `ReadingStream`
- Event system with `EventDispatcher`
- Connection metrics and operation tracking
- Mock device for testing
- Data validation and CO2 threshold helpers
- JSON and CSV output formats for all CLI commands
- Colored CO₂ status indicators (green/amber/red)
- Config file support with device, format, no_color, fahrenheit options
- `ARANET_DEVICE` environment variable support
- `--no-color` flag and `NO_COLOR` env var support

### Fixed

- Corrected UUID mappings for history characteristics (V1 → 2003, V2 → 2005)
- Fixed V2 history response parsing (10-byte header format)
- Resolved async deadlock in device connection
- Increased connection timeout for reliable BLE connections
- Corrected AranetRn+ GATT data format parsing (device_type, interval, age fields)

### Changed

- Updated to Rust 1.90 minimum (edition 2024)
- History download now uses correct parameter values for each sensor type
