# EC write audit

This document is the M9 safety audit of every code path that can write to the
Embedded Controller (EC). It enumerates the write surface, the invariants each
path must uphold, and the tests that pin those invariants.

The audit covers [`crates/omafanctrl-core/src/ec.rs`](../crates/omafanctrl-core/src/ec.rs),
which is the **only** module in the workspace that performs EC writes. The
daemon, GUI, CLI, and Waybar module never touch the EC directly; they go through
the daemon's D-Bus API.

## The write surface

There are exactly two EC registers the project may write:

| Register | Constant | Purpose |
| --- | --- | --- |
| `0x2F` | [`REG_FAN_CONTROL`](../crates/omafanctrl-core/src/ec.rs) | Fan control: `0x80` = BIOS auto, `0x01`–`0x07` = manual levels, `0x40` = disengaged |
| `0x31` | [`REG_FAN_SWITCH`](../crates/omafanctrl-core/src/ec.rs) | Fan-select handshake required before a control change |

They are listed in [`WRITABLE_REGISTERS`](../crates/omafanctrl-core/src/ec.rs).
Every other offset in the 256-byte window is read-only.

## Write paths

All writes funnel through two private primitives:

1. **`Ec::raw_write(offset, value)`** — the single choke point. It rejects any
   offset not in `WRITABLE_REGISTERS` with `EcError::WriteNotAllowed` before
   calling the backend. It does **not** apply the rate limit.
2. **`Ec::write_byte(offset, value)`** — the public low-level primitive. It
   applies the rate limit, then delegates to `raw_write`.

The public fan operations are built on `raw_write`:

| Operation | Value written | Rate-limited | Notes |
| --- | --- | --- | --- |
| [`Ec::set_fan_auto`](../crates/omafanctrl-core/src/ec.rs) | `0x80` | **No** | Safety-critical revert; must never be throttled |
| [`Ec::set_fan_level`](../crates/omafanctrl-core/src/ec.rs) | `0x01`–`0x07` | Yes | Rejects `0x00` (fan off) and `> 0x07` |
| [`Ec::set_fan_disengaged`](../crates/omafanctrl-core/src/ec.rs) | `0x40` | Yes | Unverified; not used by default |

`set_fan_auto`, `set_fan_level`, and `set_fan_disengaged` all call
`apply_fan_control`, which performs the TPFanCtrl2 fan-switch handshake:

```text
write 0x31 = 0x00   (select fan 1)
write 0x2F = value
write 0x31 = 0x01   (select fan 2)
write 0x2F = value
write 0x31 = 0x00   (select fan 1)
read  0x2F          (verify, retried up to 3 times)
```

## Invariants

| # | Invariant | Enforced by | Test |
| --- | --- | --- | --- |
| I1 | Only `0x2F` and `0x31` are ever written | `raw_write` / `WRITABLE_REGISTERS` | `every_non_writable_register_is_rejected`, `fan_operations_only_write_writable_registers` |
| I2 | Offsets can never leave the 256-byte window | `u8` offsets + `check_offset` in the backend | `backend_rejects_out_of_bounds_offsets` |
| I3 | Manual levels are `1..=7`; `0x00` (off) is rejected | `set_fan_level` | `rejects_invalid_fan_level` |
| I4 | The BIOS-auto sentinel is `0x80`, never `0x00` | `FAN_BIOS_AUTO` | `fan_auto_is_the_only_value_that_hands_back_control`, `writes_bios_auto_as_0x80` |
| I5 | A fan change is verified by reading `0x2F` back | `apply_fan_control` | `detects_failed_fan_control_apply` |
| I6 | Writes are rate-limited to one per 100 ms, except the revert | `enforce_rate_limit` | `rate_limits_writes`, `fan_auto_bypasses_rate_limit` |
| I7 | Temperatures above 127 °C are rejected | `read_temperature` / `TEMP_MAX_C` | `temperature_boundary_is_enforced`, `rejects_implausible_temperature` |
| I8 | The fan is reverted to BIOS auto on every exit path | `Watchdog` / `run_guarded` | `watchdog_reverts_to_bios_auto_on_drop`, `watchdog_reverts_when_unwinding_from_a_panic`, `run_guarded_reverts_on_panic` |

## Findings

- **No unguarded writes.** Every write in the crate passes through `raw_write`,
  which enforces I1. There is no `unsafe`, no direct `File::write` outside
  `FileBackend`, and no other module writes to the EC.
- **The rate limit is bypassed only for the safety revert.** `set_fan_auto`
  deliberately skips `enforce_rate_limit` so a revert can never be delayed. This
  is the only intentional bypass and is covered by `fan_auto_bypasses_rate_limit`.
- **The disengaged level (`0x40`) is unverified.** It is reachable only through
  the explicit `set_fan_disengaged` API, which is not called by the engine or the
  daemon. It remains gated behind the `Lev64Norm` option and is documented as
  unverified in [`docs/safety.md`](safety.md).
- **The engine clamps before writing.** `Engine::decide` clamps every commanded
  level to the safety floor/ceiling and forces the ceiling at the emergency
  temperature, so the EC layer is a second line of defence rather than the only
  one.

## Re-running the audit

The invariant tests run as part of the normal suite:

```sh
cargo test -p omafanctrl-core ec::tests
cargo test -p omafanctrl-core engine::tests
```

If a future change adds a new writable register or a new write path, the
`every_non_writable_register_is_rejected` and
`fan_operations_only_write_writable_registers` tests will fail, forcing a review
of this document.
