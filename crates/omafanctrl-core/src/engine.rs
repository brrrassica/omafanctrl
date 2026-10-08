//! Fan control engine.
//!
//! Implements the three TPFanCtrl2 control modes:
//!
//! - **BIOS** — the firmware manages the fan.
//! - **Manual** — a fixed fan level is applied.
//! - **Smart** — the maximum non-ignored sensor temperature is mapped to a fan
//!   level, with hysteresis to avoid oscillation.
//!
//! Implementation lands in milestone **M3**.

/// The active fan control mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The firmware manages the fan.
    Bios,
    /// A fixed manual fan level is applied.
    Manual,
    /// The fan level is derived from sensor temperatures.
    Smart,
}
