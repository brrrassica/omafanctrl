# Testing

omafanctrl is tested at three levels:

1. **Unit and integration tests** — run on every commit in CI, no hardware
   required.
2. **Fuzzing** — the `.ini` parser is fuzzed with `cargo-fuzz`; a deterministic
   equivalent runs in CI.
3. **End-to-end tests on real hardware** — a manual checklist, because the EC is
   only reachable on the target ThinkPad E14 Gen 4.

## Automated tests

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build --all
```

The suite covers the EC layer, the config parser, the control engine, the
daemon control loop (against a mocked EC), the D-Bus service, and the GUI
helpers. The safety invariants are pinned by dedicated tests; see
[`docs/ec-write-audit.md`](ec-write-audit.md).

## Fuzzing the `.ini` parser

The parser must never panic on arbitrary input, and any configuration that
parses must round-trip losslessly through `Config::to_ini`.

- **CI (stable):** `config::tests::fuzz_parser_never_panics_and_round_trips`
  drives the parser with a fixed-seed PRNG and a corpus of structurally
  interesting seeds.
- **Deep fuzzing (nightly):** the `cargo-fuzz` target in
  [`fuzz/`](../fuzz/README.md):

  ```sh
  cd fuzz
  cargo +nightly fuzz run parse_config
  ```

## End-to-end checklist (real hardware)

Run these on the target ThinkPad E14 Gen 4 with `ec_sys write_support=1` loaded.
Each step has an expected result; a failure is a release blocker.

### 0. Preconditions

- [ ] `lsmod | grep ec_sys` shows the module loaded with `write_support=1`.
- [ ] `/sys/kernel/debug/ec/ec0/io` exists and is readable as root.
- [ ] `cargo build --release` succeeds.

### 1. Probe the register map

- [ ] `sudo ./target/release/omafanctrl-probe` dumps 256 bytes and decodes the
      fan and temperature registers.
- [ ] The fan-control register (`0x2F`) reads `0x80` while under BIOS control.
- [ ] The reported RPM matches `/proc/acpi/ibm/fan`.

### 2. Daemon lifecycle

- [ ] `sudo ./target/release/omafanctrld --config data/profiles/e14-gen4.ini`
      starts and logs `serving the control interface`.
- [ ] `./target/release/omafanctrl status` reports the mode, RPM, and
      temperatures.
- [ ] `systemctl status omafanctrld` is `active (running)` when installed as a
      service.

### 3. Modes

- [ ] `omafanctrl mode bios` hands the fan back to the firmware; the RPM follows
      the BIOS curve.
- [ ] `omafanctrl mode manual` then `omafanctrl level set 3` spins the fan up;
      the RPM rises to ~3900.
- [ ] `omafanctrl level set 1` drops the RPM to ~1800.
- [ ] `omafanctrl mode smart` follows the configured thresholds as the CPU
      temperature crosses them.
- [ ] `omafanctrl mode cycle` cycles BIOS → Manual → Smart → BIOS.
- [ ] `omafanctrl hysteresis toggle` flips the hysteresis flag in `status`.

### 4. Configuration

- [ ] `omafanctrl config get` prints the active `.ini`.
- [ ] Editing the file on disk and running `omafanctrl reload` applies the new
      thresholds without restarting the daemon.
- [ ] `omafanctrl config set <file>` validates and persists a new configuration.

### 5. Clients

- [ ] `omafanctrl-gui` shows live temperatures, RPM, and the mode switch, and
      updates when the mode changes from the CLI.
- [ ] The tab strip below the header switches between Dashboard, Smart Curve,
      and Sensors, and collapses to icons only at narrow widths.
- [ ] The GUI dashboard reflows through the `desktop`, `half`, `quarter`, and
      `eighth` layouts as the window is resized, keeping the graphs on top and
      the status and controls below them.
- [ ] The header Settings menu button opens the preferences dialog.
- [ ] `omafanctrl-waybar --show icon,mode,rpm,temp` prints valid Waybar JSON.
- [ ] The Hyprland hotkeys in `data/hyprland/bindings.lua` switch modes.

### 6. Safety

- [ ] `sudo omafanctrld --revert` writes `0x80` to `0x2F` and exits 0.
- [ ] Stopping the daemon (`SIGTERM`) reverts the fan to BIOS auto.
- [ ] The watchdog checks in [Watchdog verification](#watchdog-verification)
      pass.

## Watchdog verification

The watchdog must revert the fan to BIOS auto on **every** exit path. The
in-process paths are covered by unit tests; the `kill -9` path relies on the
systemd `ExecStopPost` safety net and is verified on hardware.

| Exit path | Mechanism | Coverage |
| --- | --- | --- |
| Clean exit / `SIGTERM` | `Watchdog::drop` | `watchdog_reverts_to_bios_auto_on_drop` |
| Explicit revert | `Watchdog::revert` / `omafanctrld --revert` | `watchdog_revert_can_be_called_explicitly` |
| Panic | `Watchdog::drop` during unwinding + `run_guarded` | `watchdog_reverts_when_unwinding_from_a_panic`, `run_guarded_reverts_on_panic` |
| `kill -9` | systemd `ExecStopPost=omafanctrld --revert` | `scripts/verify-watchdog.sh` |

### Automated panic test

```sh
cargo test -p omafanctrl-core engine::tests::watchdog
cargo test -p omafanctrl-core engine::tests::run_guarded
```

### `kill -9` on hardware

The in-process watchdog cannot run after `SIGKILL`, so the systemd unit's
`ExecStopPost` performs the revert. Verify it with:

```sh
sudo scripts/verify-watchdog.sh
```

The script starts the daemon, forces a manual level, sends `SIGKILL`, runs the
`--revert` safety net, and asserts the fan-control register is back to `0x80`.
See [`scripts/verify-watchdog.sh`](../scripts/verify-watchdog.sh).

## Profiling

Idle CPU wakeups and cost are measured with
[`scripts/profile-daemon.sh`](../scripts/profile-daemon.sh); see
[`docs/profiling.md`](profiling.md).
