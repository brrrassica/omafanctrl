//! The Settings page, hosted in an [`adw::PreferencesDialog`] opened from the
//! header menu button.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use omafanctrl_core::config::Config;
use omafanctrl_core::dbus::State;

use crate::client::Command;
use crate::state::AppState;

/// The Settings page and the dialog that presents it.
pub struct SettingsPage {
    dialog: adw::PreferencesDialog,
    cycle_row: adw::SpinRow,
    start_minimized_row: adw::SwitchRow,
    config_path_row: adw::ActionRow,
    updating: Rc<Cell<bool>>,
}

impl SettingsPage {
    /// Build the page and its dialog.
    pub fn new(state: &AppState) -> Self {
        let page = adw::PreferencesPage::new();
        let updating = Rc::new(Cell::new(false));

        let group = adw::PreferencesGroup::new();
        group.set_title("Daemon");

        let adjustment = gtk::Adjustment::new(5.0, 1.0, 60.0, 1.0, 5.0, 0.0);
        let cycle_row = adw::SpinRow::new(Some(&adjustment), 1.0, 0);
        cycle_row.set_title("Cycle interval");
        cycle_row.set_subtitle("seconds between sensor reads");

        let start_minimized_row = adw::SwitchRow::builder().title("Start minimized").build();

        let config_path_row = adw::ActionRow::builder().title("Configuration").build();

        let reload_row = adw::ActionRow::builder()
            .title("Reload configuration")
            .subtitle("re-read the file from disk")
            .build();
        let reload_button = gtk::Button::with_label("Reload");
        reload_button.add_css_class("flat");
        reload_button.set_valign(gtk::Align::Center);
        reload_row.add_suffix(&reload_button);
        reload_row.set_activatable_widget(Some(&reload_button));

        group.add(&cycle_row);
        group.add(&start_minimized_row);
        group.add(&config_path_row);
        group.add(&reload_row);
        page.add(&group);

        let state_for_reload = state.clone();
        reload_button.connect_clicked(move |_| {
            state_for_reload.handle.send(Command::ReloadConfig);
            state_for_reload.toast("Reloading configuration…");
        });

        let state_for_cycle = state.clone();
        let updating_for_cycle = Rc::clone(&updating);
        cycle_row.connect_value_notify(move |row| {
            if updating_for_cycle.get() {
                return;
            }
            let config = state_for_cycle.config.borrow().clone();
            if let Some(mut config) = config {
                config.general.cycle = row.value().round().max(1.0) as u32;
                state_for_cycle.set_config(&config);
            }
        });

        let state_for_start = state.clone();
        let updating_for_start = Rc::clone(&updating);
        start_minimized_row.connect_active_notify(move |row| {
            if updating_for_start.get() {
                return;
            }
            let config = state_for_start.config.borrow().clone();
            if let Some(mut config) = config {
                config.general.start_minimized = row.is_active();
                state_for_start.set_config(&config);
            }
        });

        let dialog = adw::PreferencesDialog::new();
        dialog.set_title("Settings");
        dialog.add(&page);

        Self {
            dialog,
            cycle_row,
            start_minimized_row,
            config_path_row,
            updating,
        }
    }

    /// The dialog that presents the settings.
    pub fn dialog(&self) -> &adw::PreferencesDialog {
        &self.dialog
    }

    /// Apply a configuration.
    pub fn apply_config(&self, config: &Config) {
        self.updating.set(true);
        self.cycle_row.set_value(f64::from(config.general.cycle));
        self.start_minimized_row
            .set_active(config.general.start_minimized);
        self.updating.set(false);
    }

    /// Apply a state snapshot.
    pub fn apply_state(&self, state: &State) {
        self.config_path_row.set_subtitle(&state.config_path);
    }
}
