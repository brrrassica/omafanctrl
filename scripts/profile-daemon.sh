#!/usr/bin/env bash
#
# Measure the daemon's idle CPU cost and wakeups.
#
# Usage: sudo scripts/profile-daemon.sh [seconds] [path-to-config]
#
# Samples /proc/<pid>/stat and /proc/<pid>/status over the window and reports
# CPU time, context switches, and resident memory. See docs/profiling.md.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DURATION="${1:-60}"
CONFIG="${2:-$ROOT/data/profiles/e14-gen4.ini}"
DAEMON="$ROOT/target/release/omafanctrld"

if [[ "$(id -u)" -ne 0 ]]; then
  echo "error: run as root" >&2
  exit 1
fi

cleanup() {
  if [[ -n "${DAEMON_PID:-}" ]] && kill -0 "$DAEMON_PID" 2>/dev/null; then
    kill "$DAEMON_PID" 2>/dev/null || true
    wait "$DAEMON_PID" 2>/dev/null || true
  fi
  "$DAEMON" --revert >/dev/null 2>&1 || true
}
trap cleanup EXIT

"$DAEMON" --config "$CONFIG" >/tmp/omafanctrl-profile.log 2>&1 &
DAEMON_PID=$!
sleep 2

if ! kill -0 "$DAEMON_PID" 2>/dev/null; then
  echo "error: the daemon exited; see /tmp/omafanctrl-profile.log" >&2
  exit 1
fi

# utime+stime are fields 14 and 15 (1-based) of /proc/<pid>/stat, in clock ticks.
read_cpu_ticks() {
  awk '{print $14 + $15}' "/proc/$DAEMON_PID/stat"
}

read_status_field() {
  awk -v key="$1" '$1 == key {print $2}' "/proc/$DAEMON_PID/status"
}

HZ="$(getconf CLK_TCK)"
start_ticks="$(read_cpu_ticks)"
start_vol="$(read_status_field voluntary_ctxt_switches)"
start_invol="$(read_status_field nonvoluntary_ctxt_switches)"

echo "profiling pid $DAEMON_PID for ${DURATION}s (HZ=$HZ)..."
sleep "$DURATION"

end_ticks="$(read_cpu_ticks)"
end_vol="$(read_status_field voluntary_ctxt_switches)"
end_invol="$(read_status_field nonvoluntary_ctxt_switches)"
rss_kb="$(read_status_field VmRSS)"

cpu_ticks=$((end_ticks - start_ticks))
cpu_seconds="$(awk -v t="$cpu_ticks" -v hz="$HZ" 'BEGIN {printf "%.3f", t / hz}')"
cpu_percent="$(awk -v t="$cpu_ticks" -v hz="$HZ" -v d="$DURATION" 'BEGIN {printf "%.3f", (t / hz) / d * 100}')"
vol=$((end_vol - start_vol))
invol=$((end_invol - start_invol))
vol_per_sec="$(awk -v v="$vol" -v d="$DURATION" 'BEGIN {printf "%.2f", v / d}')"

cat <<EOF

== omafanctrld idle profile (${DURATION}s) ==
CPU time:             ${cpu_seconds}s
Idle CPU:             ${cpu_percent}% of one core
Voluntary switches:   ${vol} (${vol_per_sec}/s)
Involuntary switches: ${invol}
Resident memory:      ${rss_kb} kB
EOF
