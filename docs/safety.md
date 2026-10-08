# Safety

omafanctrl writes directly to the Embedded Controller (EC). Writing the wrong
register can harm the machine, so the design is conservative.

## What is protected

- **Only two registers are ever written**: the fan-control register (`0x2F`) and
  the fan-switch register (`0x31`). All other writes are rejected.
- **Bounds checking**: register offsets are `u8`, so they can never address
  outside the 256-byte window.
- **Read-back verification**: after a fan change the control register is read
  back and the change is retried briefly.
- **Rate limiting**: writes are limited to at most one per 100 ms, except the
  safety-critical BIOS revert.
- **Temperature sanity**: readings above 127 °C are rejected.
- **Safety floor/ceiling**: commanded levels are clamped, and the ceiling is
  forced at the emergency temperature.
- **Watchdog**: the daemon reverts the fan to BIOS auto when it stops, is
  killed, or panics. The systemd unit adds `ExecStopPost=omafanctrld --revert`
  as a safety net for `kill -9`.

## The watchdog

The daemon holds a `Watchdog` that owns the EC handle. Dropping it — on clean
exit, on `SIGTERM`, or while unwinding from a panic — writes `0x80` (BIOS auto)
to the fan-control register. The control loop is additionally wrapped in
`run_guarded`, which converts a panic into an error after reverting.

## Risks

- The E14 Gen 4 fan curve is non-linear; levels 4-7 add no cooling over level 3.
- The `0x40` "disengaged" level is unverified and not used by default.
- Always keep the BIOS mode available as a fallback.

## Reverting manually

```sh
sudo omafanctrld --revert