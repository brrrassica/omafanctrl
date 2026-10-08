//! `omafanctrl` — the CLI client for the `omafanctrl` daemon.
//!
//! A first-class D-Bus client for scripting, headless use, and Hyprland
//! `SUPER + <key>` hotkeys that switch fan modes.
//!
//! Implementation lands in milestone **M5**.

fn main() {
    println!("omafanctrl {} (scaffold)", omafanctrl_core::VERSION);
}
