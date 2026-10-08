//! `omafanctrl-gui` — the GTK4 + libadwaita desktop application.
//!
//! An adaptive, Wayland-native front end for the `omafanctrl` daemon. The shell
//! is an [`adw::NavigationSplitView`] (sidebar + content) that collapses to a
//! single column at narrow tiling widths. Live data arrives from the daemon's
//! `StateChanged`/`ConfigChanged` signals via a background D-Bus client.

mod chart;
mod client;
mod curve;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use omafanctrl_core::config::{Config, SmartMode};
use omafanctrl_core::dbus::State;

use crate::chart::TemperatureChart;
use crate::client::{ClientHandle, Command, Update};
use crate::curve::CurveEditor;

/// The application ID, matching the D-Bus name.
const APP_ID: &str = "org.omarchy.omafanctrl";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

/// Map a mode name to its combo-row index.
fn mode_index(mode: &str) -> u32 {
    match mode {
        "manual" => 1,
        "smart" => 2,
        _ => 0,
    }
}

/// Map a combo-row index to a mode name.
fn mode_name(index: u32) -> &'static str {
    match index {
        1 => "manual",
        2 => "smart",
        _ => "bios",
    }
}

/// Shared application state.
#[derive(Clone)]
struct AppState {
    handle: ClientHandle,
    config: Rc<RefCell<Option<Config>>>,
    toast_overlay: adw::ToastOverlay,
}

impl AppState {
    /// Show a toast.
    fn toast(&self, message: &str) {
        self.toast_overlay.add_toast(adw::Toast::new(message));
    }

    /// Persist a configuration through the daemon.
    fn set_config(&self, config: &Config) {
        *self.config.borrow_mut() = Some(config.clone());
        self.handle.send(Command::SetConfig(config.to_ini()));
    }
}

/// Build the application window.
fn build_ui(app: &adw::Application) {
    let handle = client::spawn();
    let toast_overlay = adw::ToastOverlay::new();
    let state = AppState {
        handle: handle.clone(),
        config: Rc::new(RefCell::new(None)),
        toast_overlay: toast_overlay.clone(),
    };

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("omafanctrl")
        .default_width(920)
        .default_height(660)
        .build();

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);

    let overview = Rc::new(OverviewPage::new(&state));
    let curve_page = Rc::new(CurvePage::new(&state));
    let sensors_page = Rc::new(SensorsPage::new(&state));
    let settings_page = Rc::new(SettingsPage::new(&state));

    stack.add_named(overview.widget(), Some("overview"));
    stack.add_named(curve_page.widget(), Some("curve"));
    stack.add_named(sensors_page.widget(), Some("sensors"));
    stack.add_named(settings_page.widget(), Some("settings"));

    let status = adw::StatusPage::builder()
        .icon_name("dialog-warning-symbolic")
        .title("Daemon unavailable")
        .description(
            "The omafanctrl daemon is not running. Start omafanctrld and reopen this window.",
        )
        .build();
    stack.add_named(&status, Some("error"));

    // Sidebar.
    let sidebar = gtk::ListBox::new();
    sidebar.set_selection_mode(gtk::SelectionMode::Single);
    sidebar.add_css_class("navigation-sidebar");
    let entries = [
        ("overview", "Overview", "utilities-system-monitor-symbolic"),
        ("curve", "Smart Curve", "office-chart-line-symbolic"),
        ("sensors", "Sensors", "sensors-applet-symbolic"),
        ("settings", "Settings", "preferences-system-symbolic"),
    ];
    for (_, title, icon) in entries {
        let row = adw::ActionRow::builder().title(title).build();
        row.add_prefix(&gtk::Image::from_icon_name(icon));
        sidebar.append(&row);
    }

    let content_page = adw::NavigationPage::new(&stack, "Overview");
    let sidebar_page = adw::NavigationPage::new(&sidebar, "omafanctrl");

    let split = adw::NavigationSplitView::new();
    split.set_sidebar(Some(&sidebar_page));
    split.set_content(Some(&content_page));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&split));

    toast_overlay.set_child(Some(&toolbar));
    window.set_content(Some(&toast_overlay));

    // Sidebar selection drives the content stack.
    let stack_for_select = stack.clone();
    let content_page_for_select = content_page.clone();
    sidebar.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            let index = row.index().max(0) as usize;
            if let Some((name, title, _)) = entries.get(index) {
                stack_for_select.set_visible_child_name(name);
                content_page_for_select.set_title(title);
            }
        }
    });
    if let Some(row) = sidebar.row_at_index(0) {
        sidebar.select_row(Some(&row));
    }

    // Drain updates on the GTK main loop.
    let updates = handle.updates.clone();
    let state_for_updates = state.clone();
    let stack_for_updates = stack.clone();
    glib::timeout_add_local(Duration::from_millis(200), move || {
        while let Ok(update) = updates.try_recv() {
            match update {
                Update::State(snapshot) => {
                    overview.apply_state(&snapshot);
                    curve_page.apply_state(&snapshot);
                    sensors_page.apply_state(&snapshot);
                    settings_page.apply_state(&snapshot);
                }
                Update::Config(text) => match text.parse::<Config>() {
                    Ok(config) => {
                        *state_for_updates.config.borrow_mut() = Some(config.clone());
                        curve_page.apply_config(&config);
                        sensors_page.apply_config(&config);
                        settings_page.apply_config(&config);
                    }
                    Err(error) => {
                        state_for_updates.toast(&format!("invalid configuration: {error}"));
                    }
                },
                Update::Error(message) => {
                    stack_for_updates.set_visible_child_name("error");
                    state_for_updates.toast(&message);
                }
            }
        }
        glib::ControlFlow::Continue
    });

    window.present();
}

/// The Overview page: live temperatures, RPM, and the mode switch.
struct OverviewPage {
    root: adw::PreferencesPage,
    mode_row: adw::ComboRow,
    manual_row: adw::SpinRow,
    fan_row: adw::ActionRow,
    rpm_row: adw::ActionRow,
    hysteresis_row: adw::SwitchRow,
    temps_group: adw::PreferencesGroup,
    /// Rows currently added to `temps_group`, so they can be removed on rebuild.
    temp_rows: RefCell<Vec<gtk::Widget>>,
    chart: TemperatureChart,
    updating: Rc<Cell<bool>>,
}

impl OverviewPage {
    fn new(state: &AppState) -> Self {
        let root = adw::PreferencesPage::new();
        let updating = Rc::new(Cell::new(false));

        let status_group = adw::PreferencesGroup::new();
        status_group.set_title("Status");
        let fan_row = adw::ActionRow::builder().title("Fan").build();
        let rpm_row = adw::ActionRow::builder().title("Speed").build();
        let hysteresis_row = adw::SwitchRow::builder().title("Hysteresis").build();
        status_group.add(&fan_row);
        status_group.add(&rpm_row);
        status_group.add(&hysteresis_row);

        let mode_group = adw::PreferencesGroup::new();
        mode_group.set_title("Mode");
        let mode_row = adw::ComboRow::builder()
            .title("Control mode")
            .model(&gtk::StringList::new(&["BIOS", "Manual", "Smart"]))
            .build();
        mode_group.add(&mode_row);

        let manual_adjustment = gtk::Adjustment::new(1.0, 1.0, 7.0, 1.0, 1.0, 0.0);
        let manual_row = adw::SpinRow::new(Some(&manual_adjustment), 1.0, 0);
        manual_row.set_title("Manual level");
        manual_row.set_subtitle("applied in Manual mode");
        mode_group.add(&manual_row);

        let chart_group = adw::PreferencesGroup::new();
        chart_group.set_title("Temperature history");
        let chart = TemperatureChart::new();
        chart_group.add(chart.widget());

        let temps_group = adw::PreferencesGroup::new();
        temps_group.set_title("Temperatures");

        root.add(&status_group);
        root.add(&mode_group);
        root.add(&chart_group);
        root.add(&temps_group);

        let state_for_mode = state.clone();
        let updating_for_mode = Rc::clone(&updating);
        mode_row.connect_selected_notify(move |row| {
            if updating_for_mode.get() {
                return;
            }
            state_for_mode
                .handle
                .send(Command::SetMode(mode_name(row.selected()).to_string()));
        });

        let state_for_manual = state.clone();
        let updating_for_manual = Rc::clone(&updating);
        manual_row.connect_value_notify(move |row| {
            if updating_for_manual.get() {
                return;
            }
            state_for_manual.handle.send(Command::SetManualLevel(
                row.value().round().clamp(1.0, 7.0) as u8,
            ));
        });

        let state_for_hysteresis = state.clone();
        let updating_for_hysteresis = Rc::clone(&updating);
        hysteresis_row.connect_active_notify(move |row| {
            if updating_for_hysteresis.get() {
                return;
            }
            state_for_hysteresis
                .handle
                .send(Command::SetHysteresis(row.is_active()));
        });

        Self {
            root,
            mode_row,
            manual_row,
            fan_row,
            rpm_row,
            hysteresis_row,
            temps_group,
            temp_rows: RefCell::new(Vec::new()),
            chart,
            updating,
        }
    }

    fn widget(&self) -> &adw::PreferencesPage {
        &self.root
    }

    fn apply_state(&self, state: &State) {
        self.updating.set(true);
        self.mode_row.set_selected(mode_index(&state.mode));
        self.manual_row.set_value(f64::from(state.manual_level));
        self.hysteresis_row.set_active(state.hysteresis_enabled);
        self.updating.set(false);

        self.fan_row.set_subtitle(&if state.bios_auto {
            "BIOS auto".to_string()
        } else {
            format!("level {}", state.current_level)
        });
        self.rpm_row.set_subtitle(&format!("{} RPM", state.fan_rpm));

        if let Some(max) = state.temperatures.iter().map(|sensor| sensor.celsius).max() {
            self.chart.push(f64::from(max));
        }

        for row in self.temp_rows.borrow_mut().drain(..) {
            self.temps_group.remove(&row);
        }
        if state.temperatures.is_empty() {
            let row = adw::ActionRow::builder()
                .title("No sensors reported")
                .build();
            self.temps_group.add(&row);
            self.temp_rows.borrow_mut().push(row.upcast());
        } else {
            for sensor in &state.temperatures {
                let row = adw::ActionRow::builder()
                    .title(&sensor.name)
                    .subtitle(format!("0x{:02X}", sensor.offset))
                    .build();
                row.add_suffix(&gtk::Label::new(Some(&format!("{} °C", sensor.celsius))));
                self.temps_group.add(&row);
                self.temp_rows.borrow_mut().push(row.upcast());
            }
        }
    }
}

/// The Smart Curve page: a draggable curve plus precise threshold editors.
struct CurvePage {
    root: adw::PreferencesPage,
    editor: CurveEditor,
    levels_group: adw::PreferencesGroup,
    /// Rows currently added to `levels_group`, so they can be removed on rebuild.
    level_rows: RefCell<Vec<gtk::Widget>>,
    state: AppState,
    updating: Rc<Cell<bool>>,
}

impl CurvePage {
    fn new(state: &AppState) -> Self {
        let root = adw::PreferencesPage::new();
        let updating = Rc::new(Cell::new(false));

        let editor_group = adw::PreferencesGroup::new();
        editor_group.set_title("Smart curve");
        editor_group.set_description(Some(
            "Drag a point to change its temperature and fan level.",
        ));
        let editor = CurveEditor::new();
        editor_group.add(editor.widget());

        let levels_group = adw::PreferencesGroup::new();
        levels_group.set_title("Thresholds");

        root.add(&editor_group);
        root.add(&levels_group);

        let state_for_editor = state.clone();
        editor.connect_changed(move |levels| {
            if let Some(mut config) = state_for_editor.config.borrow().clone() {
                match config.smart_modes.first_mut() {
                    Some(mode) => mode.levels = levels,
                    None => config.smart_modes.push(SmartMode {
                        label: None,
                        levels,
                        extra: Default::default(),
                    }),
                }
                state_for_editor.set_config(&config);
            }
        });

        Self {
            root,
            editor,
            levels_group,
            level_rows: RefCell::new(Vec::new()),
            state: state.clone(),
            updating,
        }
    }

    fn widget(&self) -> &adw::PreferencesPage {
        &self.root
    }

    fn apply_config(&self, config: &Config) {
        let levels = config
            .smart_modes
            .first()
            .map(|mode| mode.levels.clone())
            .unwrap_or_default();
        self.editor.set_levels(levels.clone());

        for row in self.level_rows.borrow_mut().drain(..) {
            self.levels_group.remove(&row);
        }
        for (index, level) in levels.iter().enumerate() {
            let adjustment =
                gtk::Adjustment::new(f64::from(level.temperature), 0.0, 127.0, 1.0, 5.0, 0.0);
            let row = adw::SpinRow::new(Some(&adjustment), 1.0, 0);
            row.set_title(&format!("Level {} temperature", index + 1));
            row.set_subtitle(&format!("fan level {}", level.fan_level));

            let state_for_row = self.state.clone();
            let updating = Rc::clone(&self.updating);
            row.connect_value_notify(move |row| {
                if updating.get() {
                    return;
                }
                if let Some(mut config) = state_for_row.config.borrow().clone() {
                    if let Some(mode) = config.smart_modes.first_mut() {
                        if let Some(level) = mode.levels.get_mut(index) {
                            level.temperature = row.value().round().clamp(0.0, 127.0) as u8;
                        }
                    }
                    state_for_row.set_config(&config);
                }
            });
            self.levels_group.add(&row);
            self.level_rows.borrow_mut().push(row.upcast());
        }
    }

    fn apply_state(&self, _state: &State) {}
}

/// The Sensors page: enable/disable, rename, and ignore sensors.
struct SensorsPage {
    root: adw::PreferencesPage,
    group: adw::PreferencesGroup,
    /// Rows currently added to `group`, so they can be removed on rebuild.
    rows: RefCell<Vec<gtk::Widget>>,
    state: AppState,
    known: Rc<RefCell<Vec<String>>>,
}

impl SensorsPage {
    fn new(state: &AppState) -> Self {
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

    fn widget(&self) -> &adw::PreferencesPage {
        &self.root
    }

    fn apply_state(&self, state: &State) {
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
                if let Some(mut config) = state_for_switch.config.borrow().clone() {
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
                if let Some(mut config) = state_for_entry.config.borrow().clone() {
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

    fn apply_config(&self, _config: &Config) {}
}

/// The Settings page: cycle interval, start behaviour, and config path.
struct SettingsPage {
    root: adw::PreferencesPage,
    cycle_row: adw::SpinRow,
    start_minimized_row: adw::SwitchRow,
    config_path_row: adw::ActionRow,
    updating: Rc<Cell<bool>>,
}

impl SettingsPage {
    fn new(state: &AppState) -> Self {
        let root = adw::PreferencesPage::new();
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
        root.add(&group);

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
            if let Some(mut config) = state_for_cycle.config.borrow().clone() {
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
            if let Some(mut config) = state_for_start.config.borrow().clone() {
                config.general.start_minimized = row.is_active();
                state_for_start.set_config(&config);
            }
        });

        Self {
            root,
            cycle_row,
            start_minimized_row,
            config_path_row,
            updating,
        }
    }

    fn widget(&self) -> &adw::PreferencesPage {
        &self.root
    }

    fn apply_config(&self, config: &Config) {
        self.updating.set(true);
        self.cycle_row.set_value(f64::from(config.general.cycle));
        self.start_minimized_row
            .set_active(config.general.start_minimized);
        self.updating.set(false);
    }

    fn apply_state(&self, state: &State) {
        self.config_path_row.set_subtitle(&state.config_path);
    }
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
}
