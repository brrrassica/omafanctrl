-- omafanctrl Hyprland bindings for Omarchy Quattro (4.x)
--
-- Omarchy Quattro configures Hyprland through Lua. Personal keybinding
-- overrides live in ~/.config/hypr/bindings.lua, which is loaded by
-- ~/.config/hypr/hyprland.lua via `require("hypr.bindings")`.
--
-- Append the lines below to ~/.config/hypr/bindings.lua. Do NOT replace the
-- file: it may already contain your own overrides. The `o.bind` helper takes
-- (keys, description, command); the description is shown by
-- `omarchy menu keybindings --print`.
--
-- The commands are intentionally short-lived and never block the compositor:
-- the CLI applies a D-Bus call timeout and fails fast if the daemon is
-- unresponsive.

-- Cycle BIOS -> Manual -> Smart.
o.bind("SUPER + F1", "Fan: cycle mode", "omafanctrl mode cycle --notify")

-- Toggle between BIOS and the last non-BIOS mode.
o.bind("SUPER + SHIFT + F1", "Fan: toggle mode", "omafanctrl toggle --notify")

-- Step the manual fan level up and down.
o.bind("SUPER + F2", "Fan: level up", "omafanctrl level up --notify")
o.bind("SUPER + SHIFT + F2", "Fan: level down", "omafanctrl level down --notify")

-- Show the current state as a notification.
o.bind("SUPER + F3", "Fan: status", "omafanctrl status --notify")