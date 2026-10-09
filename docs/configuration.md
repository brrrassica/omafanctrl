# Configuration

omafanctrl reads a TPFanCtrl2-compatible `.ini` file, by default
`/etc/omafanctrl/TPFanControl.ini`. The daemon watches the file and reloads it
when it changes; you can also reload explicitly with `omafanctrl reload`.

## Format

- Keys are case-insensitive.
- Values are trimmed of surrounding whitespace.
- Comments start with `;`, `#`, or `//`, on their own line or inline.
- Sections are written `[Section]`.

Both the flat TPFanCtrl2 layout and explicit sections are accepted.

## Sections

### `[General]`

| Key | Meaning | Default |
| --- | --- | --- |
| `Active` | Active mode: 0 = BIOS, 1 = Smart 1, 2 = Smart 2, 3 = Manual | 2 |
| `Cycle` | Polling interval, seconds | 5 |
| `ManFanSpeed` | Manual fan level (0-7) | 0 |
| `ManModeExit` | Temperature at which Manual mode exits | 78 |
| `MaxReadErrors` | Consecutive read errors tolerated | 10 |
| `IconLevels` | Three ascending temperature thresholds for the tray icon | 65 75 80 |
| `FanBeep` | `(on, off)` beep durations | 0 0 |
| `Hysteresis` | Apply smart-mode hysteresis (1/0) | 1 |

The remaining TPFanCtrl2 display options (`Hotkeys`, `StayOnTop`, `SlimDialog`,
`BluetoothEDR`, `NoBallons`, `NoExtSensor`, `StartMinimized`, `IconCycle`,
`ShowTempIcon`, `ShowBiasedTemps`, `ShowAll`, `IconColorFan`, `Lev64Norm`,
`Log2File`, `Log2csv`, `ProcessPriority`) are parsed and preserved.

### `[Sensors]`

| Key | Meaning |
| --- | --- |
| `IgnoreSensors` | Comma-separated sensor names (or `x<hex>` offsets) to exclude |
| `SensorName<N>` | Display name for sensor sequence `N` |

### `[FanLevels]`

Maps fan-control levels to observed RPM:

```ini
[FanLevels]
Level1=1800
Level2=2200
Level3=3900
```

### `[SmartMode]`

The temperature thresholds. `Level=temp fan [hystUp hystDown]`:

```ini
[SmartMode]
Label=Smart Mode 1/
Level=45 1 0 0
Level=50 2 0 0
Level=60 3 0 0
```

A second mode can be defined in `[SmartMode2]` (or `Level2=` in the flat
layout).

## Example

See [`data/profiles/e14-gen4.ini`](../data/profiles/e14-gen4.ini) for the
verified E14 Gen 4 profile.

## Editing

- CLI: `omafanctrl config get`, `omafanctrl config set new.ini`
- GUI: the Smart Curve and Sensors pages and the Settings dialog write the
  config back through the daemon.

`SetConfig` validates the document before writing, so an invalid configuration
never reaches disk.