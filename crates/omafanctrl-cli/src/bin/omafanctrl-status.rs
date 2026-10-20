//! Status module for the Omarchy Quattro bar (`omarchy-shell`).
//!
//! Queries the daemon and prints a JSON object (`text`, `tooltip`, `class`,
//! `alt`) or plain text. The output is the "Waybar-style JSON" contract that
//! `omarchy-shell`'s custom command module parses, so it can be dropped into
//! `~/.config/omarchy/shell.json` as a `type: "command"` bar module. The
//! `class` is the current mode; a daemon that cannot be reached reports
//! `class: "error"`.
//!
//! ```jsonc
//! {
//!   "id": "omafanctrl",
//!   "type": "command",
//!   "exec": "omafanctrl-status --show cpu",
//!   "interval": 2,
//!   "tooltip": "Fan control",
//!   "onClick": "omafanctrl mode cycle --notify",
//!   "onRightClick": "omafanctrl toggle --notify",
//!   "onMiddleClick": "omafanctrl status --notify"
//! }
//! ```

use std::time::Duration;

use clap::Parser;
use omafanctrl_cli::client::Client;
use omafanctrl_core::dbus::State;

/// Command-line arguments for the bar status module.
#[derive(Debug, Parser)]
#[command(
    name = "omafanctrl-status",
    version,
    about = "Status module for the Omarchy Quattro bar"
)]
struct Args {
    /// Fields to show, comma-separated: `icon`, `mode`, `rpm`, `temp`, `cpu`.
    #[arg(long, default_value = "icon,mode")]
    show: String,

    /// Icon glyph shown when `icon` is in `--show`.
    #[arg(long, default_value = "")]
    icon: String,

    /// Output plain text instead of the JSON module contract.
    #[arg(long)]
    plain: bool,

    /// D-Bus call timeout, in seconds.
    #[arg(long, default_value_t = 2)]
    timeout: u64,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = Args::parse();
    let (text, tooltip, class) = match fetch(&args).await {
        Ok(state) => (
            render_text(&state, &args),
            render_tooltip(&state),
            state.mode.clone(),
        ),
        Err(error) => (
            "fan ?".to_string(),
            format!("omafanctrl: {error}"),
            "error".to_string(),
        ),
    };

    if args.plain {
        println!("{text}");
    } else {
        let output = serde_json::json!({
            "text": text,
            "tooltip": tooltip,
            "class": class,
            "alt": class,
        });
        println!("{}", serde_json::to_string(&output).unwrap_or_default());
    }
}

/// Fetch the daemon state.
async fn fetch(args: &Args) -> anyhow::Result<State> {
    let client = Client::connect(Duration::from_secs(args.timeout.max(1))).await?;
    client.get_state().await
}

/// A short, human-readable mode label.
fn mode_label(mode: &str) -> &str {
    match mode {
        "manual" => "Manual",
        "smart" => "Smart",
        _ => "BIOS",
    }
}

/// Render the bar text from the selected fields.
fn render_text(state: &State, args: &Args) -> String {
    let mut parts = Vec::new();
    for field in args.show.split(',').map(str::trim) {
        match field {
            "icon" => {
                if !args.icon.is_empty() {
                    parts.push(args.icon.clone());
                }
            }
            "mode" => parts.push(mode_label(&state.mode).to_string()),
            "rpm" => parts.push(format!("{} RPM", state.fan_rpm)),
            "temp" => {
                if let Some(max) = state.temperatures.iter().map(|sensor| sensor.celsius).max() {
                    parts.push(format!("{max}°C"));
                }
            }
            // The CPU sensor only, so the bar can show a stable CPU temperature
            // instead of whichever sensor happens to be hottest.
            "cpu" => {
                if let Some(sensor) = state
                    .temperatures
                    .iter()
                    .find(|sensor| sensor.name == "cpu")
                {
                    parts.push(format!("{}°C", sensor.celsius));
                }
            }
            _ => {}
        }
    }
    parts.join(" ")
}

/// Render the multi-line tooltip.
///
/// Only the CPU sensor is listed: the daemon exposes every EC sensor, including
/// the ones the profile's `IgnoreSensors` list excludes from fan control, and
/// dumping them all into the tooltip is noise.
fn render_tooltip(state: &State) -> String {
    let fan = if state.bios_auto {
        "BIOS auto".to_string()
    } else {
        format!("level {}", state.current_level)
    };
    let mut lines = vec![
        format!("Mode: {}", mode_label(&state.mode)),
        format!("Fan: {fan}"),
        format!("Speed: {} RPM", state.fan_rpm),
        format!(
            "Hysteresis: {}",
            if state.hysteresis_enabled {
                "on"
            } else {
                "off"
            }
        ),
    ];
    if let Some(sensor) = state
        .temperatures
        .iter()
        .find(|sensor| sensor.name == "cpu")
    {
        lines.push(String::new());
        lines.push(format!("CPU: {} °C", sensor.celsius));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use omafanctrl_core::dbus::SensorState;

    fn sample_state() -> State {
        State {
            mode: "smart".to_string(),
            manual_level: 2,
            current_level: 3,
            bios_auto: false,
            fan_rpm: 3900,
            hysteresis_enabled: true,
            temperatures: vec![
                SensorState {
                    name: "cpu".to_string(),
                    offset: 0x78,
                    celsius: 62,
                },
                SensorState {
                    name: "gpu".to_string(),
                    offset: 0x79,
                    celsius: 55,
                },
            ],
            config_path: "/etc/omafanctrl/TPFanControl.ini".to_string(),
        }
    }

    fn args(show: &str, icon: &str) -> Args {
        Args {
            show: show.to_string(),
            icon: icon.to_string(),
            plain: false,
            timeout: 2,
        }
    }

    #[test]
    fn renders_selected_fields() {
        let state = sample_state();
        assert_eq!(render_text(&state, &args("mode", "")), "Smart");
        assert_eq!(render_text(&state, &args("mode,rpm", "")), "Smart 3900 RPM");
        assert_eq!(render_text(&state, &args("mode,temp", "")), "Smart 62°C");
        assert_eq!(render_text(&state, &args("icon,mode", "FAN")), "FAN Smart");
    }

    #[test]
    fn renders_cpu_temperature_only() {
        let state = sample_state();
        // The CPU sensor (62°C) wins even though the max across sensors is the
        // same here; a hotter GPU must not leak into the `cpu` field.
        assert_eq!(render_text(&state, &args("cpu", "")), "62°C");
        let mut hotter_gpu = sample_state();
        hotter_gpu.temperatures[1].celsius = 90;
        assert_eq!(render_text(&hotter_gpu, &args("cpu", "")), "62°C");
        assert_eq!(render_text(&hotter_gpu, &args("temp", "")), "90°C");
    }

    #[test]
    fn omits_an_empty_icon() {
        let state = sample_state();
        assert_eq!(render_text(&state, &args("icon,mode", "")), "Smart");
    }

    #[test]
    fn labels_modes() {
        assert_eq!(mode_label("bios"), "BIOS");
        assert_eq!(mode_label("manual"), "Manual");
        assert_eq!(mode_label("smart"), "Smart");
        assert_eq!(mode_label("unknown"), "BIOS");
    }

    #[test]
    fn tooltip_shows_only_the_cpu_sensor() {
        let tooltip = render_tooltip(&sample_state());
        assert!(tooltip.contains("Mode: Smart"));
        assert!(tooltip.contains("Speed: 3900 RPM"));
        assert!(tooltip.contains("CPU: 62 °C"));
        // The GPU sensor must not leak into the tooltip.
        assert!(!tooltip.contains("gpu"));
        assert!(!tooltip.contains("GPU"));
    }
}
