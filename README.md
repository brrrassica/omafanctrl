# omafanctrl

A modern Linux fan control suite for the **ThinkPad E14 Gen 4** running
**Omarchy Quattro (4.x)**.

`omafanctrl` is a Linux counterpart to
[TPFanCtrl2](https://github.com/Shuzhengz/TPFanCtrl2). It mirrors TPFanCtrl2's
internal behavior — direct Embedded Controller (EC) access, BIOS/Manual/Smart
modes, and `.ini` configuration — while providing a beautiful, minimalistic,
adaptive interface that feels native under Hyprland tiling.

## Releases

The latest stable release is
[`v1.0.1`](https://github.com/brrrassica/omafanctrl/releases/tag/v1.0.1).
Install it with the one-shot installer from the release bundle
(`omafanctrl.zip`), from the AUR (`yay -S omafanctrl`), or manually — see
[docs/install.md](docs/install.md). The change history is in
[CHANGELOG.md](CHANGELOG.md).

## Quick install

On a ThinkPad running Omarchy Quattro (4.x), the release bundle installs
everything — dependencies, binaries, the daemon, and the desktop integrations —
in one step:

```sh
curl -LO https://github.com/brrrassica/omafanctrl/releases/latest/download/omafanctrl.zip
unzip omafanctrl.zip
cd omafanctrl-1.0.1
./install.sh
```

[`install.sh`](install.sh) re-execs itself with `sudo`, verifies the platform,
and refuses to run on anything other than Omarchy 4 on a ThinkPad. See
[docs/install.md](docs/install.md) for the manual and AUR paths.

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

`omafanctrl-gui` is a GTK4 + libadwaita desktop app. Its primary view is an
adaptive `AdwMultiLayoutView` dashboard that reflows for Hyprland's dwindle
tiling: four layouts (`desktop`, `half`, `quarter`, `eighth`) are selected by
`AdwBreakpoint` width conditions. In every layout the graphs sit on top, with
status and controls below them, and the narrowest layout keeps all content in a
single scrollable column. The app follows the system light/dark theme.

```sh
./target/release/omafanctrl-gui
```

The shell is an `AdwViewSwitcher` tab strip directly below the header bar
(Dashboard, Smart Curve, Sensors) that collapses to icons only at narrow
widths. Settings live behind a clearly iconed and labeled header menu button
that opens an `AdwPreferencesDialog`.

- **Dashboard** — live temperatures, RPM, the mode switch, manual level, and
  Cairo temperature- and fan-speed-history charts.
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

## Documentation

- [Install](docs/install.md) — AUR, manual, and AppImage
- [Configuration](docs/configuration.md) — the `.ini` format and sections
- [Modes](docs/modes.md) — BIOS, Manual, Smart, hysteresis, safety limits
- [Safety](docs/safety.md) — what is protected and the watchdog
- [Troubleshooting](docs/troubleshooting.md) — ec_sys, debugfs, permissions
- [Testing](docs/testing.md) — automated tests, fuzzing, and the hardware checklist
- [EC write audit](docs/ec-write-audit.md) — the safety invariants on every EC write
- [Profiling](docs/profiling.md) — idle CPU wakeups and cost

## Packaging

- **Release bundle** — `omafanctrl.zip` (built by CI) contains the prebuilt
  binaries, [`install.sh`](install.sh), and the system files. See
  [docs/install.md](docs/install.md#one-shot-install-recommended).
- **AUR** — the stable [`packaging/aur/omafanctrl/PKGBUILD`](packaging/aur/omafanctrl/PKGBUILD)
  and the [`omafanctrl-git`](packaging/aur/PKGBUILD) package both build the whole
  workspace and install the daemon, CLI, Waybar module, GUI, and all system
  files. See [`packaging/aur/README.md`](packaging/aur/README.md) for the publish
  process.
- **AppImage** — [`packaging/appimage/build.sh`](packaging/appimage/build.sh)
  bundles the GUI and CLI clients. The privileged daemon must be installed
  separately; see the
  [AppImage caveat](docs/install.md#daemon-caveat-for-appimage-users).

## Safety

`omafanctrl` writes directly to the Embedded Controller. A watchdog always
reverts the fan to BIOS auto control when the daemon stops or crashes. See
[docs/safety.md](docs/safety.md) for the full list of protections.

## Help wanted

`omafanctrl` is developed and tested on a single machine: a **ThinkPad E14 Gen 4**
running **Omarchy Quattro (4.x)**. The EC register map, the fan curve, and the
safety limits are all verified against that one device, so behaviour on other
ThinkPads is currently unproven.

If you have a different ThinkPad running Omarchy Quattro, your help would be
invaluable:

- **Probe your EC** with the read-only tool and share the output:

  ```sh
  sudo cargo run -p omafanctrl-core --bin omafanctrl-probe
  ```

- **Report your model** (and the probe output) in an issue so the register map
  and fan curve can be extended to your hardware.
- **Test the GUI** at each Hyprland dwindle size (full, half, quarter, 1/8) and
  report any layout or rendering problems.

Please do not run the write path (`--force`, the daemon, or manual fan levels)
on untested hardware until the register map has been confirmed for your model.

## License

MIT — see [`LICENSE`](LICENSE).
