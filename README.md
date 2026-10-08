# omafanctrl

A modern Linux fan control suite for the **ThinkPad E14 Gen 4** running
**Omarchy Quattro (4.x)**.

`omafanctrl` is a Linux counterpart to
[TPFanCtrl2](https://github.com/Shuzhengz/TPFanCtrl2). It mirrors TPFanCtrl2's
internal behavior — direct Embedded Controller (EC) access, BIOS/Manual/Smart
modes, and `.ini` configuration — while providing a beautiful, minimalistic,
adaptive interface that feels native under Hyprland tiling.

## Components

| Component | Description |
| --- | --- |
| `omafanctrld` | Privileged daemon (systemd system service) that owns the EC and exposes a D-Bus API |
| `omafanctrl-gui` | GTK4 + libadwaita desktop app (adaptive, Wayland-native) |
| `omafanctrl` | CLI client for scripting and Hyprland `SUPER + <key>` hotkeys |
| Waybar module | Renders fan state in the Omarchy Quattro bar |

## Status

Early development. The project is being built milestone by milestone;
Contributions are currently NOT open.

## Building

```sh
cargo build --release
```

## Requirements

- Arch Linux / Omarchy Quattro (4.x)
- The `ec_sys` kernel module with write support:

  ```sh
  modprobe ec_sys write_support=1
  ```

## Probing the EC

Before trusting the register map, inspect the live EC with the read-only probe:

```sh
sudo cargo run -p omafanctrl-core --bin omafanctrl-probe
```

It dumps all 256 register bytes, decodes the known fan and temperature registers,
and reports the realtime fan speed from `/proc/acpi/ibm/fan` (`thinkpad_acpi`)
alongside the EC tachometer. Writing requires an explicit `--force` flag.

To re-measure the fan curve on real hardware, sweep every manual level:

```sh
sudo cargo run -p omafanctrl-core --bin omafanctrl-probe -- --sweep --force
```

The sweep records the RPM at each level, compares it against the known curve, and
always restores BIOS auto control when it finishes.

## Daemon and D-Bus

`omafanctrld` is the only component that writes to the EC. It runs the control
loop and serves the `org.omarchy.omafanctrl` interface on the **system bus** at
`/org/omarchy/omafanctrl`:

| Method | Purpose |
| --- | --- |
| `GetState` | A snapshot (mode, levels, RPM, temperatures) |
| `SetMode` | Switch between `bios`, `manual`, and `smart` |
| `SetManualLevel` | Set the manual fan level |
| `SetHysteresis` | Enable or disable smart-mode hysteresis |
| `GetConfig` / `SetConfig` | Read or replace the `.ini` configuration |
| `ReloadConfig` | Re-read the configuration file from disk |

It also emits the `StateChanged` and `ConfigChanged` signals.

Run it directly (as root, with `ec_sys write_support=1` loaded):

```sh
sudo ./target/debug/omafanctrld --config data/profiles/e14-gen4.ini
```

### System integration files

| File | Install to |
| --- | --- |
| [`data/dbus/org.omarchy.omafanctrl.conf`](data/dbus/org.omarchy.omafanctrl.conf) | `/usr/share/dbus-1/system.d/` |
| [`data/polkit/org.omarchy.omafanctrl.policy`](data/polkit/org.omarchy.omafanctrl.policy) | `/usr/share/polkit-1/actions/` |
| [`data/systemd/omafanctrld.service`](data/systemd/omafanctrld.service) | `/usr/lib/systemd/system/` |
| [`data/modules-load.d/ec_sys.conf`](data/modules-load.d/ec_sys.conf) | `/etc/modules-load.d/` |
| [`data/modprobe.d/ec_sys.conf`](data/modprobe.d/ec_sys.conf) | `/etc/modprobe.d/` |

The systemd unit's `ExecStopPost=/usr/bin/omafanctrld --revert` is a safety net
that hands the fan back to the firmware even after `kill -9`, when the in-process
watchdog cannot run.

## CLI

`omafanctrl` is a scriptable D-Bus client. Every call has a timeout, so a
keybinding never blocks the compositor.

```sh
omafanctrl status                 # human-readable state
omafanctrl status --json          # machine-readable state
omafanctrl mode smart             # set the mode (bios | manual | smart)
omafanctrl mode cycle             # BIOS -> Manual -> Smart
omafanctrl toggle                 # alias for `mode cycle`
omafanctrl level set 3            # set the manual level (1-7)
omafanctrl level up               # step the level up (switches to Manual)
omafanctrl level down             # step the level down
omafanctrl config get             # print the current .ini
omafanctrl config set new.ini     # validate, persist, and apply (`-` for stdin)
omafanctrl reload                 # re-read the config file from disk
omafanctrl hysteresis toggle      # enable/disable smart-mode hysteresis
```

Add `--json` for machine-readable output and `--notify` to raise a desktop
notification after a change.

## Hyprland hotkeys (Omarchy Quattro 4.x)

Omarchy Quattro configures Hyprland through **Lua**. Personal keybinding
overrides live in `~/.config/hypr/bindings.lua`, which is loaded by
`~/.config/hypr/hyprland.lua` via `require("hypr.bindings")`.

Append the ready-to-paste snippet from
[`data/hyprland/bindings.lua`](data/hyprland/bindings.lua) to your
`~/.config/hypr/bindings.lua` (do not replace the file — it may already contain
your own overrides):

```lua
o.bind("SUPER + F1", "Fan: cycle mode", "omafanctrl mode cycle --notify")
o.bind("SUPER + SHIFT + F1", "Fan: toggle mode", "omafanctrl toggle --notify")
o.bind("SUPER + F2", "Fan: level up", "omafanctrl level up --notify")
o.bind("SUPER + SHIFT + F2", "Fan: level down", "omafanctrl level down --notify")
o.bind("SUPER + F3", "Fan: status", "omafanctrl status --notify")
```

`o.bind(keys, description, command)` is the Omarchy helper; the description is
shown by `omarchy menu keybindings --print`.

## GUI

`omafanctrl-gui` is a GTK4 + libadwaita desktop app. It uses an adaptive
`AdwNavigationSplitView` shell (sidebar + content) that collapses to a single
column at narrow tiling widths, and follows the system light/dark theme.

```sh
./target/debug/omafanctrl-gui
```

Pages:

- **Overview** — live temperatures, RPM, the mode switch, manual level, and a
  Cairo temperature-history chart.
- **Smart Curve** — a draggable curve editor plus precise threshold spin rows.
- **Sensors** — enable/disable (ignore), rename, and inspect each sensor.
- **Settings** — cycle interval, start behaviour, config path, and reload.

Live data is pushed from the daemon's `StateChanged`/`ConfigChanged` signals; if
the daemon is unreachable the app shows an `AdwStatusPage` and surfaces errors as
`AdwToast` notifications.

## Fan curve

The E14 Gen 4 fan is not linear in the EC fan-control level. The observed curve
(see [`config/E14G4-quirks`](config/E14G4-quirks)) is:

| Level | RPM |
| --- | --- |
| 1 | 1800 |
| 2 | 2200 |
| 3–7 | 3900 |

Only levels 1–3 produce distinct speeds; levels 4–7 saturate at the maximum. The
curve is encoded in `omafanctrl-core::fan_curve` and the shipped profile
([`data/profiles/e14-gen4.ini`](data/profiles/e14-gen4.ini)) uses levels 1–3.

## Safety

`omafanctrl` writes directly to the Embedded Controller. A watchdog always
reverts the fan to BIOS auto control when the daemon stops or crashes.

## License

MIT — see [`LICENSE`](LICENSE).
