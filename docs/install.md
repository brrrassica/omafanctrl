# Installing omafanctrl

omafanctrl has four components:

| Component | Role |
| --- | --- |
| `omafanctrld` | Privileged daemon (systemd system service) that owns the EC |
| `omafanctrl` | CLI client |
| `omafanctrl-waybar` | Waybar module |
| `omafanctrl-gui` | GTK4 + libadwaita desktop app |

The daemon is the only component that writes to the Embedded Controller, so it
must run as root. The clients talk to it over the D-Bus **system bus**.

## Requirements

- Arch Linux / Omarchy Quattro (4.x)
- A ThinkPad (the shipped profile targets the E14 Gen 4)
- The `ec_sys` kernel module with write support
- GTK4 and libadwaita (for the GUI)

## One-shot install (recommended)

On a ThinkPad running Omarchy Quattro (4.x), the release bundle installs every
component and dependency in one step. Download `omafanctrl.zip` from the
[latest release](https://github.com/brrrassica/omafanctrl/releases/latest),
extract it, and run the installer:

```sh
curl -LO https://github.com/brrrassica/omafanctrl/releases/latest/download/omafanctrl.zip
unzip omafanctrl.zip
cd omafanctrl-1.0.1
./install.sh
```

The installer:

- re-execs itself with `sudo`;
- verifies the platform and **refuses to run** on anything other than Omarchy 4
  on a ThinkPad;
- installs the runtime dependencies (`gtk4`, `libadwaita`, `dbus`, `polkit`) and,
  when no prebuilt binaries are present, the build dependencies
  (`base-devel`, `rust`, `pkgconf`, `git`);
- installs the daemon, CLI, Waybar module, GUI, and probe binaries;
- installs the D-Bus policy, polkit action, systemd unit, `ec_sys` drop-ins,
  desktop entry, icon, and the default `/etc/omafanctrl/TPFanControl.ini`;
- loads `ec_sys` with `write_support=1` and enables `omafanctrld`;
- appends the Hyprland keybindings and the Waybar module/styling for the
  invoking user.

It is idempotent, so re-running it upgrades an existing install in place. Pass
`--no-user-setup` to skip the per-user Hyprland/Waybar wiring.

## From a release tarball (source)

Every release is published on GitHub with a binary bundle (`omafanctrl.zip`) and
GitHub's auto-generated source archives. To build the latest release (`v1.0.1`)
from source:

```sh
curl -LO https://github.com/brrrassica/omafanctrl/archive/refs/tags/v1.0.1.tar.gz
tar xf v1.0.1.tar.gz
cd omafanctrl-1.0.1
cargo build --release --locked
```

Then install the binaries and system files as in
[Manual install](#manual-install). The release notes and the full change history
are in [`CHANGELOG.md`](../CHANGELOG.md).

## From the AUR

Two packages are available:

| Package | Tracks | Install |
| --- | --- | --- |
| `omafanctrl` | The latest tagged release | `yay -S omafanctrl` |
| `omafanctrl-git` | The `master` branch | `yay -S omafanctrl-git` |

Both install the binaries, the D-Bus policy, the polkit action, the systemd
unit, the `ec_sys` drop-ins, the desktop entry, and a default configuration at
`/etc/omafanctrl/TPFanControl.ini`.

Enable the daemon:

```sh
sudo systemctl enable --now omafanctrld
```

## Manual install

```sh
cargo build --release

sudo install -Dm755 target/release/omafanctrld /usr/bin/omafanctrld
sudo install -Dm755 target/release/omafanctrl /usr/bin/omafanctrl
sudo install -Dm755 target/release/omafanctrl-waybar /usr/bin/omafanctrl-waybar
sudo install -Dm755 target/release/omafanctrl-gui /usr/bin/omafanctrl-gui

sudo install -Dm644 data/dbus/org.omarchy.omafanctrl.conf \
  /usr/share/dbus-1/system.d/org.omarchy.omafanctrl.conf
sudo install -Dm644 data/polkit/org.omarchy.omafanctrl.policy \
  /usr/share/polkit-1/actions/org.omarchy.omafanctrl.policy
sudo install -Dm644 data/systemd/omafanctrld.service \
  /usr/lib/systemd/system/omafanctrld.service
sudo install -Dm644 data/modules-load.d/ec_sys.conf \
  /usr/lib/modules-load.d/ec_sys.conf
sudo install -Dm644 data/modprobe.d/ec_sys.conf \
  /usr/lib/modprobe.d/ec_sys.conf
sudo install -Dm644 data/desktop/org.omarchy.omafanctrl.desktop \
  /usr/share/applications/org.omarchy.omafanctrl.desktop
sudo install -Dm644 data/icons/org.omarchy.omafanctrl.svg \
  /usr/share/icons/hicolor/scalable/apps/org.omarchy.omafanctrl.svg
sudo install -Dm644 data/profiles/e14-gen4.ini \
  /etc/omafanctrl/TPFanControl.ini

sudo systemctl daemon-reload
sudo systemctl enable --now omafanctrld
```

## AppImage

The AppImage bundles the **clients only** (GUI, CLI, and Waybar module):

```sh
packaging/appimage/build.sh
./omafanctrl-*.AppImage
```

### Daemon caveat for AppImage users

An AppImage cannot install a system service. The AppImage clients need the
daemon running on the system bus, so you must install `omafanctrld` separately —
either from the AUR (`yay -S omafanctrl`) or manually as above. Without the
daemon, the clients report `the omafanctrl daemon is not available`.

## Verifying the install

```sh
systemctl status omafanctrld
omafanctrl status
```

## Helper scripts

The repository ships three operational scripts under [`scripts/`](../scripts/).
They require root and a built release binary (`cargo build --release`).

| Script | Purpose |
| --- | --- |
| [`scripts/e2e-smoke.sh`](../scripts/e2e-smoke.sh) | End-to-end smoke test: starts the daemon, exercises the CLI, and always reverts the fan to BIOS auto |
| [`scripts/verify-watchdog.sh`](../scripts/verify-watchdog.sh) | Verifies the `kill -9` safety net (`ExecStopPost`) reverts the fan to BIOS auto |
| [`scripts/profile-daemon.sh`](../scripts/profile-daemon.sh) | Measures the daemon's idle CPU time, context switches, and memory |

```sh
sudo scripts/e2e-smoke.sh
sudo scripts/verify-watchdog.sh
sudo scripts/profile-daemon.sh 60
```

See [`docs/testing.md`](testing.md) for the full hardware checklist and
[`docs/profiling.md`](profiling.md) for the idle-cost methodology.