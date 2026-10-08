//! Smart-curve editor widget.
//!
//! A [`gtk::DrawingArea`] that draws the smart-mode thresholds as a step curve
//! and lets the user drag a point to change its temperature (horizontally) and
//! fan level (vertically).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::cairo;
use gtk::prelude::*;
use omafanctrl_core::config::SmartLevel;
use omafanctrl_core::ec::{FAN_LEVEL_MAX, FAN_LEVEL_MIN, TEMP_MAX_C};

/// The lower bound of the temperature axis (°C).
const MIN_TEMPERATURE: f64 = 0.0;

/// The upper bound of the temperature axis (°C).
const MAX_TEMPERATURE: f64 = 100.0;

type ChangeCallback = Box<dyn Fn(Vec<SmartLevel>)>;

/// A draggable smart-curve editor.
#[derive(Clone)]
pub struct CurveEditor {
    area: gtk::DrawingArea,
    levels: Rc<RefCell<Vec<SmartLevel>>>,
    on_change: Rc<RefCell<Option<ChangeCallback>>>,
}

impl CurveEditor {
    /// Create an empty editor.
    pub fn new() -> Self {
        let area = gtk::DrawingArea::new();
        area.set_content_height(200);
        area.set_hexpand(true);
        let levels = Rc::new(RefCell::new(Vec::new()));
        let on_change: Rc<RefCell<Option<ChangeCallback>>> = Rc::new(RefCell::new(None));

        let levels_for_draw = Rc::clone(&levels);
        area.set_draw_func(move |_, cr, width, height| {
            draw(cr, width, height, &levels_for_draw.borrow());
        });

        let drag = gtk::GestureDrag::new();
        let start = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
        let start_begin = Rc::clone(&start);
        drag.connect_drag_begin(move |_, x, y| {
            start_begin.set((x, y));
        });

        let levels_for_drag = Rc::clone(&levels);
        let on_change_for_drag = Rc::clone(&on_change);
        let area_for_drag = area.clone();
        let start_update = Rc::clone(&start);
        drag.connect_drag_update(move |_, offset_x, offset_y| {
            let (start_x, start_y) = start_update.get();
            let width = f64::from(area_for_drag.width());
            let height = f64::from(area_for_drag.height());
            let temperature = x_to_temperature(start_x + offset_x, width);
            let fan_level = y_to_level(start_y + offset_y, height);

            let mut levels = levels_for_drag.borrow_mut();
            if let Some(index) = nearest_index(&levels, temperature) {
                levels[index].temperature =
                    temperature.round().clamp(0.0, f64::from(TEMP_MAX_C)) as u8;
                levels[index].fan_level = fan_level;
                levels.sort_by_key(|level| level.temperature);
                let updated = levels.clone();
                drop(levels);
                area_for_drag.queue_draw();
                if let Some(callback) = on_change_for_drag.borrow().as_ref() {
                    callback(updated);
                }
            }
        });
        area.add_controller(drag);

        Self {
            area,
            levels,
            on_change,
        }
    }

    /// The underlying widget.
    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    /// Replace the displayed thresholds.
    pub fn set_levels(&self, levels: Vec<SmartLevel>) {
        *self.levels.borrow_mut() = levels;
        self.area.queue_draw();
    }

    /// Register a callback invoked whenever a threshold is dragged.
    pub fn connect_changed<F: Fn(Vec<SmartLevel>) + 'static>(&self, callback: F) {
        *self.on_change.borrow_mut() = Some(Box::new(callback));
    }
}

impl Default for CurveEditor {
    fn default() -> Self {
        Self::new()
    }
}

/// Map an x coordinate to a temperature.
fn x_to_temperature(x: f64, width: f64) -> f64 {
    if width <= 0.0 {
        return MIN_TEMPERATURE;
    }
    MIN_TEMPERATURE + (x / width).clamp(0.0, 1.0) * (MAX_TEMPERATURE - MIN_TEMPERATURE)
}

/// Map a y coordinate to a fan level.
fn y_to_level(y: f64, height: f64) -> u8 {
    if height <= 0.0 {
        return FAN_LEVEL_MIN;
    }
    let normalized = 1.0 - (y / height).clamp(0.0, 1.0);
    let level = f64::from(FAN_LEVEL_MIN) + normalized * f64::from(FAN_LEVEL_MAX - FAN_LEVEL_MIN);
    (level.round() as u8).clamp(FAN_LEVEL_MIN, FAN_LEVEL_MAX)
}

/// Find the index of the threshold nearest to `temperature`.
fn nearest_index(levels: &[SmartLevel], temperature: f64) -> Option<usize> {
    levels
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            let da = (f64::from(a.temperature) - temperature).abs();
            let db = (f64::from(b.temperature) - temperature).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(index, _)| index)
}

/// Draw the curve.
fn draw(cr: &cairo::Context, width: i32, height: i32, levels: &[SmartLevel]) {
    let width = f64::from(width);
    let height = f64::from(height);

    cr.set_source_rgba(0.5, 0.5, 0.5, 0.08);
    cr.rectangle(0.0, 0.0, width, height);
    let _ = cr.fill();

    if levels.is_empty() {
        return;
    }

    cr.set_source_rgba(0.2, 0.6, 1.0, 0.9);
    cr.set_line_width(2.0);
    let mut first = true;
    for level in levels {
        let x = temperature_to_x(f64::from(level.temperature), width);
        let y = level_to_y(level.fan_level, height);
        if first {
            cr.move_to(0.0, y);
            cr.line_to(x, y);
            first = false;
        } else {
            cr.line_to(x, y);
        }
    }
    let _ = cr.stroke();

    for level in levels {
        let x = temperature_to_x(f64::from(level.temperature), width);
        let y = level_to_y(level.fan_level, height);
        cr.set_source_rgba(0.2, 0.6, 1.0, 1.0);
        cr.arc(x, y, 5.0, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    }
}

/// Map a temperature to an x coordinate.
fn temperature_to_x(temperature: f64, width: f64) -> f64 {
    ((temperature - MIN_TEMPERATURE) / (MAX_TEMPERATURE - MIN_TEMPERATURE)).clamp(0.0, 1.0) * width
}

/// Map a fan level to a y coordinate.
fn level_to_y(level: u8, height: f64) -> f64 {
    let normalized = (f64::from(level) - f64::from(FAN_LEVEL_MIN))
        / (f64::from(FAN_LEVEL_MAX) - f64::from(FAN_LEVEL_MIN));
    height - normalized.clamp(0.0, 1.0) * height
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_coordinates_to_values() {
        assert_eq!(x_to_temperature(0.0, 100.0), 0.0);
        assert_eq!(x_to_temperature(100.0, 100.0), 100.0);
        assert_eq!(y_to_level(0.0, 100.0), FAN_LEVEL_MAX);
        assert_eq!(y_to_level(100.0, 100.0), FAN_LEVEL_MIN);
    }

    #[test]
    fn finds_the_nearest_threshold() {
        let levels = vec![
            SmartLevel {
                temperature: 40,
                fan_level: 1,
                hyst_up: 0,
                hyst_down: 0,
            },
            SmartLevel {
                temperature: 70,
                fan_level: 3,
                hyst_up: 0,
                hyst_down: 0,
            },
        ];
        assert_eq!(nearest_index(&levels, 42.0), Some(0));
        assert_eq!(nearest_index(&levels, 68.0), Some(1));
    }
}
