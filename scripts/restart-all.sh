#!/usr/bin/env bash
#
# Restart every omafanctrl component.
#
# Reloads the ec_sys module with write support, restarts the omafanctrld
# service, and asks the running omarchy-shell to reload its bar configuration
# so the status module picks up the new daemon. Use it after changing the
# config, upgrading the binaries, or when the bar module goes stale.
#
# Usage:
#   ./scripts/restart-all.sh                 # restart (re-execs itself with sudo)
#   sudo ./scripts/restart-all.sh            # same, already root
#   ./scripts/restart-all.sh --no-user-setup # skip the omarchy-shell reload
#   ./scripts/restart-all.sh --help
#
# The script is idempotent: re-running it is harmless.

set -euo pipefail

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

readonly CLI_BIN="omafanctrl"

# ---------------------------------------------------------------------------
# Output helpers
# ---------------------------------------------------------------------------

if [[ -t 1 ]]; then
  C_RESET=$'\033[0m'; C_BOLD=$'\033[1m'; C_RED=$'\033[31m'
  C_GREEN=$'\033[32m'; C_YELLOW=$'\033[33m'; C_BLUE=$'\033[34m'
else
  C_RESET=""; C_BOLD=""; C_RED=""; C_GREEN=""; C_YELLOW=""; C_BLUE=""
fi

info()  { printf '%s==>%s %s\n' "$C_BLUE$C_BOLD" "$C_RESET" "$*"; }
ok()    { printf '%s  ok%s %s\n' "$C_GREEN" "$C_RESET" "$*"; }
warn()  { printf '%swarn%s %s\n' "$C_YELLOW" "$C_RESET" "$*" >&2; }
die()   { printf '%serror%s %s\n' "$C_RED$C_BOLD" "$C_RESET" "$*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

DO_USER_SETUP=1

# Keep the original arguments so the sudo re-exec can forward them.
ORIG_ARGS=("$@")

usage() {
  sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-user-setup) DO_USER_SETUP=0 ;;
    -h|--help) usage ;;
    *) die "unknown argument: $1 (try --help)" ;;
  esac
  shift
done

# ---------------------------------------------------------------------------
# Locate the script (works from the repo and from the release zip)
# ---------------------------------------------------------------------------

SCRIPT_PATH="$(readlink -f "${BASH_SOURCE[0]}")"

# ---------------------------------------------------------------------------
# Re-exec with sudo
# ---------------------------------------------------------------------------

if [[ "$(id -u)" -ne 0 ]]; then
  command -v sudo >/dev/null 2>&1 || die "sudo is required; re-run as root"
  info "Elevating with sudo"
  exec sudo -- "$SCRIPT_PATH" "${ORIG_ARGS[@]}"
fi

# The user whose desktop we configured (the one who invoked sudo).
TARGET_USER="${SUDO_USER:-}"
if [[ -z "$TARGET_USER" || "$TARGET_USER" == "root" ]]; then
  TARGET_USER="$(logname 2>/dev/null || true)"
fi
if [[ -z "$TARGET_USER" || "$TARGET_USER" == "root" ]]; then
  TARGET_USER="$(awk -F: '$3 >= 1000 && $3 < 65534 { print $1; exit }' /etc/passwd)"
fi

# ---------------------------------------------------------------------------
# Reload the ec_sys module with write support
# ---------------------------------------------------------------------------

info "Reloading the ec_sys kernel module with write support"
modprobe -r ec_sys 2>/dev/null || true
modprobe ec_sys write_support=1
if [[ -e /sys/kernel/debug/ec/ec0/io ]]; then
  ok "EC window is available at /sys/kernel/debug/ec/ec0/io"
else
  warn "EC window not found; ensure debugfs is mounted (mount -t debugfs none /sys/kernel/debug)"
fi

# ---------------------------------------------------------------------------
# Restart the daemon
# ---------------------------------------------------------------------------

info "Reloading systemd and restarting omafanctrld"
systemctl daemon-reload
if systemctl list-unit-files omafanctrld.service >/dev/null 2>&1; then
  systemctl restart omafanctrld \
    || warn "omafanctrld failed to restart; check: journalctl -u omafanctrld -e"
else
  warn "omafanctrld.service is not installed; run scripts/install.sh first"
fi

# ---------------------------------------------------------------------------
# Reload the omarchy-shell bar module
# ---------------------------------------------------------------------------

# Best-effort: ask the running shell to reload shell.json. The shell also
# hot-reloads on save, so a failure here is not fatal.
reload_omarchy_shell() {
  command -v omarchy-shell >/dev/null 2>&1 || return 0
  local uid runtime
  uid="$(id -u "$TARGET_USER" 2>/dev/null || true)"
  [[ -n "$uid" ]] || return 0
  runtime="/run/user/$uid"
  if sudo -u "$TARGET_USER" env \
    XDG_RUNTIME_DIR="$runtime" \
    DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus" \
    OMARCHY_PATH="${OMARCHY_PATH:-/usr/share/omarchy}" \
    omarchy-shell shell reloadConfig >/dev/null 2>&1; then
    ok "omarchy-shell reloaded"
  else
    warn "could not reload omarchy-shell; run 'omarchy restart shell' to apply"
  fi
}

if [[ "$DO_USER_SETUP" -eq 1 ]]; then
  info "Reloading the omarchy-shell bar module for ${TARGET_USER:-<unknown>}"
  reload_omarchy_shell
else
  info "Skipping the omarchy-shell reload (--no-user-setup)"
fi

# ---------------------------------------------------------------------------
# Verify
# ---------------------------------------------------------------------------

info "Verifying"
sleep 1
if systemctl is-active --quiet omafanctrld; then
  ok "omafanctrld is active"
else
  warn "omafanctrld is not active; check: journalctl -u omafanctrld -e"
fi
if command -v "$CLI_BIN" >/dev/null 2>&1 && "$CLI_BIN" status >/dev/null 2>&1; then
  ok "CLI can reach the daemon"
else
  warn "CLI could not reach the daemon yet; it may need a moment to start"
fi

cat <<EOF

${C_GREEN}${C_BOLD}omafanctrl restarted.${C_RESET}

  Daemon : systemctl status omafanctrld
  CLI    : omafanctrl status
EOF
