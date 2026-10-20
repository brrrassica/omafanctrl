#!/usr/bin/env bash
#
# Diagnose an omafanctrl installation.
#
# Runs a series of read-only checks over the platform, the ec_sys module, the
# installed files, the daemon, and the D-Bus/polkit integration, and prints a
# PASS/WARN/FAIL report. Some checks need root (the EC window is root-only);
# run with sudo for the full report.
#
# Usage:
#   ./scripts/doctor.sh            # diagnose
#   sudo ./scripts/doctor.sh       # full report (EC window checks)
#   ./scripts/doctor.sh --help
#
# Exit status is non-zero if any check FAILs.

set -euo pipefail

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

readonly DAEMON_BIN="omafanctrld"
readonly CLI_BIN="omafanctrl"
readonly STATUS_BIN="omafanctrl-status"
readonly GUI_BIN="omafanctrl-gui"
readonly CONFIG_FILE="/etc/omafanctrl/TPFanControl.ini"
readonly EC_IO="/sys/kernel/debug/ec/ec0/io"

# ---------------------------------------------------------------------------
# Output helpers
# ---------------------------------------------------------------------------

if [[ -t 1 ]]; then
  C_RESET=$'\033[0m'; C_BOLD=$'\033[1m'; C_RED=$'\033[31m'
  C_GREEN=$'\033[32m'; C_YELLOW=$'\033[33m'
else
  C_RESET=""; C_BOLD=""; C_RED=""; C_GREEN=""; C_YELLOW=""
fi

pass=0
warns=0
fails=0

ok()   { printf '%s  PASS%s %s\n' "$C_GREEN" "$C_RESET" "$*"; pass=$((pass + 1)); }
warn() { printf '%s  WARN%s %s\n' "$C_YELLOW" "$C_RESET" "$*"; warns=$((warns + 1)); }
bad()  { printf '%s  FAIL%s %s\n' "$C_RED" "$C_RESET" "$*"; fails=$((fails + 1)); }
info() { printf '\n%s== %s ==%s\n' "$C_BOLD" "$*" "$C_RESET"; }

have() { command -v "$1" >/dev/null 2>&1; }

usage() {
  sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage ;;
    *) printf 'error: unknown argument: %s (try --help)\n' "$1" >&2; exit 2 ;;
  esac
done

# ---------------------------------------------------------------------------
# Platform detection
# ---------------------------------------------------------------------------

detect_omarchy() {
  local blob="" f d
  if [[ -r /etc/os-release ]]; then
    blob+=" $(. /etc/os-release 2>/dev/null; printf '%s %s %s' "${ID:-}" "${NAME:-}" "${PRETTY_NAME:-}")"
  fi
  for f in /etc/omarchy-release /etc/omarchy/version /etc/omarchy-version \
           /usr/share/omarchy/version /usr/local/share/omarchy/version; do
    [[ -r "$f" ]] && blob+=" $(cat "$f" 2>/dev/null || true)"
  done
  if have omarchy-version; then
    blob+=" $(omarchy-version 2>/dev/null || true)"
  fi
  if have omarchy; then
    blob+=" omarchy"
  fi
  if compgen -c 'omarchy-' >/dev/null 2>&1; then
    blob+=" omarchy"
  fi
  for d in /etc/omarchy /usr/share/omarchy /usr/local/share/omarchy; do
    [[ -d "$d" ]] && blob+=" omarchy"
  done
  printf '%s' "$blob"
}

detect_thinkpad() {
  local f blob=""
  for f in /sys/class/dmi/id/product_family \
           /sys/class/dmi/id/product_version \
           /sys/class/dmi/id/product_name \
           /sys/class/dmi/id/board_name; do
    [[ -r "$f" ]] && blob+=" $(cat "$f" 2>/dev/null || true)"
  done
  printf '%s' "$blob"
}

# ---------------------------------------------------------------------------
# Checks
# ---------------------------------------------------------------------------

info "Platform"
if have pacman; then
  ok "pacman present (Arch-based)"
else
  bad "pacman not found: omafanctrl targets Arch Linux / Omarchy only"
fi

omarchy="$(detect_omarchy)"
if printf '%s' "$omarchy" | grep -qi 'omarchy'; then
  ok "Omarchy detected"
else
  warn "Omarchy was not detected; omafanctrl targets Omarchy Quattro (4.x)"
fi

tp="$(detect_thinkpad)"
if printf '%s' "$tp" | grep -qi 'thinkpad'; then
  ok "ThinkPad detected:$(printf '%s' "$tp" | tr -s ' ')"
else
  warn "no ThinkPad detected (DMI:${tp:- unknown})"
fi

info "Kernel module (ec_sys)"
# ec_sys may be a loadable module (listed by lsmod) or built into the kernel
# (only /sys/module/ec_sys exists).
if lsmod | grep -q '^ec_sys' || [[ -d /sys/module/ec_sys ]]; then
  ok "ec_sys is present"
else
  bad "ec_sys is not loaded (run: modprobe ec_sys write_support=1)"
fi

write_support="$(cat /sys/module/ec_sys/parameters/write_support 2>/dev/null || true)"
case "$write_support" in
  Y|y|1) ok "ec_sys write_support is enabled" ;;
  "")    warn "cannot read ec_sys write_support (module not loaded?)" ;;
  *)     bad "ec_sys write_support is disabled (reload: modprobe -r ec_sys && modprobe ec_sys write_support=1)" ;;
esac

if grep -q 'debugfs' /proc/mounts 2>/dev/null; then
  ok "debugfs is mounted"
else
  bad "debugfs is not mounted (run: mount -t debugfs none /sys/kernel/debug)"
fi

if [[ -e "$EC_IO" ]]; then
  ok "EC window exists at $EC_IO"
  if [[ -w "$EC_IO" ]]; then
    ok "EC window is writable"
  elif [[ "$(id -u)" -eq 0 ]]; then
    bad "EC window is not writable"
  else
    warn "EC window writability needs root (re-run with sudo)"
  fi
else
  bad "EC window missing at $EC_IO"
fi

info "Installed files"
for bin in "$DAEMON_BIN" "$CLI_BIN" "$STATUS_BIN" "$GUI_BIN"; do
  if [[ -x "/usr/bin/$bin" ]]; then
    ok "/usr/bin/$bin"
  else
    bad "/usr/bin/$bin is missing"
  fi
done

if [[ -f /usr/lib/systemd/system/omafanctrld.service ]]; then
  ok "systemd unit installed"
else
  bad "systemd unit missing (/usr/lib/systemd/system/omafanctrld.service)"
fi

if [[ -f /usr/share/dbus-1/system.d/org.omarchy.omafanctrl.conf ]]; then
  ok "D-Bus policy installed"
else
  bad "D-Bus policy missing (the daemon cannot own its bus name)"
fi

if [[ -f /usr/share/polkit-1/actions/org.omarchy.omafanctrl.policy ]]; then
  ok "polkit action installed"
else
  bad "polkit action missing (clients will get 'authorization denied')"
fi

if [[ -f "$CONFIG_FILE" ]]; then
  ok "config present: $CONFIG_FILE"
else
  warn "config missing: $CONFIG_FILE (the daemon falls back to defaults)"
fi

info "Daemon"
if systemctl is-active --quiet omafanctrld; then
  ok "omafanctrld is active"
else
  bad "omafanctrld is not active (check: journalctl -u omafanctrld -e)"
fi

if systemctl is-enabled --quiet omafanctrld 2>/dev/null; then
  ok "omafanctrld is enabled"
else
  warn "omafanctrld is not enabled (run: systemctl enable omafanctrld)"
fi

if have "$CLI_BIN"; then
  if "$CLI_BIN" status >/dev/null 2>&1; then
    ok "CLI can reach the daemon over D-Bus"
  else
    bad "CLI cannot reach the daemon (is it running on the system bus?)"
  fi
else
  warn "CLI not installed; skipping the D-Bus round-trip"
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

printf '\n%d passed, %d warnings, %d failed\n' "$pass" "$warns" "$fails"
if [[ "$fails" -gt 0 ]]; then
  exit 1
fi
