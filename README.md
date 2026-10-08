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
