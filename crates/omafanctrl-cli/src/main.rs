//! `omafanctrl` — the CLI client for the `omafanctrl` daemon.
//!
//! A first-class D-Bus client for scripting, headless use, and Hyprland
//! `SUPER + <key>` hotkeys that switch fan modes. Every command is designed to
//! return quickly so it never blocks a keybinding.

mod client;

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use omafanctrl_core::dbus::{State, parse_mode};
use omafanctrl_core::ec::{FAN_LEVEL_MAX, FAN_LEVEL_MIN};
use omafanctrl_core::engine::Mode;
use serde_json::json;

use crate::client::Client;

/// Control the ThinkPad fan through the `omafanctrl` daemon.
#[derive(Debug, Parser)]
#[command(name = "omafanctrl", version, about)]
struct Cli {
    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,

    /// Show a desktop notification after a mode or level change.
    #[arg(long, global = true)]
    notify: bool,

    /// D-Bus call timeout, in seconds.
    #[arg(long, global = true, default_value_t = 5)]
    timeout: u64,

    #[command(subcommand)]
    command: Command,
}

/// The available commands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Show the current fan state.
    Status,
    /// Get or set the fan mode.
    Mode {
        #[command(subcommand)]
        action: Option<ModeAction>,
    },
    /// Get or set the manual fan level.
    Level {
        #[command(subcommand)]
        action: Option<LevelAction>,
    },
    /// Cycle to the next mode (BIOS -> Manual -> Smart).
    Toggle,
    /// Manage the configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Reload the configuration from disk.
    Reload,
    /// Enable, disable, or toggle smart-mode hysteresis.
    Hysteresis {
        #[command(subcommand)]
        action: Option<HysteresisAction>,
    },
}

/// Mode subcommands.
#[derive(Debug, Subcommand)]
enum ModeAction {
    /// Use firmware (BIOS) control.
    Bios,
    /// Apply a fixed manual level.
    Manual,
    /// Derive the level from temperatures.
    Smart,
    /// Cycle to the next mode.
    Cycle,
}

/// Level subcommands.
#[derive(Debug, Subcommand)]
enum LevelAction {
    /// Increase the manual level by one.
    Up,
    /// Decrease the manual level by one.
    Down,
    /// Set an explicit level.
    Set {
        /// The fan level (1-7).
        level: u8,
    },
}

/// Configuration subcommands.
#[derive(Debug, Subcommand)]
enum ConfigAction {
    /// Print the current configuration.
    Get,
    /// Replace the configuration from a file (`-` for stdin).
    Set {
        /// Path to the new configuration, or `-` for stdin.
        path: String,
    },
}

/// Hysteresis subcommands.
#[derive(Debug, Subcommand)]
enum HysteresisAction {
    /// Enable hysteresis.
    On,
    /// Disable hysteresis.
    Off,
    /// Toggle hysteresis.
    Toggle,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let timeout = Duration::from_secs(cli.timeout.max(1));
    let client = Client::connect(timeout).await?;
    run(&cli, &client).await
}

/// Dispatch the parsed command.
async fn run(cli: &Cli, client: &Client) -> Result<()> {
    match &cli.command {
        Command::Status => {
            let state = client.get_state().await?;
            if cli.notify {
                let fan = if state.bios_auto {
                    "BIOS auto".to_string()
                } else {
                    format!("level {}", state.current_level)
                };
                notify(
                    "omafanctrl",
                    &format!("{} · {} RPM · {fan}", state.mode, state.fan_rpm),
                );
            }
            print_state(&state, cli.json)
        }
        Command::Mode { action } => match action {
            None => {
                let state = client.get_state().await?;
                if cli.json {
                    print_json(&json!({ "mode": state.mode }))
                } else {
                    println!("{}", state.mode);
                    Ok(())
                }
            }
            Some(ModeAction::Cycle) => cycle_mode(cli, client).await,
            Some(action) => {
                let mode = match action {
                    ModeAction::Bios => "bios",
                    ModeAction::Manual => "manual",
                    ModeAction::Smart => "smart",
                    ModeAction::Cycle => unreachable!("handled above"),
                };
                client.set_mode(mode).await?;
                after_change(cli, client, &format!("mode: {mode}")).await
            }
        },
        Command::Toggle => cycle_mode(cli, client).await,
        Command::Level { action } => match action {
            None => {
                let state = client.get_state().await?;
                if cli.json {
                    print_json(&json!({ "level": state.manual_level }))
                } else {
                    println!("{}", state.manual_level);
                    Ok(())
                }
            }
            Some(LevelAction::Set { level }) => {
                let level = validate_level(*level)?;
                client.set_manual_level(level).await?;
                client.set_mode("manual").await?;
                after_change(cli, client, &format!("level: {level}")).await
            }
            Some(LevelAction::Up) => adjust_level(cli, client, 1).await,
            Some(LevelAction::Down) => adjust_level(cli, client, -1).await,
        },
        Command::Config { action } => match action {
            ConfigAction::Get => {
                let config = client.get_config().await?;
                if cli.json {
                    print_json(&json!({ "config": config }))
                } else {
                    print!("{config}");
                    Ok(())
                }
            }
            ConfigAction::Set { path } => {
                let contents = read_config(path)?;
                client.set_config(&contents).await?;
                if cli.json {
                    print_json(&json!({ "ok": true }))
                } else {
                    println!("configuration updated");
                    Ok(())
                }
            }
        },
        Command::Reload => {
            client.reload_config().await?;
            if cli.json {
                print_json(&json!({ "ok": true }))
            } else {
                println!("configuration reloaded");
                Ok(())
            }
        }
        Command::Hysteresis { action } => {
            let enabled = match action {
                None => {
                    let state = client.get_state().await?;
                    return if cli.json {
                        print_json(&json!({ "hysteresis": state.hysteresis_enabled }))
                    } else {
                        println!("{}", state.hysteresis_enabled);
                        Ok(())
                    };
                }
                Some(HysteresisAction::On) => true,
                Some(HysteresisAction::Off) => false,
                Some(HysteresisAction::Toggle) => !client.get_state().await?.hysteresis_enabled,
            };
            client.set_hysteresis(enabled).await?;
            if cli.json {
                print_json(&json!({ "hysteresis": enabled }))
            } else {
                println!(
                    "hysteresis {}",
                    if enabled { "enabled" } else { "disabled" }
                );
                Ok(())
            }
        }
    }
}

/// Cycle BIOS -> Manual -> Smart.
async fn cycle_mode(cli: &Cli, client: &Client) -> Result<()> {
    let state = client.get_state().await?;
    let current = parse_mode(&state.mode).unwrap_or(Mode::Bios);
    let next = current.next();
    client.set_mode(next.as_str()).await?;
    after_change(cli, client, &format!("mode: {}", next.as_str())).await
}

/// Adjust the manual level by `delta`, switching to Manual mode.
async fn adjust_level(cli: &Cli, client: &Client, delta: i8) -> Result<()> {
    let state = client.get_state().await?;
    let base = if state.bios_auto {
        state.manual_level
    } else {
        state.current_level
    }
    .max(FAN_LEVEL_MIN);
    let next = if delta >= 0 {
        base.saturating_add(delta as u8).min(FAN_LEVEL_MAX)
    } else {
        base.saturating_sub(delta.unsigned_abs()).max(FAN_LEVEL_MIN)
    };
    client.set_manual_level(next).await?;
    client.set_mode("manual").await?;
    after_change(cli, client, &format!("level: {next}")).await
}

/// Print the outcome of a change and optionally notify.
async fn after_change(cli: &Cli, client: &Client, human: &str) -> Result<()> {
    if cli.json {
        let state = client.get_state().await?;
        print_json(&state)?;
    } else {
        println!("{human}");
    }
    if cli.notify {
        notify("omafanctrl", human);
    }
    Ok(())
}

/// Validate a fan level.
fn validate_level(level: u8) -> Result<u8> {
    if (FAN_LEVEL_MIN..=FAN_LEVEL_MAX).contains(&level) {
        Ok(level)
    } else {
        bail!("fan level {level} is out of range {FAN_LEVEL_MIN}-{FAN_LEVEL_MAX}")
    }
}

/// Read a configuration file, or stdin when `path` is `-`.
fn read_config(path: &str) -> Result<String> {
    if path == "-" {
        let mut contents = String::new();
        std::io::stdin()
            .read_to_string(&mut contents)
            .context("failed to read the configuration from stdin")?;
        Ok(contents)
    } else {
        std::fs::read_to_string(Path::new(path)).with_context(|| format!("failed to read `{path}`"))
    }
}

/// Print a state snapshot.
fn print_state(state: &State, json: bool) -> Result<()> {
    if json {
        return print_json(state);
    }
    let fan = if state.bios_auto {
        "BIOS auto".to_string()
    } else {
        format!("level {}", state.current_level)
    };
    println!("Mode:         {}", state.mode);
    println!("Fan:          {fan}");
    println!("Manual level: {}", state.manual_level);
    println!("RPM:          {}", state.fan_rpm);
    println!(
        "Hysteresis:   {}",
        if state.hysteresis_enabled {
            "on"
        } else {
            "off"
        }
    );
    println!("Config:       {}", state.config_path);
    if !state.temperatures.is_empty() {
        println!("Temperatures:");
        for sensor in &state.temperatures {
            println!(
                "  {:<6} 0x{:02X}  {} °C",
                sensor.name, sensor.offset, sensor.celsius
            );
        }
    }
    Ok(())
}

/// Print a value as pretty JSON.
fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Send a desktop notification, ignoring any failure.
fn notify(summary: &str, body: &str) {
    let _ = std::process::Command::new("notify-send")
        .arg("--app-name=omafanctrl")
        .arg("--icon=preferences-system")
        .arg(summary)
        .arg(body)
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_levels() {
        assert_eq!(validate_level(1).unwrap(), 1);
        assert_eq!(validate_level(7).unwrap(), 7);
        assert!(validate_level(0).is_err());
        assert!(validate_level(8).is_err());
    }

    #[test]
    fn cli_parses_hotkey_commands() {
        let cli = Cli::try_parse_from(["omafanctrl", "toggle", "--notify"]).unwrap();
        assert!(cli.notify);
        assert!(matches!(cli.command, Command::Toggle));

        let cli = Cli::try_parse_from(["omafanctrl", "mode", "cycle"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Mode {
                action: Some(ModeAction::Cycle)
            }
        ));

        let cli = Cli::try_parse_from(["omafanctrl", "level", "up"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Level {
                action: Some(LevelAction::Up)
            }
        ));

        let cli = Cli::try_parse_from(["omafanctrl", "level", "set", "3"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Level {
                action: Some(LevelAction::Set { level: 3 })
            }
        ));

        let cli = Cli::try_parse_from(["omafanctrl", "config", "get", "--json"]).unwrap();
        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Command::Config {
                action: ConfigAction::Get
            }
        ));
    }
}
