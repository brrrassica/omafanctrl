# omarchy-shell bar module

Omarchy Quattro (4.x) replaced Waybar with `omarchy-shell`, a single long-running
Quickshell process. The bar is the first-party `omarchy.bar` plugin, configured
from `~/.config/omarchy/shell.json`.

`omafanctrl-status` is a **custom command module**: the bar runs its `exec` on an
`interval` and parses the "Waybar-style JSON" it prints (`text`, `tooltip`,
`class`/`alt`).

## Add the module

Run the helper, which merges the entry into `bar.layout.right` idempotently and
seeds `shell.json` from the Omarchy defaults when it does not exist yet:

```sh
data/omarchy-shell/install-module.sh
```

Or add the entry from [`omafanctrl.json`](omafanctrl.json) to
`bar.layout.<left|center|right>` in `~/.config/omarchy/shell.json` by hand:

```json
{
  "id": "omafanctrl",
  "type": "command",
  "exec": "omafanctrl-status --show icon,mode,rpm,temp",
  "interval": 2,
  "tooltip": "Fan control",
  "onClick": "omafanctrl mode cycle --notify",
  "onRightClick": "omafanctrl toggle --notify",
  "onMiddleClick": "omafanctrl status --notify"
}
```

The shell hot-reloads `shell.json` on save. If the module does not appear, run
`omarchy restart shell`.

## Interactions

| Action | Command |
| --- | --- |
| Left click | `omafanctrl mode cycle --notify` |
| Right click | `omafanctrl toggle --notify` |
| Middle click | `omafanctrl status --notify` |

Custom command modules support `onClick`, `onRightClick`, and `onMiddleClick`
only — there are no scroll handlers, so `level up`/`level down` remain CLI and
Hyprland-hotkey actions.

## Notes

- `exec` runs via `bash -lc`, so `omafanctrl-status` must be on `PATH`
  (`/usr/bin`).
- `interval` is in seconds.
- The bar only special-cases `class: "active"` for accent styling; the module's
  per-mode `class` (`bios`/`manual`/`smart`/`error`) is informational.
