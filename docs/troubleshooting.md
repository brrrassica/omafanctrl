# Troubleshooting

If you installed with [`install.sh`](../install.sh), re-running it repairs the
system files, reloads `ec_sys`, and restarts the daemon. The manual fixes below
are for installs done by hand.

## The daemon exits with `Request to own name refused by policy`

The D-Bus system-bus policy is not installed. Install it and reload D-Bus:

```sh
sudo install -Dm644 data/dbus/org.omarchy.omafanctrl.conf \
  /usr/share/dbus-1/system.d/org.omarchy.omafanctrl.conf
sudo systemctl reload dbus
```

## `the ec_sys kernel module is not loaded`

Load the module with write support:

```sh
sudo modprobe ec_sys write_support=1
```

To make it persistent, install the drop-ins:

```sh
sudo install -Dm644 data/modules-load.d/ec_sys.conf /usr/lib/modules-load.d/ec_sys.conf
sudo install -Dm644 data/modprobe.d/ec_sys.conf /usr/lib/modprobe.d/ec_sys.conf
```

## `EC device /sys/kernel/debug/ec/ec0/io was not found`

The `ec_sys` module exposes the window under debugfs. Ensure debugfs is mounted:

```sh
sudo mount -t debugfs none /sys/kernel/debug
```

and that `ec_sys` is loaded with `write_support=1`.

## `EC device ... is not writable`

The module was loaded without write support, or the process is not root. Reload
the module and run the daemon as root:

```sh
sudo modprobe -r ec_sys
sudo modprobe ec_sys write_support=1
```

## `authorization denied` from the CLI

The polkit action `org.omarchy.omafanctrl.control` denied the request. Install
the action and authenticate when prompted:

```sh
sudo install -Dm644 data/polkit/org.omarchy.omafanctrl.policy \
  /usr/share/polkit-1/actions/org.omarchy.omafanctrl.policy
```

## The clients report `the omafanctrl daemon is not available`

The daemon is not running or not on the system bus:

```sh
systemctl status omafanctrld
sudo systemctl start omafanctrld
```

## The fan does not change

- Check the mode: `omafanctrl status`. In BIOS mode omafanctrl does not control
  the fan.
- In Smart mode, the temperature may be below the first threshold.
- The minimum dwell time may be delaying a change; wait a few seconds.
- Verify the register map with the probe:

  ```sh
  sudo cargo run -p omafanctrl-core --bin omafanctrl-probe
  ```

## The fan is stuck at a manual level

Revert to BIOS auto:

```sh
omafanctrl mode bios
# or, if the daemon is gone:
sudo omafanctrld --revert
```

## Permissions

The daemon must run as root (it writes to the EC). The clients run as your user
and talk to the daemon over the system bus; they never touch the EC directly.