//! The Sensors page: enable/disable and rename each sensor.
//!
//! The page is flat — one [`adw::EntryRow`] per sensor, with the sensor name as
//! the title, the display name as the editable text, and an enable switch as a
//! suffix — matching the flat row style of the dashboard.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use omafanctrl_core::config::Config;
use omafanctrl_core::dbus::{SensorState, State};
use omafanctrl_core::engine::{sensor_display_name, sensor_sequence};

use crate::state::{AppState, sensor_is_ignored, set_sensor_ignored};

/// The Sensors page.
pub struct SensorsPage {
    root: adw::PreferencesPage,
    group: adw::PreferencesGroup,
    /// Rows currently added to `group`, so they can be removed on rebuild.
    rows: RefCell<Vec<gtk::Widget>>,
    state: AppState,
    known: Rc<RefCell<Vec<(String, u8)>>>,
}

impl SensorsPage {
    /// Build the page.
    pub fn new(state: &AppState) -> Self {
        let root = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::new();
        group.set_title("Sensors");
        group.set_description(Some(
            "Toggle a sensor to include it in the smart-mode maximum, and edit its display name.",
        ));
        root.add(&group);
        Self {
            root,
            group,
            rows: RefCell::new(Vec::new()),
            state: state.clone(),
            known: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// The underlying widget.
    pub fn widget(&self) -> &adw::PreferencesPage {
        &self.root
    }

    /// Apply a state snapshot.
    pub fn apply_state(&self, state: &State) {
        let mut sensors: Vec<(String, u8)> = state
            .temperatures
            .iter()
            .map(|sensor| (sensor.name.clone(), sensor.offset))
            .collect();
        sensors.sort();
        sensors.dedup();
        if sensors.is_empty() || *self.known.borrow() == sensors {
            return;
        }
        *self.known.borrow_mut() = sensors.clone();
        self.rebuild_rows(&sensors);
    }

    /// Rebuild the rows for the given sensors.
    fn rebuild_rows(&self, sensors: &[(String, u8)]) {
        for row in self.rows.borrow_mut().drain(..) {
            self.group.remove(&row);
        }
        let config = self.state.config.borrow().clone();
        for (name, offset) in sensors {
            let sensor = SensorState {
                name: name.clone(),
                offset: *offset,
                celsius: 0,
            };
            let ignored = sensor_is_ignored(config.as_ref(), &sensor);

            let row = adw::EntryRow::builder()
                .title(sensor_display_name(*offset))
                .build();
            let sequence = sensor_sequence(*offset);
            if let Some(config) = &config {
                let display = config.sensors.names.get(&sequence).or_else(|| {
                    config
                        .sensors
                        .names
                        .values()
                        .find(|display| display.as_str() == name.as_str())
                });
                if let Some(display) = display {
                    row.set_text(display);
                }
            }

            // Enable/disable switch as a suffix.
            let switch = gtk::Switch::new();
            switch.set_active(!ignored);
            switch.set_valign(gtk::Align::Center);
            switch.set_tooltip_text(Some("Include this sensor in the smart-mode maximum"));
            row.add_suffix(&switch);

            let state_for_switch = self.state.clone();
            let sensor_for_switch = sensor.clone();
            switch.connect_active_notify(move |switch| {
                let config = state_for_switch.config.borrow().clone();
                if let Some(mut config) = config {
                    // An active switch means "enabled", i.e. not ignored. This
                    // removes both the name and `x<hex>` offset entries.
                    set_sensor_ignored(&mut config, &sensor_for_switch, !switch.is_active());
                    state_for_switch.set_config(&config);
                }
            });

            let state_for_entry = self.state.clone();
            let name_for_entry = name.clone();
            let offset_for_entry = *offset;
            row.connect_apply(move |row| {
                let config = state_for_entry.config.borrow().clone();
                if let Some(mut config) = config {
                    let display = row.text().to_string();
                    // SensorName entries are keyed by the sensor's stable
                    // sequence, never by the editable display text, so a rename
                    // survives the next rebuild.
                    let sequence = sensor_sequence(offset_for_entry);
                    config.sensors.names.retain(|key, value| {
                        *key == sequence || value.as_str() != name_for_entry.as_str()
                    });
                    config.sensors.names.insert(sequence, display);
                    state_for_entry.set_config(&config);
                }
            });

            self.group.add(&row);
            self.rows.borrow_mut().push(row.upcast());
        }
    }

    /// Rebuild the rows when the configuration changes, so the switches reflect
    /// the current `IgnoreSensors` and `SensorName` values even when the sensor
    /// set has not changed (including the initial State-before-Config update).
    pub fn apply_config(&self, _config: &Config) {
        let sensors = self.known.borrow().clone();
        if sensors.is_empty() {
            return;
        }
        self.rebuild_rows(&sensors);
    }
}
