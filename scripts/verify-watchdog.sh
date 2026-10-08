#!/usr/bin/env bash
#
# Verify the watchdog's `kill -9` safety net on real hardware.
#
# The in-process watchdog cannot run after SIGKILL, so the systemd unit's
# ExecStopPost (`omafanctrld --revert`) performs the revert. This script
# simulates that sequence without systemd:
#
#   1. start the daemon
#   2. force a manual fan level
#   3. SIGKILL the daemon (no chance to revert in-process)
#   4. run `omafanctrld --revert` (the ExecStopPost safety net)
#   5. assert the fan-control register is back to 0x80
#
# Usage: sudo scripts/verify-watchdog.sh [path-to-config]

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG="${1:-$ROOT/data/profiles/e14-gen4.ini}"
DAEMON="$ROOT/target/release/omafanctrld"
CLI="$ROOT/target/release/omafanctrl"
EC_IO="/sys/kernel/debug/ec/ec0/io"
FAN_CONTROL_OFFSET=47   # 0x2F

if [[ "$(id -u)" -ne 0 ]]; then
  echo "error: run as root" >&2
  exit 1
fi

read_control() {
  # Read the single byte at 0x2F from the EC window.
  dd if="$EC_IO" bs=1 skip="$FAN_CONTROL_OFFSET" count=1 2>/dev/null | od -An -tu1 | tr -d ' '
}

cleanup() {
  "$DAEMON" --revert >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "== starting the daemon =="
"$DAEMON" --config "$CONFIG" >/tmp/omafanctrl-watchdog.log 2>&1 &
DAEMON_PID=$!
sleep 2

echo "== forcing a manual level =="
"$CLI" mode manual >/dev/null
"$CLI" level set 3 >/dev/null
sleep 1
before="$(read_control)"
echo "fan-control register before kill: 0x$(printf '%02X' "$before")"

echo "== SIGKILL (no in-process revert possible) =="
kill -9 "$DAEMON_PID"
wait "$DAEMON_PID" 2>/dev/null || true
sleep 1
after_kill="$(read_control)"
echo "fan-control register after kill:  0x$(printf '%02X' "$after_kill")"

echo "== running the ExecStopPost safety net =="
"$DAEMON" --revert >/dev/null
sleep 1
after_revert="$(read_control)"
echo "fan-control register after revert: 0x$(printf '%02X' "$after_revert")"

if [[ "$after_revert" -eq 128 ]]; then
  echo "PASS: the fan was reverted to BIOS auto (0x80)"
  exit 0
else
  echo "FAIL: expected 0x80, got 0x$(printf '%02X' "$after_revert")" >&2
  exit 1
fi
