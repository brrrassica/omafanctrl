#!/usr/bin/env bash
#
# omafanctrl uninstaller.
#
# Reverses scripts/install.sh: stops and disables the daemon, hands the fan back
# to BIOS auto, removes the binaries and system integration files, and (best
# effort) removes the per-user Hyprland keybindings and the omarchy-shell bar
# module. The configuration in /etc/omafanctrl is kept unless --purge is given.
#
# Usage:
#   ./scripts/uninstall.sh                 # uninstall (re-execs itself with sudo)
#   sudo ./scripts/uninstall.sh            # same, already root
#   ./scripts/uninstall.sh --purge         # also remove /etc/omafanctrl
#   ./scripts/uninstall.sh --no-user-setup # skip the per-user cleanup
#   ./scripts/uninstall.sh --help
#
# The script is idempotent: re-running it is harmless.

set -euo pipefail

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

readonly DAEMON_BIN="omafanctrld"
readonly CLI_BIN="omafanctrl"
readonly STATUS_BIN="omafanctrl-status"
readonly GUI_BIN="omafanctrl-gui"
readonly PROBE_BIN="omafanctrl-probe"

readonly CONFIG_DIR="/etc/omafanctrl"
readonly CONFIG_FILE="$CONFIG_DIR/TPFanControl.ini"

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
PURGE=0

# Keep the original arguments so the sudo re-exec can forward them.
ORIG_ARGS=("$@")

usage() {
  sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --purge) PURGE=1 ;;
    --no-user-setup) DO_USER_SETUP=0 ;;
    -h|--help) usage ;;
    *) die "unknown argument: $1 (try --help)" ;;
  esac
  shift
done

# ---------------------------------------------------------------------------
# Locate the payload (works from the repo and from the release zip)
# ---------------------------------------------------------------------------

SCRIPT_PATH="$(readlink -f "${BASH_SOURCE[0]}")"
SCRIPT_DIR="$(cd "$(dirname "$SCRIPT_PATH")" && pwd)"
# The script lives in scripts/, so the payload (data/) is one level up.
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DATA_DIR="$ROOT_DIR/data"

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
TARGET_HOME=""
if [[ -n "$TARGET_USER" ]]; then
  TARGET_HOME="$(getent passwd "$TARGET_USER" | cut -d: -f6)"
fi

# ---------------------------------------------------------------------------
# Stop the daemon and hand the fan back to the firmware
# ---------------------------------------------------------------------------

info "Stopping the omafanctrld service"
if systemctl list-unit-files omafanctrld.service >/dev/null 2>&1; then
  # ExecStopPost runs `omafanctrld --revert`, so the fan is reverted on stop.
  systemctl disable --now omafanctrld 2>/dev/null || true
fi
# Safety net: revert even if the unit was already gone or never installed.
if command -v "$DAEMON_BIN" >/dev/null 2>&1; then
  "$DAEMON_BIN" --revert >/dev/null 2>&1 || true
fi
ok "daemon stopped and the fan reverted to BIOS auto"

# ---------------------------------------------------------------------------
# Remove binaries
# ---------------------------------------------------------------------------

info "Removing binaries from /usr/bin"
for bin in "$DAEMON_BIN" "$CLI_BIN" "$STATUS_BIN" "$GUI_BIN" "$PROBE_BIN"; do
  rm -f "/usr/bin/$bin"
done
ok "binaries removed"

# ---------------------------------------------------------------------------
# Remove system integration files
# ---------------------------------------------------------------------------

info "Removing system integration files"
rm -f /usr/share/dbus-1/system.d/org.omarchy.omafanctrl.conf
rm -f /usr/share/polkit-1/actions/org.omarchy.omafanctrl.policy
rm -f /usr/lib/systemd/system/omafanctrld.service
rm -f /usr/lib/modules-load.d/ec_sys.conf
rm -f /usr/lib/modprobe.d/ec_sys.conf
rm -f /usr/share/applications/org.omarchy.omafanctrl.desktop
rm -f /usr/share/icons/hicolor/scalable/apps/org.omarchy.omafanctrl.svg
ok "system files removed"

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

if [[ "$PURGE" -eq 1 ]]; then
  info "Removing configuration"
  rm -rf "$CONFIG_DIR"
  ok "removed $CONFIG_DIR"
else
  info "Keeping $CONFIG_FILE (pass --purge to remove it)"
fi

# ---------------------------------------------------------------------------
# Reload D-Bus/systemd and refresh the desktop caches
# ---------------------------------------------------------------------------

info "Reloading D-Bus and systemd"
systemctl reload dbus 2>/dev/null || systemctl restart dbus 2>/dev/null || true
systemctl daemon-reload

command -v update-desktop-database >/dev/null 2>&1 \
  && update-desktop-database -q /usr/share/applications 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null 2>&1 \
  && gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor 2>/dev/null || true

# ---------------------------------------------------------------------------
# Per-user desktop integration (Hyprland + omarchy-shell bar)
# ---------------------------------------------------------------------------

# Remove the exact block that install.sh appended to bindings.lua. The block is
# the shipped data/hyprland/bindings.lua; matching it verbatim avoids touching
# the user's own bindings. Falls back to dropping the omafanctrl lines when the
# shipped file is unavailable.
remove_hyprland_bindings() {
  local bindings="$TARGET_HOME/.config/hypr/bindings.lua"
  if [[ ! -f "$bindings" ]]; then
    warn "no $bindings; skipping Hyprland cleanup"
    return 0
  fi
  if ! grep -q 'omafanctrl' "$bindings"; then
    ok "no omafanctrl Hyprland bindings to remove"
    return 0
  fi

  local block="$DATA_DIR/hyprland/bindings.lua"
  local tmp; tmp="$(mktemp)"
  if [[ -f "$block" ]]; then
    awk -v blockfile="$block" '
      BEGIN {
        n = 0
        while ((getline l < blockfile) > 0) { blk[++n] = l }
        close(blockfile)
      }
      { tgt[NR] = $0; m = NR }
      END {
        start = 0
        for (i = 1; i <= m - n + 1; i++) {
          hit = 1
          for (j = 1; j <= n; j++) {
            if (tgt[i + j - 1] != blk[j]) { hit = 0; break }
          }
          if (hit) { start = i; break }
        }
        for (i = 1; i <= m; i++) {
          if (start && i >= start && i < start + n) continue
          print tgt[i]
        }
      }
    ' "$bindings" > "$tmp"
  else
    grep -v 'omafanctrl' "$bindings" > "$tmp" || true
  fi

  if cmp -s "$bindings" "$tmp"; then
    rm -f "$tmp"
    ok "no omafanctrl Hyprland bindings to remove"
  else
    cat "$tmp" > "$bindings"
    rm -f "$tmp"
    chown "$TARGET_USER:$TARGET_USER" "$bindings" 2>/dev/null || true
    ok "removed the omafanctrl Hyprland bindings"
  fi
}

# Remove the omafanctrl entry from every bar.layout section of shell.json.
remove_shell_module() {
  local shell_json="$TARGET_HOME/.config/omarchy/shell.json"
  if [[ ! -f "$shell_json" ]]; then
    warn "no $shell_json; skipping omarchy-shell cleanup"
    return 0
  fi
  if ! grep -q 'omafanctrl' "$shell_json"; then
    ok "no omafanctrl bar module to remove"
    return 0
  fi

  local tmp; tmp="$(mktemp)"
  if command -v jq >/dev/null 2>&1; then
    jq '
      def object_or_empty: if type == "object" then . else {} end;
      def array_or_empty: if type == "array" then . else [] end;
      def entry_id: if type == "object" then (.id // "") else tostring end;
      def without: [.[] | select(entry_id != "omafanctrl")];
      object_or_empty
      | .bar = (.bar | object_or_empty)
      | .bar.layout = (.bar.layout | object_or_empty)
      | .bar.layout.left = ((.bar.layout.left | array_or_empty) | without)
      | .bar.layout.center = ((.bar.layout.center | array_or_empty) | without)
      | .bar.layout.right = ((.bar.layout.right | array_or_empty) | without)
    ' "$shell_json" > "$tmp"
  elif command -v python3 >/dev/null 2>&1; then
    python3 - "$shell_json" > "$tmp" <<'PY'
import json
import sys

path = sys.argv[1]
with open(path, encoding="utf-8") as fh:
    data = json.load(fh)
if not isinstance(data, dict):
    data = {}
bar = data.get("bar") if isinstance(data.get("bar"), dict) else {}
data["bar"] = bar
layout = bar.get("layout") if isinstance(bar.get("layout"), dict) else {}
bar["layout"] = layout


def entry_id(entry):
    if isinstance(entry, dict):
        return str(entry.get("id", ""))
    return str(entry)


for key in ("left", "center", "right"):
    entries = layout.get(key) if isinstance(layout.get(key), list) else []
    layout[key] = [entry for entry in entries if entry_id(entry) != "omafanctrl"]

json.dump(data, sys.stdout, indent=2, sort_keys=True)
sys.stdout.write("\n")
PY
  else
    warn "jq or python3 is required to edit $shell_json; remove the omafanctrl entry manually"
    rm -f "$tmp"
    return 0
  fi

  cat "$tmp" > "$shell_json"
  rm -f "$tmp"
  chown "$TARGET_USER:$TARGET_USER" "$shell_json" 2>/dev/null || true
  ok "removed the omafanctrl bar module"
}

# Best-effort: ask the running shell to reload shell.json.
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
  if [[ -z "$TARGET_HOME" || ! -d "$TARGET_HOME" ]]; then
    warn "could not determine the desktop user's home; skipping per-user cleanup"
  else
    info "Cleaning up the desktop integration for ${TARGET_USER:-<unknown>}"
    remove_hyprland_bindings
    remove_shell_module
    reload_omarchy_shell
  fi
else
  info "Skipping per-user cleanup (--no-user-setup)"
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

cat <<EOF

${C_GREEN}${C_BOLD}omafanctrl is uninstalled.${C_RESET}

The ec_sys module is still loaded; it will not load on the next boot now that
the drop-ins are gone. To unload it immediately:

  sudo modprobe -r ec_sys

Reload Hyprland to drop the removed keybindings.
EOF
