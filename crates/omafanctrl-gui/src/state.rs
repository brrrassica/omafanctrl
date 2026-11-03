//! Shared application state and small helpers used across the GUI pages.

use std::cell::RefCell;
use std::rc::Rc;

use omafanctrl_core::config::Config;
use omafanctrl_core::dbus::SensorState;

use crate::client::{ClientHandle, Command};

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// The background D-Bus client.
    pub handle: ClientHandle,
    /// The most recently loaded configuration.
    pub config: Rc<RefCell<Option<Config>>>,
    /// The overlay used to show toasts.
    pub toast_overlay: adw::ToastOverlay,
}

impl AppState {
    /// Show a toast.
    pub fn toast(&self, message: &str) {
        self.toast_overlay.add_toast(adw::Toast::new(message));
    }

    /// Persist a configuration through the daemon.
    pub fn set_config(&self, config: &Config) {
        *self.config.borrow_mut() = Some(config.clone());
        self.handle.send(Command::SetConfig(config.to_ini()));
    }
}

/// Map a mode name to its combo-row index.
pub fn mode_index(mode: &str) -> u32 {
    match mode {
        "manual" => 1,
        "smart" => 2,
        _ => 0,
    }
}

/// Map a combo-row index to a mode name.
pub fn mode_name(index: u32) -> &'static str {
    match index {
        1 => "manual",
        2 => "smart",
        _ => "bios",
    }
}

/// Whether an `IgnoreSensors` entry refers to the given sensor.
///
/// Entries may be sensor names (e.g. `pwr`) or hex register offsets
/// (e.g. `x7d`), matching the engine's ignore semantics: comparison is
/// case-insensitive and ignores surrounding whitespace.
fn ignore_entry_matches(entry: &str, sensor: &SensorState) -> bool {
    let entry = entry.trim().to_ascii_lowercase();
    entry == sensor.name.to_ascii_lowercase() || entry == format!("x{:02x}", sensor.offset)
}

/// Whether a sensor is excluded by the configuration's `IgnoreSensors` list.
///
/// Entries may be sensor names (e.g. `pwr`) or hex register offsets
/// (e.g. `x7d`), matching the engine's ignore semantics.
pub fn sensor_is_ignored(config: Option<&Config>, sensor: &SensorState) -> bool {
    let Some(config) = config else {
        return false;
    };
    config
        .sensors
        .ignore
        .iter()
        .any(|entry| ignore_entry_matches(entry, sensor))
}

/// Enable or disable a sensor in the configuration's `IgnoreSensors` list.
///
/// Enabling removes *every* matching entry, including the `x<hex>` offset form,
/// so a sensor ignored by offset can actually be re-enabled.
pub fn set_sensor_ignored(config: &mut Config, sensor: &SensorState, ignored: bool) {
    if ignored {
        if !sensor_is_ignored(Some(config), sensor) {
            config.sensors.ignore.push(sensor.name.clone());
        }
    } else {
        config
            .sensors
            .ignore
            .retain(|entry| !ignore_entry_matches(entry, sensor));
    }
}

/// The temperature of the named sensor, if present.
pub fn temperature(sensors: &[SensorState], name: &str) -> Option<f64> {
    sensors
        .iter()
        .find(|sensor| sensor.name == name)
        .map(|sensor| f64::from(sensor.celsius))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_index_and_name_round_trip() {
        for (index, name) in [(0, "bios"), (1, "manual"), (2, "smart")] {
            assert_eq!(mode_index(name), index);
            assert_eq!(mode_name(index), name);
        }
        assert_eq!(mode_index("unknown"), 0);
        assert_eq!(mode_name(99), "bios");
    }

    #[test]
    fn ignored_sensors_are_hidden_by_name_or_offset() {
        let config: Config = "IgnoreSensors=pwr,x7d\n".parse().unwrap();
        let sensor = |name: &str, offset: u8| SensorState {
            name: name.to_string(),
            offset,
            celsius: 40,
        };

        // Ignored by name.
        assert!(sensor_is_ignored(Some(&config), &sensor("pwr", 0xC0)));
        // Ignored by hex register offset.
        assert!(sensor_is_ignored(Some(&config), &sensor("no5", 0x7D)));
        // Not ignored.
        assert!(!sensor_is_ignored(Some(&config), &sensor("cpu", 0x78)));
        // Without a loaded config nothing is hidden.
        assert!(!sensor_is_ignored(None, &sensor("pwr", 0xC0)));
    }

    #[test]
    fn enabling_a_sensor_removes_name_and_offset_entries() {
        let mut config: Config = "IgnoreSensors=pwr,x7d\n".parse().unwrap();

        // Disabled by offset only.
        let offset_ignored = SensorState {
            name: "no5".to_string(),
            offset: 0x7D,
            celsius: 40,
        };
        assert!(sensor_is_ignored(Some(&config), &offset_ignored));
        set_sensor_ignored(&mut config, &offset_ignored, false);
        assert!(!sensor_is_ignored(Some(&config), &offset_ignored));
        assert!(config.sensors.ignore.iter().all(|e| e != "x7d"));

        // Disabled by name only.
        let name_ignored = SensorState {
            name: "pwr".to_string(),
            offset: 0xC0,
            celsius: 40,
        };
        assert!(sensor_is_ignored(Some(&config), &name_ignored));
        set_sensor_ignored(&mut config, &name_ignored, false);
        assert!(!sensor_is_ignored(Some(&config), &name_ignored));

        // Re-disabling adds the name back exactly once.
        set_sensor_ignored(&mut config, &name_ignored, true);
        set_sensor_ignored(&mut config, &name_ignored, true);
        assert!(sensor_is_ignored(Some(&config), &name_ignored));
        assert_eq!(
            config.sensors.ignore.iter().filter(|e| *e == "pwr").count(),
            1
        );
    }
}
