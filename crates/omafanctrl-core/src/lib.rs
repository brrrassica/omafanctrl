//! Core library for `omafanctrl`.
//!
//! This crate is the single source of truth for the Embedded Controller (EC)
//! protocol, the configuration model, and the fan control engine. It is shared by
//! the daemon, the GUI, and the CLI.
//!
//! The design mirrors the internal behavior of
//! [TPFanCtrl2](https://github.com/Shuzhengz/TPFanCtrl2): read EC temperature
//! sensors, compute a target fan level, and write the fan control register.

pub mod config;
pub mod ec;
pub mod engine;

/// The crate version, sourced from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
