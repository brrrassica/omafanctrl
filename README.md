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

It dumps all 256 register bytes and decodes the known fan and temperature
registers. Writing requires an explicit `--force` flag.

## Safety

`omafanctrl` writes directly to the Embedded Controller. A watchdog always
reverts the fan to BIOS auto control when the daemon stops or crashes.

## License

MIT — see [`LICENSE`](LICENSE).
