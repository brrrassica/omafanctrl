//! The adaptive dashboard.
//!
//! A single [`adw::MultiLayoutView`] holds one instance of every live widget and
//! places them into four [`adw::Layout`]s — `desktop`, `half`, `quarter`, and
//! `eighth` — chosen by [`adw::Breakpoint`] width conditions on an
//! [`adw::BreakpointBin`]. In every layout the graphs sit on top, with status
//! and controls below them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use omafanctrl_core::config::Config;
use omafanctrl_core::dbus::{SensorState, State};

use crate::chart::HistoryChart;
use crate::client::Command;
use crate::state::{AppState, mode_index, mode_name, sensor_is_ignored, temperature};

/// The width (in `sp`) at or above which the `desktop` layout is used.
const DESKTOP_MIN_WIDTH: f64 = 900.0;
/// The width (in `sp`) at or above which the `half` layout is used.
const HALF_MIN_WIDTH: f64 = 600.0;
/// The width (in `sp`) at or above which the `quarter` layout is used.
const QUARTER_MIN_WIDTH: f64 = 400.0;

/// The adaptive dashboard.
pub struct Dashboard {
    bin: adw::BreakpointBin,
    mode_row: adw::ComboRow,
    manual_row: adw::SpinRow,
    fan_row: adw::ActionRow,
    rpm_row: adw::ActionRow,
    hysteresis_row: adw::SwitchRow,
    temps_group: adw::PreferencesGroup,
    /// Rows currently added to `temps_group`, keyed by sensor name, so their
    /// values can be updated in place between rebuilds.
    temp_rows: RefCell<Vec<(String, gtk::Widget, gtk::Label)>>,
    chart: HistoryChart,
    rpm_chart: HistoryChart,
    updating: Rc<Cell<bool>>,
    /// The most recent sensor snapshot, replayed when the ignore list changes.
    last_temps: RefCell<Vec<SensorState>>,
    /// Shared state, used to read the current `IgnoreSensors` list.
    state: AppState,
}

impl Dashboard {
    /// Build the dashboard.
    pub fn new(state: &AppState) -> Self {
        let updating = Rc::new(Cell::new(false));

        // Status group.
        let status_group = adw::PreferencesGroup::new();
        status_group.set_title("Status");
        let fan_row = adw::ActionRow::builder().title("Fan").build();
        let rpm_row = adw::ActionRow::builder().title("Speed").build();
        let hysteresis_row = adw::SwitchRow::builder().title("Hysteresis").build();
        status_group.add(&fan_row);
        status_group.add(&rpm_row);
        status_group.add(&hysteresis_row);

        // Controls group.
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

        // Temperature history chart.
        let chart_group = adw::PreferencesGroup::new();
        chart_group.set_title("Temperature history");
        let chart = HistoryChart::new(
            &[("CPU", (0.20, 0.60, 1.00)), ("GPU", (1.00, 0.50, 0.20))],
            20.0,
            100.0,
        );
        chart_group.add(chart.widget());

        // Fan speed history chart.
        let rpm_group = adw::PreferencesGroup::new();
        rpm_group.set_title("Fan speed history");
        let rpm_chart = HistoryChart::new(&[("Fan", (0.30, 0.80, 0.40))], 0.0, 4500.0);
        rpm_group.add(rpm_chart.widget());

        // Temperatures group.
        let temps_group = adw::PreferencesGroup::new();
        temps_group.set_title("Temperatures");

        // The multi-layout view and its four layouts.
        let view = adw::MultiLayoutView::new();
        view.add_layout(make_layout("desktop", &desktop_layout()));
        view.add_layout(make_layout("half", &half_layout()));
        view.add_layout(make_layout("quarter", &quarter_layout()));
        view.add_layout(make_layout("eighth", &eighth_layout()));
        view.set_layout_name("desktop");

        view.set_child("temp_chart", &chart_group);
        view.set_child("rpm_chart", &rpm_group);
        view.set_child("status", &status_group);
        view.set_child("controls", &mode_group);
        view.set_child("temperatures", &temps_group);

        // The whole dashboard scrolls, so the narrowest layout keeps every
        // widget reachable.
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scrolled.set_hexpand(true);
        scrolled.set_vexpand(true);
        scrolled.set_child(Some(&view));

        // Breakpoints are attached to the bin, which measures the dashboard's
        // own width rather than the window's.
        let bin = adw::BreakpointBin::new();
        bin.set_child(Some(&scrolled));
        add_breakpoint(
            &bin,
            &view,
            adw::BreakpointCondition::new_length(
                adw::BreakpointConditionLengthType::MaxWidth,
                QUARTER_MIN_WIDTH,
                adw::LengthUnit::Sp,
            ),
            "eighth",
        );
        add_breakpoint(
            &bin,
            &view,
            adw::BreakpointCondition::new_length(
                adw::BreakpointConditionLengthType::MinWidth,
                QUARTER_MIN_WIDTH,
                adw::LengthUnit::Sp,
            ),
            "quarter",
        );
        add_breakpoint(
            &bin,
            &view,
            adw::BreakpointCondition::new_length(
                adw::BreakpointConditionLengthType::MinWidth,
                HALF_MIN_WIDTH,
                adw::LengthUnit::Sp,
            ),
            "half",
        );
        add_breakpoint(
            &bin,
            &view,
            adw::BreakpointCondition::new_length(
                adw::BreakpointConditionLengthType::MinWidth,
                DESKTOP_MIN_WIDTH,
                adw::LengthUnit::Sp,
            ),
            "desktop",
        );

        // Mode controls.
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
            bin,
            mode_row,
            manual_row,
            fan_row,
            rpm_row,
            hysteresis_row,
            temps_group,
            temp_rows: RefCell::new(Vec::new()),
            chart,
            rpm_chart,
            updating,
            last_temps: RefCell::new(Vec::new()),
            state: state.clone(),
        }
    }

    /// The underlying widget.
    pub fn widget(&self) -> &adw::BreakpointBin {
        &self.bin
    }

    /// Apply a fresh state snapshot.
    pub fn apply_state(&self, state: &State) {
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

        let cpu = temperature(&state.temperatures, "cpu");
        let gpu = temperature(&state.temperatures, "gpu");
        self.chart.push(&[cpu, gpu]);
        self.rpm_chart.push(&[Some(f64::from(state.fan_rpm))]);

        *self.last_temps.borrow_mut() = state.temperatures.clone();
        self.update_temperatures(&state.temperatures);
    }

    /// Rebuild the temperature rows when the ignore list changes, so sensors
    /// disabled in the configuration disappear from the dashboard immediately.
    pub fn apply_config(&self, _config: &Config) {
        let temps = self.last_temps.borrow().clone();
        self.update_temperatures(&temps);
    }

    /// Rebuild the temperature rows when the sensor set changes, otherwise
    /// update the existing rows in place so the fast refresh does not churn
    /// widgets.
    fn update_temperatures(&self, sensors: &[SensorState]) {
        let mut rows = self.temp_rows.borrow_mut();

        // Sensors listed in `IgnoreSensors` are hidden from the dashboard.
        let config = self.state.config.borrow();
        let sensors: Vec<&SensorState> = sensors
            .iter()
            .filter(|sensor| !sensor_is_ignored(config.as_ref(), sensor))
            .collect();

        if sensors.is_empty() {
            if rows.len() == 1 && rows[0].0.is_empty() {
                return;
            }
            for (_, row, _) in rows.drain(..) {
                self.temps_group.remove(&row);
            }
            let row = adw::ActionRow::builder()
                .title("No sensors reported")
                .build();
            self.temps_group.add(&row);
            rows.push((String::new(), row.upcast(), gtk::Label::new(None)));
            return;
        }

        let same_set = rows.len() == sensors.len()
            && rows
                .iter()
                .zip(&sensors)
                .all(|((name, _, _), sensor)| name == &sensor.name);
        if same_set {
            for ((_, _, label), sensor) in rows.iter().zip(&sensors) {
                label.set_text(&format!("{} °C", sensor.celsius));
            }
            return;
        }

        for (_, row, _) in rows.drain(..) {
            self.temps_group.remove(&row);
        }
        for sensor in &sensors {
            let row = adw::ActionRow::builder()
                .title(&sensor.name)
                .subtitle(format!("0x{:02X}", sensor.offset))
                .build();
            let label = gtk::Label::new(Some(&format!("{} °C", sensor.celsius)));
            row.add_suffix(&label);
            self.temps_group.add(&row);
            rows.push((sensor.name.clone(), row.upcast(), label));
        }
    }
}

/// Wrap a layout's content widget in a named [`adw::Layout`].
fn make_layout(name: &str, content: &impl IsA<gtk::Widget>) -> adw::Layout {
    let layout = adw::Layout::new(content);
    layout.set_name(Some(name));
    layout
}

/// A layout slot with the given id, optionally expanding horizontally.
fn slot(id: &str, hexpand: bool) -> adw::LayoutSlot {
    let slot = adw::LayoutSlot::new(id);
    slot.set_hexpand(hexpand);
    slot
}

/// A vertical container with the dashboard's standard spacing and margins.
fn column() -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.set_margin_top(12);
    column.set_margin_bottom(12);
    column.set_margin_start(12);
    column.set_margin_end(12);
    column
}

/// A horizontal container with the dashboard's standard spacing.
fn row() -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, 12)
}

/// The `desktop` layout: graphs side by side on top, status and controls side
/// by side below, then temperatures.
fn desktop_layout() -> gtk::Box {
    let graphs = row();
    graphs.append(&slot("temp_chart", true));
    graphs.append(&slot("rpm_chart", true));

    let controls = row();
    controls.append(&slot("status", true));
    controls.append(&slot("controls", true));

    let column = column();
    column.append(&graphs);
    column.append(&controls);
    column.append(&slot("temperatures", false));
    column
}

/// The `half` layout: graphs side by side on top, then status, controls, and
/// temperatures stacked.
fn half_layout() -> gtk::Box {
    let graphs = row();
    graphs.append(&slot("temp_chart", true));
    graphs.append(&slot("rpm_chart", true));

    let column = column();
    column.append(&graphs);
    column.append(&slot("status", false));
    column.append(&slot("controls", false));
    column.append(&slot("temperatures", false));
    column
}

/// The `quarter` layout: graphs stacked on top, status and controls side by
/// side, then temperatures.
fn quarter_layout() -> gtk::Box {
    let controls = row();
    controls.append(&slot("status", true));
    controls.append(&slot("controls", true));

    let column = column();
    column.append(&slot("temp_chart", false));
    column.append(&slot("rpm_chart", false));
    column.append(&controls);
    column.append(&slot("temperatures", false));
    column
}

/// The `eighth` layout: every widget in a single column.
fn eighth_layout() -> gtk::Box {
    let column = column();
    column.append(&slot("temp_chart", false));
    column.append(&slot("rpm_chart", false));
    column.append(&slot("status", false));
    column.append(&slot("controls", false));
    column.append(&slot("temperatures", false));
    column
}

/// Attach a breakpoint that switches the view to the named layout.
fn add_breakpoint(
    bin: &adw::BreakpointBin,
    view: &adw::MultiLayoutView,
    condition: adw::BreakpointCondition,
    name: &str,
) {
    let breakpoint = adw::Breakpoint::new(condition);
    breakpoint.add_setter(view, "layout-name", Some(&glib::Value::from(name)));
    bin.add_breakpoint(breakpoint);
}

/// The layout name for a given dashboard width.
///
/// This mirrors the breakpoint conditions above and documents the intended
/// mapping; the breakpoints themselves are declarative.
#[cfg(test)]
fn layout_for_width(width: f64) -> &'static str {
    if width >= DESKTOP_MIN_WIDTH {
        "desktop"
    } else if width >= HALF_MIN_WIDTH {
        "half"
    } else if width >= QUARTER_MIN_WIDTH {
        "quarter"
    } else {
        "eighth"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_thresholds_are_ordered() {
        const { assert!(QUARTER_MIN_WIDTH < HALF_MIN_WIDTH) };
        const { assert!(HALF_MIN_WIDTH < DESKTOP_MIN_WIDTH) };
    }

    #[test]
    fn width_maps_to_the_expected_layout() {
        assert_eq!(layout_for_width(1200.0), "desktop");
        assert_eq!(layout_for_width(900.0), "desktop");
        assert_eq!(layout_for_width(700.0), "half");
        assert_eq!(layout_for_width(600.0), "half");
        assert_eq!(layout_for_width(500.0), "quarter");
        assert_eq!(layout_for_width(400.0), "quarter");
        assert_eq!(layout_for_width(300.0), "eighth");
    }
}
