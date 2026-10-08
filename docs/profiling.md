# Profiling the daemon

`omafanctrld` runs continuously, so its idle cost matters. This document
describes how to measure CPU wakeups and idle CPU time, and the trade-offs that
control them.

## What the daemon does while idle

The control loop is driven by a Tokio interval timer. Two cadences are at play:

| Cadence | Default | Purpose |
| --- | --- | --- |
| Telemetry | 250 ms (4 Hz) | Sample sensors and publish a `State` snapshot for the GUI history |
| Control | 5 s | Evaluate the engine and write the EC if the fan level must change |

The telemetry cadence dominates idle wakeups: at 4 Hz the process wakes roughly
four times per second even when nothing changes. The control cadence is
comparatively negligible.

## Measuring

Use [`scripts/profile-daemon.sh`](../scripts/profile-daemon.sh), which samples
the daemon's CPU time and context switches over a fixed window:

```sh
sudo scripts/profile-daemon.sh 60
```

It reports:

- **CPU time** consumed over the window (from `/proc/<pid>/stat`), and the
  implied idle CPU percentage.
- **Voluntary and involuntary context switches** (from `/proc/<pid>/status`),
  a proxy for wakeups.
- **Resident memory** (from `/proc/<pid>/status`).

For a wakeup-level breakdown, run `powertop` or `perf` alongside it:

```sh
sudo powertop --time=60
# or
sudo perf stat -e context-switches,cpu-clock -p "$(pgrep omafanctrld)" -- sleep 60
```

## Expected idle cost

On the E14 Gen 4, with the default 250 ms telemetry interval and a steady
temperature, the daemon should consume well under 1 % of one core and a handful
of context switches per second. The exact numbers depend on the kernel and the
`ec_sys` read cost.

## Tuning the telemetry interval

The telemetry interval is configurable, so idle cost can be traded against GUI
responsiveness:

```sh
sudo omafanctrld --telemetry-interval-ms 1000
```

| Interval | Wakeups | Effect |
| --- | --- | --- |
| 100 ms | 10 Hz | Smoothest GUI history, highest idle cost |
| 250 ms (default) | 4 Hz | Balanced |
| 1000 ms | 1 Hz | Lowest idle cost, coarser history |

The interval is clamped to at least 1 ms. The control cadence is independent and
is set with `--interval <secs>`.

## Reducing wakeups further

- Increase `--telemetry-interval-ms` when the GUI is not in use.
- The control loop only writes the EC when the engine's decision changes, so a
  steady temperature produces no EC writes.
- The daemon does not poll the configuration file on a separate timer; it checks
  the file fingerprint on each control tick.
