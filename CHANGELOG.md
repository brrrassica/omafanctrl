# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.1.6] - 2026-11-20

### Fixed

- **Smart-mode downward hysteresis honours unlisted committed levels.** A
  committed level without a `smart_levels` entry (a manually commanded level
  carried across a Manual → Smart transition, or a level clamped by the safety
  floor/ceiling) made the step-down lookup fail and silently skipped the
  configured `hyst_down` hold; the dwell now falls back to the closest lower
  table entry.
  ([#78](https://github.com/brrrassica/omafanctrl/issues/78),
  [#82](https://github.com/brrrassica/omafanctrl/issues/82))
- **Entering Smart mode clears the committed level.** Switching Manual → Smart
  re-syncs hysteresis from the threshold table instead of holding anti-drop
  state against a level absent from it.
  ([#82](https://github.com/brrrassica/omafanctrl/issues/82))
- **Validation rejects `ManModeExit=127`.** The exit check is strict and
  readings are capped at `TEMP_MAX_C`, so a threshold at the ceiling could
  never trigger and silently disabled the Manual-mode safety exit; the accepted
  range is now `0..=126`.
  ([#77](https://github.com/brrrassica/omafanctrl/issues/77))
- **Validation rejects `ManFanSpeed=0`.** The EC rejects level `0x00` and the
  engine silently substituted the safety floor, diverging from the configured
  value; `ManFanSpeed` must now be `1..=7` and the shipped profiles set `3`.
  ([#85](https://github.com/brrrassica/omafanctrl/issues/85))
- **Telemetry no longer consumes the `MaxReadErrors` budget.** The 4 Hz
  telemetry sample and the control tick had shared one consecutive-error
  counter, so at the default cadence a burst of EC read failures aborted the
  daemon after ~2.5 s instead of the documented 10 control cycles — and any
  successful telemetry sample masked a control path failing every cycle.
  ([#74](https://github.com/brrrassica/omafanctrl/issues/74))
- **A stalled fan is flagged instead of reported healthy.** A tachometer read
  of 0 RPM while the engine commands a nonzero fan level is now treated as a
  failed read: the last known RPM is kept for telemetry and the iteration
  counts toward `MaxReadErrors`. A genuine 0 RPM in BIOS-auto or idle is still
  accepted. ([#71](https://github.com/brrrassica/omafanctrl/issues/71))
- **Implausible sensor reads count toward the error budget.** Sensors stuck at
  zero or returning out-of-range temperatures were silently replaced by the
  last cached value and reset the error streak, letting control run on frozen
  temperatures; they are now reported as per-iteration errors and partial
  failures mark the whole iteration. ([#66](https://github.com/brrrassica/omafanctrl/issues/66))
- **Runtime mode and manual level persist consistently.** `SetMode` and
  `SetManualLevel` persist their own changes (with watcher sync), the thermal
  safety manual-exit persists the new mode, and cycling BIOS → Manual → Smart
  preserves the selected Smart slot instead of reverting to Smart 1.
  ([#70](https://github.com/brrrassica/omafanctrl/issues/70),
  [#67](https://github.com/brrrassica/omafanctrl/issues/67),
  [#69](https://github.com/brrrassica/omafanctrl/issues/69))
- **The curve page edits the Smart slot the engine actually drives.**
  ([#75](https://github.com/brrrassica/omafanctrl/issues/75))
- **Threshold-less Smart configurations are rejected, and reload honours
  `Active`.** A config with `Active=1/2` but no `[SmartMode]` sections is now
  invalid, the config-load fallback fails safe to BIOS, and a reloaded
  configuration re-applies the mode from `[General] Active`.
  ([#73](https://github.com/brrrassica/omafanctrl/issues/73),
  [#59](https://github.com/brrrassica/omafanctrl/issues/59))

## [1.1.5] - 2026-11-18

### Fixed

- **Persisting a runtime setting no longer resets the live manual fan level.**
  `persist_config` now records the exact text it wrote in `ConfigWatcher`, so the
  next control tick does not mistake the daemon's own write for an external edit
  and reload it. Previously, toggling hysteresis right after `SetManualLevel`
  could drop the fan back to the `ManFanSpeed` value captured before the write.
- **`ManModeExit` is validated against the 127 °C sensor ceiling.** A value above
  it could never be reached, silently disabling the documented Manual-to-Smart
  safety exit; the loader now rejects it with `ManModeExitOutOfRange`.
- **`scripts/profile-daemon.sh` reports context switches and RSS again.** The
  `/proc/<pid>/status` lookup now matches the colon-terminated keys
  (`voluntary_ctxt_switches:`, `VmRSS:`), so the profile no longer prints blank
  or zero values.

## [1.1.4] - 2026-11-04

### Fixed

- **A single EC read error no longer terminates the daemon.** The control loop
  and the 4 Hz telemetry sample now survive transient Embedded Controller read
  failures: a failed sensor offset is skipped and filled from the last good
  sample, a failed tachometer read keeps the last known RPM, and
  `[General] MaxReadErrors` is finally consulted. Only after that many
  consecutive failed iterations does the daemon exit so systemd can restart it
  and the watchdog can revert the fan.
- **Hysteresis toggled at runtime is persisted.** `SetHysteresis` (D-Bus, CLI,
  or GUI) now updates `[General] Hysteresis` in the live configuration and
  writes it back, instead of only flipping the in-memory engine policy and
  having the next reload or restart silently restore the file value.
- **`Cycle` is honored.** The daemon now uses `[General] Cycle` as its
  control-loop cadence and re-reads it on configuration reload; `--interval`
  overrides it when given.
- **Sensors page honors hex-offset `IgnoreSensors` entries.** Ignore matching
  now uses the engine's name-or-`x<hex>` semantics, and re-enabling a sensor
  removes both forms, so a sensor ignored by offset can be enabled again.
- **Re-running the installer restarts an active daemon.** The installer now
  runs `systemctl restart omafanctrld` after installing the new binaries, so an
  upgrade replaces the running process instead of leaving the old binary alive.

## [1.1.3] - 2026-11-03

### Added

- **Lifecycle scripts.** The installer now lives at
  [`scripts/install.sh`](scripts/install.sh), alongside three new scripts:
  [`scripts/uninstall.sh`](scripts/uninstall.sh) (reverses the install, keeping
  `/etc/omafanctrl` unless `--purge` is given),
  [`scripts/restart-all.sh`](scripts/restart-all.sh) (reloads `ec_sys`, restarts
  the daemon, and reloads the omarchy-shell bar module), and
  [`scripts/doctor.sh`](scripts/doctor.sh) (a read-only PASS/WARN/FAIL
  diagnostic of the platform, module, files, and daemon).

### Changed

- **Installer relocated.** `install.sh` moved from the repository root to
  `scripts/install.sh`, and the release bundle now ships it under `scripts/`, so
  the install command is `./scripts/install.sh`.

### Fixed

- **Polkit authorization is enforced.** Every mutating D-Bus method
  (`SetMode`, `SetManualLevel`, `SetHysteresis`, `SetConfig`, `ReloadConfig`)
  now calls `CheckAuthorization` for `org.omarchy.omafanctrl.control` and
  returns `AccessDenied` when the caller is not authorized.
- **Sensors page reflects the configuration.** Switch states and display names
  are rebuilt when the configuration arrives or changes, instead of being
  frozen from the initial State-before-Config update.
- **Failed explicit reloads keep fan control running.** An unreadable or
  invalid configuration no longer terminates the control loop; the last good
  configuration is kept, and a genuinely fatal loop error now exits the daemon
  non-zero so systemd can restart it.

[1.1.6]: https://github.com/brrrassica/omafanctrl/releases/tag/v1.1.6
[1.1.5]: https://github.com/brrrassica/omafanctrl/releases/tag/v1.1.5
[1.1.4]: https://github.com/brrrassica/omafanctrl/releases/tag/v1.1.4
[1.1.3]: https://github.com/omafanctrl/omafanctrl/releases/tag/v1.1.3

## [1.1.2] - 2026-11-01

### Fixed

- **Manual mode honours `ManFanSpeed`.** Starting with `Active=3` now applies
  the configured manual level instead of the safety floor, and reloading a
  configuration updates the engine's manual level as well.

[1.1.2]: https://github.com/omafanctrl/omafanctrl/releases/tag/v1.1.2

## [1.1.1] - 2026-10-10

### Added

- `omafanctrl-status --show cpu` renders the CPU sensor specifically, rather
  than the hottest sensor across the board.

### Changed

- **Bar module relocated.** The `omafanctrl` status module now sits in the
  `center` section, immediately left of the `omarchy.indicators` hidden controls
  and the `omarchy.clock` anchor, instead of the `right` section. The installer
  inserts it at the front of `bar.layout.center` and moves it there if it is
  already in another section.
- **Bar module content.** The module now shows the CPU temperature followed by
  the fan speed (`--show cpu,rpm`, for example `56°C 3900 RPM`) instead of the
  mode/RPM/max-temperature mix, and its tooltip lists only the CPU sensor rather
  than every EC sensor the daemon exposes (including the profile's ignored ones).

[1.1.1]: https://github.com/omafanctrl/omafanctrl/releases/tag/v1.1.1

## [1.1.0] - 2026-10-09

### Changed

- **Bar integration migrated to omarchy-shell.** Omarchy Quattro (4.x) replaced
  Waybar with `omarchy-shell` (Quickshell), so the Waybar module never appeared.
  The status module is now a `type: "command"` entry in
  `~/.config/omarchy/shell.json` under `bar.layout.<section>`, and the installer
  merges it idempotently (seeding the file from the Omarchy defaults). The
  `data/waybar/` config and CSS were removed.
- **Binary renamed** `omafanctrl-waybar` → `omafanctrl-status`. It still emits
  the "Waybar-style JSON" contract (`text`, `tooltip`, `class`, `alt`) that the
  omarchy-shell command module parses, plus `--plain` for debugging.
- **Interactions:** left click cycles the mode, right click toggles, middle click
  shows a status notification. Custom command modules have no scroll handlers, so
  `level up`/`level down` remain CLI and Hyprland-hotkey actions.
- **Packaging:** the AUR packages and the AppImage now install
  `omafanctrl-status` and ship the `data/omarchy-shell/` helper.

[1.1.0]: https://github.com/omafanctrl/omafanctrl/releases/tag/v1.1.0

## [1.0.2] - 2026-10-09

### Changed

- **GUI redesign**: the app now uses an adaptive `AdwMultiLayoutView` dashboard
  with four layouts (`desktop`, `half`, `quarter`, `eighth`) selected by
  `AdwBreakpoint` width conditions, so it reflows cleanly under Hyprland's
  dwindle tiling. Graphs sit on top with status and controls below in every
  layout, and the narrowest layout keeps all content in a single scrollable
  column.
- **GUI shell**: navigation is now an `AdwViewSwitcher` tab strip below the
  header bar (Dashboard, Smart Curve, Sensors) instead of a sidebar, and
  Settings moved behind a clearly iconed and labeled header menu button that
  opens an `AdwPreferencesDialog`.
- **GUI sensors**: the Sensors page is flat — one row per sensor with an enable
  switch and an editable display name — and both tabs show human-readable
  sensor labels (for example `CPU Temperature`, or `Register 0x7D` for
  undocumented registers).
- **GUI dashboard**: a "Show hidden" toggle reveals sensors excluded by
  `IgnoreSensors`.
- **Dependency**: the `libadwaita` feature was bumped from `v1_5` to `v1_7` for
  `AdwMultiLayoutView` and `AdwViewStack`/`AdwViewSwitcher`.

[1.0.2]: https://github.com/omafanctrl/omafanctrl/releases/tag/v1.0.2

## [1.0.1] - 2026-10-08

### Added

- **One-shot installer** ([`install.sh`](install.sh)): a single script that
  installs every component and dependency on Omarchy Quattro (4.x) on a
  ThinkPad, loads the `ec_sys` module, enables the daemon, and wires up the
  Hyprland and Waybar integrations. It refuses to run on any other distribution
  or hardware.
- **Release bundle** (`omafanctrl.zip`): CI now builds the release binaries and
  packages them together with `install.sh` and the system files, so a release
  can be installed with a single command.

### Changed

- CI builds and publishes `omafanctrl.zip` as a build artifact and attaches it
  to tagged GitHub releases. Source archives are provided by GitHub itself.

[1.0.1]: https://github.com/omafanctrl/omafanctrl/releases/tag/v1.0.1

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
