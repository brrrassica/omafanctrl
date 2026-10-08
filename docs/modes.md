# Modes

omafanctrl has three control modes.

## BIOS

The firmware manages the fan. omafanctrl writes `0x80` to the fan-control
register and otherwise stays out of the way. This is the safe default and the
state the watchdog reverts to.

## Manual

A fixed fan level (1-7) is applied. On the E14 Gen 4 only levels 1-3 produce
distinct speeds (1800 / 2200 / 3900 RPM); levels 4-7 saturate at the maximum.

## Smart

The maximum usable sensor temperature is mapped to a fan level using the
`[SmartMode]` thresholds. Sensors that read `0x00` or above 127 °C, and any
sensor in `IgnoreSensors`, are excluded.

### Hysteresis

Hysteresis prevents oscillation around a threshold. Stepping up requires the
temperature to clear the target threshold by its `hystUp`; stepping down
requires it to fall below the current threshold by its `hystDown`. Hysteresis
can be toggled at runtime:

```sh
omafanctrl hysteresis off
omafanctrl hysteresis on
omafanctrl hysteresis toggle
```

or from the GUI Overview page. The setting is persisted as `Hysteresis` in
`[General]`.

### Minimum dwell time

The engine will not change the fan level more often than the minimum dwell time
(default 5 s), except for BIOS reverts and emergency escalations. Explicit user
commands bypass the dwell.

## Safety floor and ceiling

Every commanded level is clamped to the safety floor (default 1) and ceiling
(default 7). When the maximum temperature reaches the emergency threshold
(default 85 °C), the ceiling is forced and the dwell is bypassed.

## Cycling

```sh
omafanctrl mode cycle   # BIOS -> Manual -> Smart -> BIOS
omafanctrl toggle       # alias