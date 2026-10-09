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

/// Whether a sensor is excluded by the configuration's `IgnoreSensors` list.
///
/// Entries may be sensor names (e.g. `pwr`) or hex register offsets
/// (e.g. `x7d`), matching the engine's ignore semantics.
pub fn sensor_is_ignored(config: Option<&Config>, sensor: &SensorState) -> bool {
    let Some(config) = config else {
        return false;
    };
    let name = sensor.name.to_ascii_lowercase();
    let offset_key = format!("x{:02x}", sensor.offset);
    config.sensors.ignore.iter().any(|entry| {
        let entry = entry.trim().to_ascii_lowercase();
        entry == name || entry == offset_key
    })
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
}
