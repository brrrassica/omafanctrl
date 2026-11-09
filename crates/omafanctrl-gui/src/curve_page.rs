//! The Smart Curve page: a draggable curve plus precise threshold editors.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use omafanctrl_core::config::{Config, SmartMode};
use omafanctrl_core::dbus::State;
use omafanctrl_core::ec::TEMP_MAX_C;

use crate::curve::CurveEditor;
use crate::state::AppState;

/// The Smart Curve page.
pub struct CurvePage {
    root: adw::PreferencesPage,
    editor: CurveEditor,
    levels_group: adw::PreferencesGroup,
    /// Rows currently added to `levels_group`, so they can be removed on rebuild.
    level_rows: RefCell<Vec<gtk::Widget>>,
    state: AppState,
    updating: Rc<Cell<bool>>,
}

impl CurvePage {
    /// Build the page.
    pub fn new(state: &AppState) -> Self {
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
            let config = state_for_editor.config.borrow().clone();
            if let Some(mut config) = config {
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

    /// The underlying widget.
    pub fn widget(&self) -> &adw::PreferencesPage {
        &self.root
    }

    /// Apply a configuration.
    pub fn apply_config(&self, config: &Config) {
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
                let config = state_for_row.config.borrow().clone();
                if let Some(mut config) = config {
                    if let Some(mode) = config.smart_modes.first_mut() {
                        if index < mode.levels.len() {
                            // Keep the threshold strictly between its neighbours;
                            // otherwise `Config::validate` rejects the edit and the
                            // next refresh silently restores the old value.
                            let minimum = index.checked_sub(1).map_or(0, |previous| {
                                mode.levels[previous].temperature.saturating_add(1)
                            });
                            let maximum = mode
                                .levels
                                .get(index + 1)
                                .map_or(TEMP_MAX_C, |next| next.temperature.saturating_sub(1));
                            let maximum = maximum.max(minimum);
                            let temperature =
                                (row.value().round().clamp(0.0, f64::from(TEMP_MAX_C)) as u8)
                                    .clamp(minimum, maximum);
                            mode.levels[index].temperature = temperature;
                            if (row.value() - f64::from(temperature)).abs() > f64::EPSILON {
                                updating.set(true);
                                row.set_value(f64::from(temperature));
                                updating.set(false);
                            }
                        }
                    }
                    state_for_row.set_config(&config);
                }
            });
            self.levels_group.add(&row);
            self.level_rows.borrow_mut().push(row.upcast());
        }
    }

    /// Apply a state snapshot (unused).
    pub fn apply_state(&self, _state: &State) {}
}
