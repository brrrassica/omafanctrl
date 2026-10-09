# omafanctrl

A modern fan-control suite for the **ThinkPad E14 Gen 4** on **Omarchy Quattro
(4.x)**. It is a Linux counterpart to
[TPFanCtrl2](https://github.com/Shuzhengz/TPFanCtrl2): direct Embedded
Controller (EC) access, BIOS/Manual/Smart modes, and `.ini` configuration, with
a native GTK4 + libadwaita app, a scriptable CLI, and a Waybar module.

## Install

On a ThinkPad running Omarchy Quattro (4.x), the release bundle installs
everything — dependencies, binaries, the daemon, and the desktop integrations —
in one step:

```sh
curl -LO https://github.com/brrrassica/omafanctrl/releases/latest/download/omafanctrl.zip
unzip omafanctrl.zip
cd omafanctrl-1.0.2
./install.sh
```

[`install.sh`](install.sh) re-execs itself with `sudo`, verifies the platform,
and refuses to run on anything other than Omarchy 4 on a ThinkPad.

Other options: the AUR (`yay -S omafanctrl`) or a manual build — see
[docs/install.md](docs/install.md). The latest release is
[`v1.0.2`](https://github.com/brrrassica/omafanctrl/releases/tag/v1.0.2); the
change history is in [CHANGELOG.md](CHANGELOG.md).

## Components

| Component | Description |
| --- | --- |
| `omafanctrld` | Privileged daemon (systemd service) that owns the EC and exposes a D-Bus API |
| `omafanctrl-gui` | GTK4 + libadwaita desktop app (adaptive, Wayland-native) |
| `omafanctrl` | CLI client for scripting and Hyprland hotkeys |
| `omafanctrl-waybar` | Waybar module that renders fan state in the bar |

## Requirements

- Arch Linux / Omarchy Quattro (4.x)
- The `ec_sys` kernel module with write support:

  ```sh
  sudo modprobe ec_sys write_support=1
  ```

- The D-Bus system-bus policy installed so the daemon may own
  `org.omarchy.omafanctrl`. The installer handles this; for a manual setup see
  [docs/install.md](docs/install.md).

## Usage

### GUI

```sh
omafanctrl-gui
```

The app is an adaptive dashboard that reflows for Hyprland's dwindle tiling
(full, half, quarter, and 1/8 screen): graphs sit on top, with status and
controls below. A tab strip switches between:

- **Dashboard** — live temperatures, RPM, the mode switch, manual level, and
  temperature/fan-speed history charts.
- **Smart Curve** — a draggable curve editor plus precise threshold editors.
- **Sensors** — enable/disable and rename each sensor.

Settings (cycle interval, start behaviour, config path, reload) live behind the
header menu button.

### CLI

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

### Hyprland hotkeys

Append the snippet from
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

### Waybar

Add the module from
[`data/waybar/omafanctrl.jsonc`](data/waybar/omafanctrl.jsonc) to your Waybar
config and append the colours from
[`data/waybar/omafanctrl.css`](data/waybar/omafanctrl.css) to
`~/.config/waybar/style.css`:

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

## Manual install

Build the workspace and install the system files:

```sh
cargo build --release

sudo install -Dm644 data/dbus/org.omarchy.omafanctrl.conf \
  /usr/share/dbus-1/system.d/org.omarchy.omafanctrl.conf
sudo install -Dm644 data/polkit/org.omarchy.omafanctrl.policy \
  /usr/share/polkit-1/actions/org.omarchy.omafanctrl.policy
sudo install -Dm644 data/systemd/omafanctrld.service \
  /usr/lib/systemd/system/omafanctrld.service

sudo systemctl daemon-reload
sudo systemctl enable --now omafanctrld
```

The daemon is the only component that writes to the EC, so it runs as root. Run
it directly with `sudo omafanctrld --config <path>`; `--interval <secs>` sets
the control-loop cadence and `--revert` hands the fan back to BIOS auto and
exits. See [docs/install.md](docs/install.md) for the full manual and AppImage
paths.

## Safety

`omafanctrl` writes directly to the Embedded Controller. A watchdog always
reverts the fan to BIOS auto control when the daemon stops or crashes. See
[docs/safety.md](docs/safety.md) for the full list of protections.

## Help wanted

`omafanctrl` is developed and tested on a single machine: a **ThinkPad E14 Gen
4** running **Omarchy Quattro (4.x)**. The EC register map, fan curve, and
safety limits are verified against that one device, so behaviour on other
ThinkPads is unproven.

If you have a different ThinkPad running Omarchy Quattro, your help would be
invaluable:

- **Probe your EC** with the read-only tool and share the output:

  ```sh
  sudo cargo run -p omafanctrl-core --bin omafanctrl-probe
  ```

- **Report your model** (and the probe output) in an issue so the register map
  and fan curve can be extended to your hardware.
- **Test the GUI** at each Hyprland dwindle size and report any layout or
  rendering problems.

Please do not run the write path (`--force`, the daemon, or manual fan levels)
on untested hardware until the register map has been confirmed for your model.

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
  binaries, [`install.sh`](install.sh), and the system files.
- **AUR** — [`packaging/aur/omafanctrl/PKGBUILD`](packaging/aur/omafanctrl/PKGBUILD)
  (stable) and [`packaging/aur/PKGBUILD`](packaging/aur/PKGBUILD) (`-git`).
- **AppImage** — [`packaging/appimage/build.sh`](packaging/appimage/build.sh)
  bundles the GUI and CLI clients; the daemon must be installed separately.

## License

MIT — see [`LICENSE`](LICENSE).
