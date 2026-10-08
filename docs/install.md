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
- The `ec_sys` kernel module with write support
- GTK4 and libadwaita (for the GUI)

## From the AUR

```sh
yay -S omafanctrl-git
```

The package installs the binaries, the D-Bus policy, the polkit action, the
systemd unit, the `ec_sys` drop-ins, the desktop entry, and a default
configuration at `/etc/omafanctrl/TPFanControl.ini`.

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
either from the AUR (`yay -S omafanctrl-git`) or manually as above. Without the
daemon, the clients report `the omafanctrl daemon is not available`.

## Verifying the install

```sh
systemctl status omafanctrld
omafanctrl status