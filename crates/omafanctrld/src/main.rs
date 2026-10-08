//! `omafanctrld` — the privileged `omafanctrl` daemon.
//!
//! Runs as a systemd system service, owns the EC file handle, runs the control
//! loop, and exposes the control API over the D-Bus system bus.
//!
//! Implementation lands in milestone **M4**.

fn main() {
    println!("omafanctrld {} (scaffold)", omafanctrl_core::VERSION);
}
