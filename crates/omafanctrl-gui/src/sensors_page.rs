//! The Sensors page: enable/disable, rename, and ignore sensors.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use omafanctrl_core::config::Config;
use omafanctrl_core::dbus::State;

use crate::state::AppState;

/// The Sensors page.
pub struct SensorsPage {
    root: adw::PreferencesPage,
    group: adw::PreferencesGroup,
    /// Rows currently added to `group`, so they can be removed on rebuild.
    rows: RefCell<Vec<gtk::Widget>>,
    state: AppState,
    known: Rc<RefCell<Vec<String>>>,
}

impl SensorsPage {
    /// Build the page.
    pub fn new(state: &AppState) -> Self {
        let root = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::new();
        group.set_title("Sensors");
        group.set_description(Some(
            "Disable a sensor to exclude it from the smart-mode maximum.",
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
        let mut names: Vec<String> = state
            .temperatures
            .iter()
            .map(|sensor| sensor.name.clone())
            .collect();
        names.sort();
        names.dedup();
        if names.is_empty() || *self.known.borrow() == names {
            return;
        }
        *self.known.borrow_mut() = names.clone();

        for row in self.rows.borrow_mut().drain(..) {
            self.group.remove(&row);
        }
        let config = self.state.config.borrow().clone();
        for name in names {
            let ignored = config
                .as_ref()
                .is_some_and(|config| config.sensors.ignore.iter().any(|entry| entry == &name));
            let expander = adw::ExpanderRow::builder().title(&name).build();

            let switch = adw::SwitchRow::builder().title("Enabled").build();
            switch.set_active(!ignored);
            let state_for_switch = self.state.clone();
            let name_for_switch = name.clone();
            switch.connect_active_notify(move |row| {
                let config = state_for_switch.config.borrow().clone();
                if let Some(mut config) = config {
                    if row.is_active() {
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
            expander.add_row(&switch);

            let entry = adw::EntryRow::builder().title("Display name").build();
            if let Some(config) = &config {
                if let Some((_, display)) = config
                    .sensors
                    .names
                    .iter()
                    .find(|(_, display)| display.as_str() == name)
                {
                    entry.set_text(display);
                }
            }
            let state_for_entry = self.state.clone();
            let name_for_entry = name.clone();
            entry.connect_apply(move |entry| {
                let config = state_for_entry.config.borrow().clone();
                if let Some(mut config) = config {
                    let display = entry.text().to_string();
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
            expander.add_row(&entry);

            self.group.add(&expander);
            self.rows.borrow_mut().push(expander.upcast());
        }
    }

    /// Apply a configuration (unused).
    pub fn apply_config(&self, _config: &Config) {}
}
