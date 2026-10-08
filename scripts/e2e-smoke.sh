#!/usr/bin/env bash
#
# End-to-end smoke test for omafanctrl on real hardware.
#
# Requires root, the `ec_sys` module with write_support=1, and a built release
# binary. It starts the daemon, exercises the CLI, and always reverts the fan to
# BIOS auto on exit.
#
# Usage: sudo scripts/e2e-smoke.sh [path-to-config]

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG="${1:-$ROOT/data/profiles/e14-gen4.ini}"
DAEMON="$ROOT/target/release/omafanctrld"
CLI="$ROOT/target/release/omafanctrl"
PROBE="$ROOT/target/release/omafanctrl-probe"

pass=0
fail=0

ok()   { printf '  \033[32mPASS\033[0m %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; fail=$((fail + 1)); }
info() { printf '\n== %s ==\n' "$1"; }

cleanup() {
  if [[ -n "${DAEMON_PID:-}" ]] && kill -0 "$DAEMON_PID" 2>/dev/null; then
    kill "$DAEMON_PID" 2>/dev/null || true
    wait "$DAEMON_PID" 2>/dev/null || true
  fi
  # Always hand the fan back to the firmware.
  "$DAEMON" --revert >/dev/null 2>&1 || true
}
trap cleanup EXIT

if [[ "$(id -u)" -ne 0 ]]; then
  echo "error: run as root (the daemon writes to the EC)" >&2
  exit 1
fi

for bin in "$DAEMON" "$CLI" "$PROBE"; do
  if [[ ! -x "$bin" ]]; then
    echo "error: $bin not found; run 'cargo build --release' first" >&2
    exit 1
  fi
done

info "Preconditions"
if lsmod | grep -q '^ec_sys'; then ok "ec_sys is loaded"; else bad "ec_sys is not loaded"; fi
if [[ -e /sys/kernel/debug/ec/ec0/io ]]; then ok "EC window exists"; else bad "EC window missing"; fi

info "Probe"
if "$PROBE" >/dev/null 2>&1; then ok "probe ran"; else bad "probe failed"; fi

info "Daemon"
"$DAEMON" --config "$CONFIG" >/tmp/omafanctrl-e2e.log 2>&1 &
DAEMON_PID=$!
sleep 2
if kill -0 "$DAEMON_PID" 2>/dev/null; then ok "daemon is running"; else bad "daemon exited"; fi

info "CLI"
if "$CLI" status >/dev/null 2>&1; then ok "status"; else bad "status"; fi
if "$CLI" mode bios >/dev/null 2>&1; then ok "mode bios"; else bad "mode bios"; fi
if "$CLI" mode manual >/dev/null 2>&1; then ok "mode manual"; else bad "mode manual"; fi
if "$CLI" level set 3 >/dev/null 2>&1; then ok "level set 3"; else bad "level set 3"; fi
if "$CLI" mode smart >/dev/null 2>&1; then ok "mode smart"; else bad "mode smart"; fi
if "$CLI" mode cycle >/dev/null 2>&1; then ok "mode cycle"; else bad "mode cycle"; fi
if "$CLI" config get >/dev/null 2>&1; then ok "config get"; else bad "config get"; fi
if "$CLI" reload >/dev/null 2>&1; then ok "reload"; else bad "reload"; fi

info "Revert"
if "$DAEMON" --revert >/dev/null 2>&1; then ok "revert"; else bad "revert"; fi

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[[ "$fail" -eq 0 ]]
