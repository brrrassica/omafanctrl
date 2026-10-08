#!/usr/bin/env bash
#
# omafanctrl installer.
#
# One script that sets up every component of omafanctrl on a ThinkPad running
# Omarchy Quattro (4.x): it installs the dependencies, builds (or uses the
# prebuilt) binaries, installs the daemon, CLI, Waybar module, and GUI, wires up
# the D-Bus policy, polkit action, systemd unit, and ec_sys drop-ins, loads the
# EC module, enables the daemon, and configures the Hyprland and Waybar
# integrations for the invoking user.
#
# It is designed to "just work" on Omarchy 4 on a ThinkPad. On any other
# distribution or hardware it refuses to run.
#
# Usage:
#   ./install.sh                 # install (re-execs itself with sudo)
#   sudo ./install.sh            # same, already root
#   ./install.sh --no-user-setup  # skip the per-user Hyprland/Waybar wiring
#   ./install.sh --skip-preflight # bypass the Omarchy/ThinkPad checks (testing)
#   ./install.sh --help
#
# The script is idempotent: re-running it upgrades the binaries and system files
# in place and never duplicates the user-level snippets.

set -euo pipefail

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

readonly SCRIPT_NAME="omafanctrl"
readonly DAEMON_BIN="omafanctrld"
readonly CLI_BIN="omafanctrl"
readonly WAYBAR_BIN="omafanctrl-waybar"
readonly GUI_BIN="omafanctrl-gui"
readonly PROBE_BIN="omafanctrl-probe"

readonly CONFIG_DIR="/etc/omafanctrl"
readonly CONFIG_FILE="$CONFIG_DIR/TPFanControl.ini"

readonly MARKER_WAYBAR="/* >>> omafanctrl (managed by install.sh) >>> */"

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
SKIP_PREFLIGHT=0

# Keep the original arguments so the sudo re-exec can forward them.
ORIG_ARGS=("$@")

usage() {
  sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-user-setup) DO_USER_SETUP=0 ;;
    --skip-preflight) SKIP_PREFLIGHT=1 ;;
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

# The release zip ships prebuilt binaries in ./bin; a source checkout builds
# them into ./target/release.
if [[ -d "$SCRIPT_DIR/bin" && -x "$SCRIPT_DIR/bin/$DAEMON_BIN" ]]; then
  BIN_DIR="$SCRIPT_DIR/bin"
  PREBUILT=1
elif [[ -d "$SCRIPT_DIR/target/release" && -x "$SCRIPT_DIR/target/release/$DAEMON_BIN" ]]; then
  BIN_DIR="$SCRIPT_DIR/target/release"
  PREBUILT=1
else
  BIN_DIR="$SCRIPT_DIR/target/release"
  PREBUILT=0
fi

DATA_DIR="$SCRIPT_DIR/data"

# ---------------------------------------------------------------------------
# Re-exec with sudo
# ---------------------------------------------------------------------------

if [[ "$(id -u)" -ne 0 ]]; then
  command -v sudo >/dev/null 2>&1 || die "sudo is required; re-run as root"
  info "Elevating with sudo"
  exec sudo --preserve-env=OMAFANCTRL_SKIP_PREFLIGHT -- "$SCRIPT_PATH" "${ORIG_ARGS[@]}"
fi

# The user whose desktop we configure (the one who invoked sudo).
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
# Preflight: Omarchy 4 on a ThinkPad
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
  if command -v omarchy-version >/dev/null 2>&1; then
    blob+=" $(omarchy-version 2>/dev/null || true)"
  fi
  # Omarchy ships a family of `omarchy-*` commands and a data directory.
  if command -v omarchy >/dev/null 2>&1; then
    blob+=" omarchy"
  fi
  if compgen -c 'omarchy-' >/dev/null 2>&1; then
    blob+=" omarchy"
  fi
  for d in /etc/omarchy /usr/share/omarchy /usr/local/share/omarchy \
           "${TARGET_HOME:-}/.local/share/omarchy"; do
    [[ -n "$d" && -d "$d" ]] && blob+=" omarchy"
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

preflight() {
  info "Checking the platform"

  command -v pacman >/dev/null 2>&1 \
    || die "pacman not found: omafanctrl targets Arch Linux / Omarchy only"

  local omarchy; omarchy="$(detect_omarchy)"
  if ! printf '%s' "$omarchy" | grep -qi 'omarchy'; then
    die "Omarchy was not detected. omafanctrl only supports Omarchy Quattro (4.x)."
  fi
  ok "Omarchy detected"

  # Version 4 ("Quattro"). If a version is discoverable, require 4.x.
  if printf '%s' "$omarchy" | grep -qiE 'quattro|(^|[^0-9])4([^0-9]|$)'; then
    ok "Omarchy 4.x (Quattro) detected"
  else
    warn "could not confirm the Omarchy version is 4.x; continuing anyway"
  fi

  local tp; tp="$(detect_thinkpad)"
  if ! printf '%s' "$tp" | grep -qi 'thinkpad'; then
    die "no ThinkPad detected (DMI:${tp:- unknown}). omafanctrl only supports ThinkPad hardware."
  fi
  ok "ThinkPad detected:$(printf '%s' "$tp" | tr -s ' ')"

  if ! printf '%s' "$tp" | grep -qi 'E14'; then
    warn "this build is tuned for the ThinkPad E14 Gen 4; other models may behave differently"
  fi
}

if [[ "$SKIP_PREFLIGHT" -eq 1 || "${OMAFANCTRL_SKIP_PREFLIGHT:-0}" == "1" ]]; then
  warn "preflight checks skipped"
else
  preflight
fi

# ---------------------------------------------------------------------------
# Dependencies
# ---------------------------------------------------------------------------

install_packages() {
  local -a pkgs=("$@")
  [[ ${#pkgs[@]} -eq 0 ]] && return 0
  info "Installing packages: ${pkgs[*]}"
  pacman -S --needed --noconfirm "${pkgs[@]}"
}

info "Installing runtime dependencies"
install_packages gtk4 libadwaita dbus polkit

if [[ "$PREBUILT" -eq 0 ]]; then
  info "No prebuilt binaries found; building from source"
  install_packages base-devel rust pkgconf git
fi

# ---------------------------------------------------------------------------
# Build (only when there are no prebuilt binaries)
# ---------------------------------------------------------------------------

if [[ "$PREBUILT" -eq 0 ]]; then
  [[ -f "$SCRIPT_DIR/Cargo.toml" ]] \
    || die "no prebuilt binaries and no Cargo.toml; run install.sh from the release zip or the source tree"
  command -v cargo >/dev/null 2>&1 || die "cargo is not available after installing rust"
  info "Building the workspace (release)"
  ( cd "$SCRIPT_DIR" && cargo build --release --locked --workspace )
  BIN_DIR="$SCRIPT_DIR/target/release"
fi

for bin in "$DAEMON_BIN" "$CLI_BIN" "$WAYBAR_BIN" "$GUI_BIN"; do
  [[ -x "$BIN_DIR/$bin" ]] || die "missing binary: $BIN_DIR/$bin"
done

# ---------------------------------------------------------------------------
# Install binaries
# ---------------------------------------------------------------------------

info "Installing binaries to /usr/bin"
install -Dm755 "$BIN_DIR/$DAEMON_BIN" "/usr/bin/$DAEMON_BIN"
install -Dm755 "$BIN_DIR/$CLI_BIN"    "/usr/bin/$CLI_BIN"
install -Dm755 "$BIN_DIR/$WAYBAR_BIN" "/usr/bin/$WAYBAR_BIN"
install -Dm755 "$BIN_DIR/$GUI_BIN"    "/usr/bin/$GUI_BIN"
if [[ -x "$BIN_DIR/$PROBE_BIN" ]]; then
  install -Dm755 "$BIN_DIR/$PROBE_BIN" "/usr/bin/$PROBE_BIN"
fi
ok "binaries installed"

# ---------------------------------------------------------------------------
# Install system integration files
# ---------------------------------------------------------------------------

[[ -d "$DATA_DIR" ]] || die "data directory not found at $DATA_DIR"

info "Installing system integration files"
install -Dm644 "$DATA_DIR/dbus/org.omarchy.omafanctrl.conf" \
  /usr/share/dbus-1/system.d/org.omarchy.omafanctrl.conf
install -Dm644 "$DATA_DIR/polkit/org.omarchy.omafanctrl.policy" \
  /usr/share/polkit-1/actions/org.omarchy.omafanctrl.policy
install -Dm644 "$DATA_DIR/systemd/omafanctrld.service" \
  /usr/lib/systemd/system/omafanctrld.service
install -Dm644 "$DATA_DIR/modules-load.d/ec_sys.conf" \
  /usr/lib/modules-load.d/ec_sys.conf
install -Dm644 "$DATA_DIR/modprobe.d/ec_sys.conf" \
  /usr/lib/modprobe.d/ec_sys.conf
install -Dm644 "$DATA_DIR/desktop/org.omarchy.omafanctrl.desktop" \
  /usr/share/applications/org.omarchy.omafanctrl.desktop
install -Dm644 "$DATA_DIR/icons/org.omarchy.omafanctrl.svg" \
  /usr/share/icons/hicolor/scalable/apps/org.omarchy.omafanctrl.svg

# Default configuration: never clobber an existing user-edited file.
install -d "$CONFIG_DIR"
if [[ -f "$CONFIG_FILE" ]]; then
  ok "keeping existing $CONFIG_FILE"
else
  install -Dm644 "$DATA_DIR/profiles/e14-gen4.ini" "$CONFIG_FILE"
  ok "installed default configuration to $CONFIG_FILE"
fi

# Refresh the desktop/icon caches when the tools are present.
command -v update-desktop-database >/dev/null 2>&1 \
  && update-desktop-database -q /usr/share/applications 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null 2>&1 \
  && gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor 2>/dev/null || true

# ---------------------------------------------------------------------------
# Load the EC module and start the daemon
# ---------------------------------------------------------------------------

info "Loading the ec_sys kernel module with write support"
modprobe -r ec_sys 2>/dev/null || true
modprobe ec_sys write_support=1
if [[ -e /sys/kernel/debug/ec/ec0/io ]]; then
  ok "EC window is available at /sys/kernel/debug/ec/ec0/io"
else
  warn "EC window not found; ensure debugfs is mounted (mount -t debugfs none /sys/kernel/debug)"
fi

info "Reloading D-Bus and systemd"
systemctl reload dbus 2>/dev/null || systemctl restart dbus 2>/dev/null || true
systemctl daemon-reload

info "Enabling and starting the omafanctrld service"
systemctl enable --now omafanctrld \
  || warn "omafanctrld failed to start; check: journalctl -u omafanctrld -e"

# ---------------------------------------------------------------------------
# Per-user desktop integration (Hyprland + Waybar)
# ---------------------------------------------------------------------------

setup_user_integration() {
  if [[ -z "$TARGET_HOME" || ! -d "$TARGET_HOME" ]]; then
    warn "could not determine the desktop user's home; skipping Hyprland/Waybar setup"
    return 0
  fi

  local hypr_dir="$TARGET_HOME/.config/hypr"
  local waybar_dir="$TARGET_HOME/.config/waybar"

  # --- Hyprland bindings -------------------------------------------------
  if [[ -d "$hypr_dir" ]]; then
    local bindings="$hypr_dir/bindings.lua"
    if [[ -f "$bindings" ]] && grep -q 'omafanctrl' "$bindings"; then
      ok "Hyprland bindings already present"
    else
      info "Adding Hyprland keybindings to $bindings"
      # NOTE: bindings.lua is Lua, where '#' is not a comment. Do not emit the
      # shell-style markers here; the appended content already contains
      # "omafanctrl", which is what the idempotency check above greps for.
      {
        printf '\n'
        cat "$DATA_DIR/hyprland/bindings.lua"
      } >> "$bindings"
      chown "$TARGET_USER:$TARGET_USER" "$bindings" 2>/dev/null || true
      ok "Hyprland bindings appended"
    fi
  else
    warn "no ~/.config/hypr directory; skipping Hyprland bindings"
  fi

  # --- Waybar CSS --------------------------------------------------------
  if [[ -d "$waybar_dir" ]]; then
    local css="$waybar_dir/style.css"
    if [[ -f "$css" ]] && grep -q 'custom-omafanctrl' "$css"; then
      ok "Waybar styling already present"
    else
      info "Adding Waybar styling to $css"
      {
        printf '\n%s\n' "$MARKER_WAYBAR"
        cat "$DATA_DIR/waybar/omafanctrl.css"
      } >> "$css"
      chown "$TARGET_USER:$TARGET_USER" "$css" 2>/dev/null || true
      ok "Waybar styling appended"
    fi

    # --- Waybar module ---------------------------------------------------
    local wconf=""
    for candidate in "$waybar_dir/config.jsonc" "$waybar_dir/config"; do
      [[ -f "$candidate" ]] && { wconf="$candidate"; break; }
    done
    if [[ -z "$wconf" ]]; then
      warn "no Waybar config found; add the module from data/waybar/omafanctrl.jsonc manually"
    elif grep -q 'custom/omafanctrl' "$wconf"; then
      ok "Waybar module already configured"
    elif command -v python3 >/dev/null 2>&1; then
      info "Merging the Waybar module into $wconf"
      cp -a "$wconf" "$wconf.omafanctrl.bak"
      if python3 - "$wconf" <<'PY'
import re, sys

path = sys.argv[1]
with open(path, "r", encoding="utf-8") as fh:
    text = fh.read()

if "custom/omafanctrl" in text:
    sys.exit(0)

module = '''  "custom/omafanctrl": {
    "exec": "omafanctrl-waybar --show icon,mode,rpm,temp --icon \\"\\"",
    "return-type": "json",
    "interval": 2,
    "format": "{}",
    "tooltip": true,
    "on-click": "omafanctrl mode cycle --notify",
    "on-click-right": "omafanctrl toggle --notify",
    "on-scroll-up": "omafanctrl level up --notify",
    "on-scroll-down": "omafanctrl level down --notify"
  }'''

# Add the module to modules-right (create the key if it is missing).
m = re.search(r'("modules-right"\s*:\s*\[)', text)
if m:
    after = text[m.end():]
    if re.match(r'\s*\]', after):
        text = text[:m.end()] + '\n    "custom/omafanctrl"\n  ' + after
    else:
        text = text[:m.end()] + '\n    "custom/omafanctrl",' + after
else:
    # Insert a modules-right array right after the opening brace.
    brace = text.find("{")
    if brace == -1:
        sys.exit(1)
    text = text[:brace + 1] + '\n  "modules-right": ["custom/omafanctrl"],' + text[brace + 1:]

# Insert the module definition before the final closing brace, making sure the
# preceding property is comma-terminated.
close = text.rfind("}")
if close == -1:
    sys.exit(1)
head = text[:close]
tail = text[close:]
stripped = head.rstrip()
if stripped and stripped[-1] not in "{,":
    head = stripped + ",\n"
text = head + module + "\n" + tail

with open(path, "w", encoding="utf-8") as fh:
    fh.write(text)
PY
      then
        chown "$TARGET_USER:$TARGET_USER" "$wconf" 2>/dev/null || true
        ok "Waybar module merged (backup at $wconf.omafanctrl.bak)"
      else
        warn "could not merge the Waybar config automatically; see data/waybar/omafanctrl.jsonc"
      fi
    else
      warn "python3 not found; add the module from data/waybar/omafanctrl.jsonc manually"
    fi
  else
    warn "no ~/.config/waybar directory; skipping Waybar setup"
  fi
}

if [[ "$DO_USER_SETUP" -eq 1 ]]; then
  info "Configuring the desktop integration for ${TARGET_USER:-<unknown>}"
  setup_user_integration
else
  info "Skipping per-user integration (--no-user-setup)"
fi

# ---------------------------------------------------------------------------
# Verify
# ---------------------------------------------------------------------------

info "Verifying the installation"
sleep 1
if systemctl is-active --quiet omafanctrld; then
  ok "omafanctrld is active"
else
  warn "omafanctrld is not active; check: journalctl -u omafanctrld -e"
fi
if "$CLI_BIN" status >/dev/null 2>&1; then
  ok "CLI can reach the daemon"
else
  warn "CLI could not reach the daemon yet; it may need a moment to start"
fi

cat <<EOF

${C_GREEN}${C_BOLD}omafanctrl is installed.${C_RESET}

  Daemon   : systemctl status omafanctrld
  CLI      : omafanctrl status
  GUI      : omafanctrl-gui
  Waybar   : omafanctrl-waybar --show icon,mode,rpm,temp
  Config   : $CONFIG_FILE

Reload Hyprland to pick up the new keybindings (SUPER + F1..F3).
EOF
