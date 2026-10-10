#!/usr/bin/env bash
#
# Add the omafanctrl status module to the Omarchy Quattro bar.
#
# Merges a `type: "command"` entry into the `bar.layout.<section>` array of
# ~/.config/omarchy/shell.json, seeding the file from the Omarchy defaults when
# it does not exist yet. The default section is `center`, where the entry is
# inserted at the front so it sits left of the clock anchor (and left of the
# `omarchy.indicators` hidden controls). Idempotent: re-running never
# duplicates the entry, and moves it if it is already in another section.
#
# Usage:
#   install-module.sh [--config <path>] [--defaults <path>] [--section <left|center|right>]
#
# The shell hot-reloads shell.json on save; run `omarchy restart shell` if the
# module does not appear.

set -euo pipefail

config="${XDG_CONFIG_HOME:-$HOME/.config}/omarchy/shell.json"
defaults="${OMARCHY_PATH:-/usr/share/omarchy}/config/omarchy/shell.json"
section="center"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --config) config="$2"; shift 2 ;;
    --defaults) defaults="$2"; shift 2 ;;
    --section) section="$2"; shift 2 ;;
    -h | --help) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

case "$section" in
  left | center | right) ;;
  *) echo "section must be left, center, or right" >&2; exit 2 ;;
esac

module='{"id":"omafanctrl","type":"command","exec":"omafanctrl-status --show cpu,rpm","interval":2,"tooltip":"Fan control","onClick":"omafanctrl mode cycle --notify","onRightClick":"omafanctrl toggle --notify","onMiddleClick":"omafanctrl status --notify"}'

mkdir -p "$(dirname "$config")"

# Seed from the Omarchy defaults when the user has no shell.json yet. The shell
# does not deep-merge, so starting from the defaults preserves the stock layout.
if [[ ! -s "$config" ]]; then
  if [[ -s "$defaults" ]]; then
    cp "$defaults" "$config"
  else
    printf '{"version":1,"bar":{"layout":{"left":[],"center":[],"right":[]}},"plugins":[]}\n' >"$config"
  fi
fi

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

if command -v jq >/dev/null 2>&1; then
  jq -S -e --argjson module "$module" --arg section "$section" '
    def object_or_empty: if type == "object" then . else {} end;
    def array_or_empty: if type == "array" then . else [] end;
    def entry_id: if type == "object" then (.id // "") else tostring end;
    def without_omafanctrl: [.[] | select(entry_id != "omafanctrl")];
    object_or_empty
    | .version = 1
    | .bar = (.bar | object_or_empty)
    | .bar.layout = (.bar.layout | object_or_empty)
    | .bar.layout.left = (.bar.layout.left | array_or_empty)
    | .bar.layout.center = (.bar.layout.center | array_or_empty)
    | .bar.layout.right = (.bar.layout.right | array_or_empty)
    | .plugins = (.plugins | array_or_empty)
    | if ([.bar.layout[$section][] | entry_id] | index("omafanctrl")) != null then .
      else
        .bar.layout.left = (.bar.layout.left | without_omafanctrl)
        | .bar.layout.center = (.bar.layout.center | without_omafanctrl)
        | .bar.layout.right = (.bar.layout.right | without_omafanctrl)
        | if $section == "center"
          then .bar.layout.center = [$module] + .bar.layout.center
          else .bar.layout[$section] = .bar.layout[$section] + [$module]
          end
      end
  ' "$config" >"$tmp"
elif command -v python3 >/dev/null 2>&1; then
  python3 - "$config" "$section" "$module" >"$tmp" <<'PY'
import json
import sys

path, section, module = sys.argv[1], sys.argv[2], json.loads(sys.argv[3])
with open(path, encoding="utf-8") as fh:
    data = json.load(fh)
if not isinstance(data, dict):
    data = {}
data["version"] = 1
bar = data.get("bar") if isinstance(data.get("bar"), dict) else {}
data["bar"] = bar
layout = bar.get("layout") if isinstance(bar.get("layout"), dict) else {}
bar["layout"] = layout
for key in ("left", "center", "right"):
    if not isinstance(layout.get(key), list):
        layout[key] = []


def entry_id(entry):
    if isinstance(entry, dict):
        return str(entry.get("id", ""))
    return str(entry)


if not any(entry_id(entry) == "omafanctrl" for entry in layout[section]):
    for key in ("left", "center", "right"):
        layout[key] = [entry for entry in layout[key] if entry_id(entry) != "omafanctrl"]
    if section == "center":
        layout["center"].insert(0, module)
    else:
        layout[section].append(module)

if not isinstance(data.get("plugins"), list):
    data["plugins"] = []
json.dump(data, sys.stdout, indent=2, sort_keys=True)
sys.stdout.write("\n")
PY
else
  echo "jq or python3 is required to update $config" >&2
  exit 1
fi

mv "$tmp" "$config"
trap - EXIT
echo "omafanctrl module is present in $config"
