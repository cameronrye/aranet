# Aranet BLE Protocol Reference

This document consolidates the complete BLE GATT protocol for Aranet devices, extracted from the Aranet4-Python library.

> **Source**: [Aranet4-Python](https://github.com/Anrijs/Aranet4-Python) by Anrijs Jargans

---

## Device Types

| Type Code | Name | Model |
|-----------|------|-------|
| `0xF1` | ARANET4 | Aranet4 (CO₂ Monitor) |
| `0xF2` | ARANET2 | Aranet2 (Temp/Humidity) |
| `0xF3` | ARANET_RADON | Aranet Radon Plus |
| `0xF4` | ARANET_RADIATION | Aranet Radiation |

---

## Manufacturer ID

```rust
const MANUFACTURER_ID: u16 = 0x0702;  // SAF Tehnika
```

---

## Service UUIDs

| Service | UUID | Notes |
|---------|------|-------|
| **SAF Tehnika (NEW)** | `0000fce0-0000-1000-8000-00805f9b34fb` | Firmware v1.2.0+ |
| **SAF Tehnika (OLD)** | `f0cd1400-95da-4f4b-9ac8-aa55d312af0c` | Pre-v1.2.0 |
| GAP Service | `00001800-0000-1000-8000-00805f9b34fb` | Standard BLE |
| Device Information | `0000180a-0000-1000-8000-00805f9b34fb` | Standard BLE |
| Battery Service | `0000180f-0000-1000-8000-00805f9b34fb` | Standard BLE |
| Nordic DFU | `0000fe59-0000-1000-8000-00805f9b34fb` | Firmware updates |

---

## Characteristic UUIDs

### GAP Service (`0x1800`)

| Characteristic | UUID | Type |
|----------------|------|------|
| Device Name | `00002a00-...` | String |
| Appearance | `00002a01-...` | u16 |

### Device Information Service (`0x180a`)

| Characteristic | UUID (short) | Type |
|----------------|--------------|------|
| System ID | `0x2a23` | raw |
| Model Number | `0x2a24` | String |
| Serial Number | `0x2a25` | String |
| Firmware Rev | `0x2a26` | String |
| Hardware Rev | `0x2a27` | String |
| Software Rev | `0x2a28` | String |
| Manufacturer Name | `0x2a29` | String |

### Battery Service (`0x180f`)

| Characteristic | UUID (short) | Type |
|----------------|--------------|------|
| Battery Level | `0x2a19` | u8 (0-100%) |

### SAF Tehnika Service

| Characteristic | UUID | Purpose |
|----------------|------|---------|
| Sensor State | `f0cd1401-95da-4f4b-9ac8-aa55d312af0c` | Device settings |
| Command | `f0cd1402-95da-4f4b-9ac8-aa55d312af0c` | Write commands |
| Calibration Data | `f0cd1502-95da-4f4b-9ac8-aa55d312af0c` | Calibration |
| Current Readings | `f0cd1503-95da-4f4b-9ac8-aa55d312af0c` | Basic readings |
| Current Readings (Aranet2) | `f0cd1504-95da-4f4b-9ac8-aa55d312af0c` | Aranet2 only |
| Total Readings | `f0cd2001-95da-4f4b-9ac8-aa55d312af0c` | History count |
| Read Interval | `f0cd2002-95da-4f4b-9ac8-aa55d312af0c` | Interval seconds |
| History V1 | `f0cd2003-95da-4f4b-9ac8-aa55d312af0c` | History (notify) |
| Seconds Since Update | `f0cd2004-95da-4f4b-9ac8-aa55d312af0c` | Last update |
| History V2 | `f0cd2005-95da-4f4b-9ac8-aa55d312af0c` | History (read) |
| Current Readings Detail | `f0cd3001-95da-4f4b-9ac8-aa55d312af0c` | Extended readings |
| Current Readings A | `f0cd3002-95da-4f4b-9ac8-aa55d312af0c` | Alternative |
| Current Readings A (Aranet2) | `f0cd3003-95da-4f4b-9ac8-aa55d312af0c` | Aranet2 only |

### Nordic DFU Service (`0xfe59`)

| Characteristic | UUID | Purpose |
|----------------|------|---------|
| Secure DFU | `8ec90003-f315-4f60-9fb8-838830daea50` | Firmware update |

---

## Data Parsing

### Aranet4 Current Readings (`f0cd3001`)

13 bytes total:

| Offset | Bytes | Name | Type | Transform |
|--------|-------|------|------|-----------|
| 0-1 | SS:SS | CO₂ (ppm) | u16LE | none |
| 2-3 | TT:TT | Temperature | u16LE | ÷ 20.0 → °C |
| 4-5 | UU:UU | Pressure | u16LE | ÷ 10 → hPa |
| 6 | VV | Humidity (%) | u8 | none |
| 7 | WW | Battery (%) | u8 | none |
| 8 | XX | Status | u8 | See Color enum |
| 9-10 | YY:YY | Interval (s) | u16LE | none |
| 11-12 | ZZ:ZZ | Age (s) | u16LE | none |

### Aranet2 Current Readings (GATT)

| Offset | Name | Type | Transform |
|--------|------|------|-----------|
| 0-1 | Unknown | u16LE | - |
| 2-3 | Interval (s) | u16LE | none |
| 4-5 | Age (s) | u16LE | none |
| 6 | Battery (%) | u8 | none |
| 7-8 | Temperature | u16LE | ÷ 20.0 → °C |
| 9-10 | Humidity | u16LE | ÷ 10.0 → % |
| 11 | Status Flags | u8 | See below |

Status flags: `bits[0:1]` = humidity status, `bits[2:3]` = temperature status

### Aranet Radiation Current Readings (GATT)

| Offset | Name | Type | Transform |
|--------|------|------|-----------|
| 0-1 | Unknown | u16LE | - |
| 2-3 | Interval (s) | u16LE | none |
| 4-5 | Age (s) | u16LE | none |
| 6 | Battery (%) | u8 | none |
| 7-10 | Dose Rate (nSv/h) | u32LE | ÷ 1000 → µSv/h |
| 11-18 | Dose Total (nSv) | u64LE | ÷ 1000000 → mSv |
| 19-26 | Duration (s) | u64LE | none |
| 27 | Status | u8 | - |

### Aranet Radon Current Readings (GATT)

18 bytes minimum (extended format includes averages):

| Offset | Name | Type | Transform |
|--------|------|------|-----------|
| 0-1 | Device Type | u16LE | 0x0003 = Radon |
| 2-3 | Interval (s) | u16LE | none |
| 4-5 | Age (s) | u16LE | none |
| 6 | Battery (%) | u8 | none |
| 7-8 | Temperature | u16LE | ÷ 20.0 → °C |
| 9-10 | Pressure | u16LE | ÷ 10 → hPa |
| 11-12 | Humidity | u16LE | ÷ 10.0 → % |
| 13-16 | Radon (Bq/m³) | u32LE | none |
| 17 | Status | u8 | See Color enum |
| 18+ | Averages | u64LE × 3 | 24h, 7d, 30d (optional) |

---

## Commands (Write to `f0cd1402`)

### Set Interval (`0x90`)

```
Bytes: 90:XX
XX = Interval in minutes (01, 02, 05, 0A)
```

| Value | Interval |
|-------|----------|
| `0x01` | 1 minute |
| `0x02` | 2 minutes |
| `0x05` | 5 minutes |
| `0x0A` | 10 minutes |

### Toggle Smart Home Integration (`0x91`)

```
Bytes: 91:XX
XX = 00 (disabled) or 01 (enabled)
```

### Set Bluetooth Range (`0x92`)

```
Bytes: 92:XX
XX = 00 (standard) or 01 (extended)
```

### Request History V1 (`0x82`)

```
Bytes: 82:PP:00:00:SS:SS:EE:EE
PP = Parameter (see Param enum)
SS:SS = Start index (u16LE, starts at 1)
EE:EE = End index (u16LE)
```

### Request History V2 (`0x61`)

```
Bytes: 61:PP:SS:SS
PP = Parameter (see Param enum)
SS:SS = Start index (u16LE)
```

---

## Parameter Enum

| Value | Name | Data Size | Notes |
|-------|------|-----------|-------|
| 1 | TEMPERATURE | u16 | Raw ÷ 20 = °C |
| 2 | HUMIDITY | u16 | Percentage (0-100) |
| 3 | PRESSURE | u16 | Raw ÷ 10 = hPa |
| 4 | CO2 | u16 | ppm |
| 5 | HUMIDITY2 | u16 | Tenths of % (for AranetRn+) |
| 6 | PULSES | u16 | Radiation pulses |
| 7 | RADIATION_DOSE | u32 | Total dose (nSv) |
| 8 | RADIATION_DOSE_RATE | u32 | Dose rate (nSv/h) |
| 9 | RADIATION_DOSE_INTEGRAL | u64 | Integral dose |
| 10 | RADON_CONCENTRATION | u32 | Bq/m³ (4 bytes) |

**Note**: Parameters 1-6 use 2-byte values. Parameters 7-10 use 4-byte values (u32).
For history download, each parameter type must be downloaded separately.

---

## Status Colors (CO₂)

| Value | Color | CO₂ Range |
|-------|-------|-----------|
| 0 | ERROR | - |
| 1 | GREEN | < 1000 ppm |
| 2 | YELLOW/AMBER | 1000-1400 ppm |
| 3 | RED | > 1400 ppm |

---

## Advertisement Data

### Detection

```rust
// Check manufacturer data for SAF Tehnika
manufacturer_id == 0x0702

// Device type is first byte
match data[0] {
    0xF1 => Aranet4,
    0xF2 => Aranet2,
    0xF3 => Aranet Radon,
    0xF4 => Aranet Radiation,
}
```

### Aranet4 Advertisement

| Offset | Name | Type |
|--------|------|------|
| 0 | Type (0xF1) | u8 |
| 1 | Flags | u8 |
| 2-3 | CO₂ (ppm) | u16LE |
| 4-5 | Temperature | u16LE ÷ 20 |
| 6-7 | Pressure | u16LE ÷ 10 |
| 8 | Humidity (%) | u8 |
| 9 | Battery (%) | u8 |
| 10 | Status | u8 |
| 11-12 | Interval (s) | u16LE |
| 13-14 | Age (s) | u16LE |
| 15 | Counter | u8 |

---

## History Reading

### V1 (Notification-based)

1. Write request to Command characteristic (`0x82` header)
2. Subscribe to History V1 notifications
3. Receive packets with format:
   - Byte 0: Data type (Param)
   - Bytes 1-2: Start index (u16LE)
   - Byte 3: Count
   - Bytes 4+: Data values

### V2 (Read-based)

1. Write request to Command characteristic (`0x61` header)
2. Poll History V2 characteristic
3. Packet ends when index > total size

---

## Rust Implementation Notes

### UUID Helper

```rust
use uuid::{uuid, Uuid};

// Short UUID to full UUID
fn normalize_uuid_16(short: u16) -> Uuid {
    uuid::Uuid::from_u128(
        (short as u128) << 96 
        | 0x0000_1000_8000_00805f9b34fb
    )
}

// Or use the uuid crate directly
const SERVICE_SAF_TEHNIKA: Uuid = uuid\!("0000fce0-0000-1000-8000-00805f9b34fb");
```

### Parsing Example

```rust
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::Cursor;

fn parse_aranet4_reading(data: &[u8]) -> Result<Reading, Error> {
    let mut cursor = Cursor::new(data);
    
    Ok(Reading {
        co2: cursor.read_u16::<LittleEndian>()?,
        temperature: cursor.read_u16::<LittleEndian>()? as f32 / 20.0,
        pressure: cursor.read_u16::<LittleEndian>()? as f32 / 10.0,
        humidity: cursor.read_u8()?,
        battery: cursor.read_u8()?,
        status: Status::try_from(cursor.read_u8()?)?,
        interval: cursor.read_u16::<LittleEndian>()?,
        age: cursor.read_u16::<LittleEndian>()?,
    })
}
```

---

## References

- [docs/UUIDs.md](./UUIDs.md) - Original UUID documentation
- [Aranet4-Python client.py](https://github.com/Anrijs/Aranet4-Python/blob/master/aranet4/client.py) - Reference implementation
- [btleplug examples](https://github.com/deviceplug/btleplug/tree/master/examples) - Rust BLE examples
