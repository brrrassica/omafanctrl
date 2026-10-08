//! Configuration model.
//!
//! Configuration is TPFanCtrl2 `.ini` compatible, with the sections
//! `[General]`, `[Sensors]`, `[FanLevels]`, and `[SmartMode]`.
//!
//! Implementation lands in milestone **M2**.

/// The default configuration file name, matching TPFanCtrl2.
pub const DEFAULT_CONFIG_FILE: &str = "TPFanControl.ini";
