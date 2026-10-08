# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - 2026-10-08

The first stable release of omafanctrl, a modern fan control suite for the
ThinkPad E14 Gen 4 on Omarchy Quattro (4.x).

### Added

- **EC access layer** (`omafanctrl-core::ec`): bounds-checked reads and writes
  to the `ec_sys` register window, the verified E14 Gen 4 register map, the
  TPFanCtrl2 fan-switch handshake, read-back verification, rate limiting, and
  temperature sanity checks.
- **Configuration system** (`omafanctrl-core::config`): a TPFanCtrl2 `.ini`
  parser with case-insensitive keys, `;`/`#`/`//` comments, typed
  `[General]`/`[Sensors]`/`[FanLevels]`/`[SmartMode]` sections, validation with
  actionable errors, lossless round-tripping, and a content-fingerprint watcher.
- **Control engine** (`omafanctrl-core::engine`): BIOS / Manual / Smart modes,
  optional runtime-toggleable hysteresis, a minimum-dwell timer, ignore lists,
  out-of-range filtering, safety floor/ceiling clamping, an emergency ceiling,
  and a watchdog that reverts to BIOS auto on every exit path.
- **Daemon** (`omafanctrld`): a privileged systemd service that owns the EC and
  serves the `org.omarchy.omafanctrl` D-Bus interface on the system bus, with
  `GetState`, `SetMode`, `SetManualLevel`, `SetHysteresis`, `GetConfig`,
  `SetConfig`, `ReloadConfig`, and the `StateChanged`/`ConfigChanged` signals.
- **CLI** (`omafanctrl`): a scriptable D-Bus client with `status`, `mode`,
  `level`, `toggle`, `config get|set`, `reload`, and `hysteresis`, plus `--json`
  and `--notify` output modes.
- **GUI** (`omafanctrl-gui`): a GTK4 + libadwaita app with an adaptive
  `AdwNavigationSplitView` shell, live Overview, a draggable Smart Curve editor,
  a Sensors page, a Settings page, and a Cairo temperature-history chart.
- **Waybar module** (`omafanctrl-waybar`): a `custom` module emitting Waybar
  JSON with per-mode styling.
- **Hyprland hotkeys**: a ready-to-paste `bindings.lua` snippet for Omarchy
  Quattro.
- **Packaging**: an AUR `PKGBUILD` (stable and `-git`) and an AppImage build
  script.
- **Documentation**: install, configuration, modes, safety, troubleshooting,
  testing, EC write audit, and profiling guides.

### Safety

- Only the fan-control (`0x2F`) and fan-switch (`0x31`) registers are ever
  written; all other writes are rejected.
- The watchdog reverts the fan to BIOS auto on clean exit, `SIGTERM`, panic, and
  (via the systemd `ExecStopPost` safety net) `kill -9`.
- The `.ini` parser is fuzzed and covered by a deterministic round-trip test.

[1.0.0]: https://github.com/omafanctrl/omafanctrl/releases/tag/v1.0.0
