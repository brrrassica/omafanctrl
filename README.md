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

## Requirements

- Arch Linux / Omarchy Quattro (4.x)
- The `ec_sys` kernel module with write support:

  ```sh
  sudo modprobe ec_sys write_support=1
  ```

- The D-Bus system-bus policy installed so the daemon may own the well-known
  name `org.omarchy.omafanctrl` (see
  [step 3](#3-install-the-d-bus-policy)). Without it the daemon exits with
  `org.freedesktop.DBus.Error.AccessDenied: Request to own name refused by policy`.

## Running

### 1. Build

```sh
cargo build --release
```

This produces all four binaries in `target/release/`.

### 2. Load the EC module

```sh
sudo modprobe ec_sys write_support=1
```

### 3. Install the D-Bus policy

The daemon owns the well-known name `org.omarchy.omafanctrl` on the **system
bus**. The default system-bus policy denies owning any name, so the shipped
policy must be installed first — otherwise the daemon exits with
`org.freedesktop.DBus.Error.AccessDenied: Request to own name refused by policy`.

```sh
sudo install -Dm644 data/dbus/org.omarchy.omafanctrl.conf \
  /usr/share/dbus-1/system.d/org.omarchy.omafanctrl.conf
sudo systemctl reload dbus
```

### 4. Start the daemon

The daemon is the only component that writes to the EC, so it must run as root
and be started first. It serves `org.omarchy.omafanctrl` on the **system bus** at
`/org/omarchy/omafanctrl`.

```sh
sudo ./target/release/omafanctrld --config data/profiles/e14-gen4.ini
```

Flags:

- `--config <path>` / `-c` — config file (default `/etc/omafanctrl/TPFanControl.ini`)
- `--interval <secs>` — control-loop interval (default `5`)
- `--revert` — hand the fan back to BIOS auto and exit

### 5. Use a client

With the daemon running, the CLI, GUI, and Waybar module talk to it over D-Bus.

```sh
./target/release/omafanctrl status
./target/release/omafanctrl-gui
./target/release/omafanctrl-waybar --show icon,mode,rpm,temp
```

### Install as a system service

Install the system integration files and enable the unit:

| File | Install to |
| --- | --- |
| [`data/dbus/org.omarchy.omafanctrl.conf`](data/dbus/org.omarchy.omafanctrl.conf) | `/usr/share/dbus-1/system.d/` |
| [`data/polkit/org.omarchy.omafanctrl.policy`](data/polkit/org.omarchy.omafanctrl.policy) | `/usr/share/polkit-1/actions/` |
| [`data/systemd/omafanctrld.service`](data/systemd/omafanctrld.service) | `/usr/lib/systemd/system/` |
| [`data/modules-load.d/ec_sys.conf`](data/modules-load.d/ec_sys.conf) | `/etc/modules-load.d/` |
| [`data/modprobe.d/ec_sys.conf`](data/modprobe.d/ec_sys.conf) | `/etc/modprobe.d/` |

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now omafanctrld
```

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
./target/release/omafanctrl-gui
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

## Waybar module

`omafanctrl-waybar` is a Waybar `custom` module that queries the daemon and
prints Waybar JSON (`text`, `tooltip`, `class`, `alt`). The `class` is the
current mode (`bios`, `manual`, `smart`) or `error`, so the bar can be styled
per mode.

```sh
omafanctrl-waybar --show icon,mode,rpm,temp --icon ""   # Waybar JSON
omafanctrl-waybar --show mode,rpm --plain               # plain text
```

`--show` accepts any of `icon`, `mode`, `rpm`, and `temp`.

Add the module to your Waybar config (see
[`data/waybar/omafanctrl.jsonc`](data/waybar/omafanctrl.jsonc)):

```jsonc
"custom/omafanctrl": {
  "exec": "omafanctrl-waybar --show icon,mode,rpm,temp",
  "return-type": "json",
  "interval": 2,
  "on-click": "omafanctrl mode cycle --notify",
  "on-scroll-up": "omafanctrl level up --notify",
  "on-scroll-down": "omafanctrl level down --notify"
}
```

and append the per-mode colours from
[`data/waybar/omafanctrl.css`](data/waybar/omafanctrl.css) to your
`~/.config/waybar/style.css`.

## Safety

`omafanctrl` writes directly to the Embedded Controller. A watchdog always
reverts the fan to BIOS auto control when the daemon stops or crashes.

## Development

Technical design, the D-Bus contract, EC register details, the fan curve, and
the milestone plan live under [`plans/`](plans/README.md).

## License

MIT — see [`LICENSE`](LICENSE).
