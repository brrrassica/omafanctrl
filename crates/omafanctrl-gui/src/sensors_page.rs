//! The Sensors page: enable/disable and rename each sensor.
//!
//! The page is flat — one [`adw::EntryRow`] per sensor, with the sensor name as
//! the title, the display name as the editable text, and an enable switch as a
//! suffix — matching the flat row style of the dashboard.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use omafanctrl_core::config::Config;
use omafanctrl_core::dbus::State;
use omafanctrl_core::engine::sensor_display_name;

use crate::state::AppState;

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

        for row in self.rows.borrow_mut().drain(..) {
            self.group.remove(&row);
        }
        let config = self.state.config.borrow().clone();
        for (name, offset) in sensors {
            let ignored = config
                .as_ref()
                .is_some_and(|config| config.sensors.ignore.iter().any(|entry| entry == &name));

            let row = adw::EntryRow::builder()
                .title(sensor_display_name(offset))
                .build();
            if let Some(config) = &config {
                if let Some((_, display)) = config
                    .sensors
                    .names
                    .iter()
                    .find(|(_, display)| display.as_str() == name)
                {
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
            let name_for_switch = name.clone();
            switch.connect_active_notify(move |switch| {
                let config = state_for_switch.config.borrow().clone();
                if let Some(mut config) = config {
                    if switch.is_active() {
                        config
                            .sensors
                            .ignore
                            .retain(|entry| entry != &name_for_switch);
                    } else if !config.sensors.ignore.contains(&name_for_switch) {
                        config.sensors.ignore.push(name_for_switch.clone());
                    }
                    state_for_switch.set_config(&config);
                }
            });

            let state_for_entry = self.state.clone();
            let name_for_entry = name.clone();
            row.connect_apply(move |row| {
                let config = state_for_entry.config.borrow().clone();
                if let Some(mut config) = config {
                    let display = row.text().to_string();
                    if let Some((sequence, _)) = config
                        .sensors
                        .names
                        .iter()
                        .find(|(_, value)| value.as_str() == name_for_entry)
                        .map(|(sequence, value)| (*sequence, value.clone()))
                    {
                        config.sensors.names.insert(sequence, display);
                        state_for_entry.set_config(&config);
                    }
                }
            });

            self.group.add(&row);
            self.rows.borrow_mut().push(row.upcast());
        }
    }

    /// Apply a configuration (unused).
    pub fn apply_config(&self, _config: &Config) {}
}
