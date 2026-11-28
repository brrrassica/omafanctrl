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
use omafanctrl_core::engine::highest_smart_level;

/// The lower bound of the temperature axis (°C).
const MIN_TEMPERATURE: f64 = 0.0;

/// The upper bound of the temperature axis (°C), matching the validated
/// threshold range so any accepted threshold stays draggable.
const MAX_TEMPERATURE: f64 = TEMP_MAX_C as f64;

type ChangeCallback = Box<dyn Fn(Vec<SmartLevel>)>;

/// A draggable smart-curve editor.
#[derive(Clone)]
pub struct CurveEditor {
    area: gtk::DrawingArea,
    levels: Rc<RefCell<Vec<SmartLevel>>>,
    hysteresis_enabled: Rc<Cell<bool>>,
    on_change: Rc<RefCell<Option<ChangeCallback>>>,
}

impl CurveEditor {
    /// Create an empty editor.
    pub fn new() -> Self {
        let area = gtk::DrawingArea::new();
        area.set_content_height(200);
        area.set_hexpand(true);
        let levels = Rc::new(RefCell::new(Vec::new()));
        let hysteresis_enabled = Rc::new(Cell::new(true));
        let on_change: Rc<RefCell<Option<ChangeCallback>>> = Rc::new(RefCell::new(None));

        let levels_for_draw = Rc::clone(&levels);
        let hysteresis_for_draw = Rc::clone(&hysteresis_enabled);
        area.set_draw_func(move |_, cr, width, height| {
            draw(
                cr,
                width,
                height,
                &levels_for_draw.borrow(),
                hysteresis_for_draw.get(),
            );
        });

        let drag = gtk::GestureDrag::new();
        let start = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
        let dragged = Rc::new(Cell::new(None));
        let start_begin = Rc::clone(&start);
        let dragged_begin = Rc::clone(&dragged);
        let levels_for_begin = Rc::clone(&levels);
        let area_for_begin = area.clone();
        drag.connect_drag_begin(move |_, x, y| {
            start_begin.set((x, y));
            let width = f64::from(area_for_begin.width());
            let temperature = x_to_temperature(x, width);
            dragged_begin.set(nearest_index(&levels_for_begin.borrow(), temperature));
        });

        let levels_for_drag = Rc::clone(&levels);
        let on_change_for_drag = Rc::clone(&on_change);
        let area_for_drag = area.clone();
        let start_update = Rc::clone(&start);
        let dragged_update = Rc::clone(&dragged);
        drag.connect_drag_update(move |_, offset_x, offset_y| {
            let Some(index) = dragged_update.get() else {
                return;
            };
            let (start_x, start_y) = start_update.get();
            let width = f64::from(area_for_drag.width());
            let height = f64::from(area_for_drag.height());
            let temperature = x_to_temperature(start_x + offset_x, width);
            let fan_level = y_to_level(start_y + offset_y, height);

            let mut levels = levels_for_drag.borrow_mut();
            if move_level_at(&mut levels, index, temperature, fan_level) {
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
            hysteresis_enabled,
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

    /// Set whether the engine's hysteresis extents are shown.
    pub fn set_hysteresis_enabled(&self, enabled: bool) {
        if self.hysteresis_enabled.replace(enabled) != enabled {
            self.area.queue_draw();
        }
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

/// The inclusive range a threshold at `index` may take while staying strictly
/// between its neighbours.
fn temperature_bounds(levels: &[SmartLevel], index: usize) -> (u8, u8) {
    let minimum = index
        .checked_sub(1)
        .map_or(0, |previous| levels[previous].temperature.saturating_add(1));
    let maximum = levels
        .get(index + 1)
        .map_or(TEMP_MAX_C, |next| next.temperature.saturating_sub(1));
    (minimum, maximum.max(minimum))
}

/// Move the threshold at `index` to the dragged position.
///
/// The index is captured when the drag starts, so the grabbed handle keeps
/// moving even when the cursor gets nearer to a different threshold. The
/// temperature is clamped strictly between its neighbours and the fan level
/// between theirs, so a drag can never create a configuration that
/// `Config::validate` rejects.
fn move_level_at(levels: &mut [SmartLevel], index: usize, temperature: f64, fan_level: u8) -> bool {
    if index >= levels.len() {
        return false;
    }
    let (minimum_temperature, maximum_temperature) = temperature_bounds(levels, index);
    let previous_level = index
        .checked_sub(1)
        .map_or(FAN_LEVEL_MIN, |previous| levels[previous].fan_level);
    let next_level = levels
        .get(index + 1)
        .map_or(FAN_LEVEL_MAX, |next| next.fan_level);

    let moved = &mut levels[index];
    moved.temperature = (temperature.round().clamp(0.0, f64::from(TEMP_MAX_C)) as u8)
        .clamp(minimum_temperature, maximum_temperature);
    moved.fan_level = fan_level.clamp(previous_level, next_level);
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HysteresisDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct HysteresisBand {
    start: f64,
    end: f64,
    direction: HysteresisDirection,
}

/// Build the horizontal-then-vertical path used by the engine's step lookup.
fn step_points(levels: &[SmartLevel], width: f64, height: f64) -> Vec<(f64, f64)> {
    let Some(first) = levels.first() else {
        return Vec::new();
    };
    let mut points = vec![
        (0.0, level_to_y(first.fan_level, height)),
        (
            temperature_to_x(f64::from(first.temperature), width),
            level_to_y(first.fan_level, height),
        ),
    ];
    let mut previous_y = points[1].1;
    for level in &levels[1..] {
        let x = temperature_to_x(f64::from(level.temperature), width);
        let y = level_to_y(level.fan_level, height);
        points.push((x, previous_y));
        if y != previous_y {
            points.push((x, y));
        }
        previous_y = y;
    }
    // The final level remains in force all the way to the temperature ceiling.
    points.push((width, previous_y));
    points
}

/// Build the effective hysteresis extents, omitting superseded plateau rows.
fn hysteresis_bands(levels: &[SmartLevel], width: f64, enabled: bool) -> Vec<HysteresisBand> {
    if !enabled {
        return Vec::new();
    }
    let mut bands = Vec::new();
    for level in levels {
        if highest_smart_level(levels, level.fan_level) != Some(*level) {
            continue;
        }
        let x = temperature_to_x(f64::from(level.temperature), width);
        if level.hyst_up > 0 {
            bands.push(HysteresisBand {
                start: x,
                end: temperature_to_x(
                    f64::from(level.temperature.saturating_add(level.hyst_up)),
                    width,
                ),
                direction: HysteresisDirection::Up,
            });
        }
        if level.hyst_down > 0 {
            bands.push(HysteresisBand {
                start: temperature_to_x(
                    f64::from(level.temperature.saturating_sub(level.hyst_down)),
                    width,
                ),
                end: x,
                direction: HysteresisDirection::Down,
            });
        }
    }
    bands
}

/// Draw the threshold step curve and the engine's hysteresis extents.
fn draw(cr: &cairo::Context, width: i32, height: i32, levels: &[SmartLevel], hysteresis: bool) {
    let width = f64::from(width);
    let height = f64::from(height);

    cr.set_source_rgba(0.5, 0.5, 0.5, 0.08);
    cr.rectangle(0.0, 0.0, width, height);
    let _ = cr.fill();

    if levels.is_empty() {
        return;
    }

    for band in hysteresis_bands(levels, width, hysteresis) {
        let color = match band.direction {
            HysteresisDirection::Up => (1.0, 0.65, 0.2, 0.18),
            HysteresisDirection::Down => (0.2, 0.8, 0.9, 0.18),
        };
        cr.set_source_rgba(color.0, color.1, color.2, color.3);
        cr.rectangle(band.start, 0.0, (band.end - band.start).max(0.0), height);
        let _ = cr.fill();
    }

    cr.set_source_rgba(0.2, 0.6, 1.0, 0.9);
    cr.set_line_width(2.0);
    let points = step_points(levels, width, height);
    if let Some(&(x, y)) = points.first() {
        cr.move_to(x, y);
    }
    for &(x, y) in &points[1..] {
        cr.line_to(x, y);
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
        assert_eq!(x_to_temperature(100.0, 100.0), MAX_TEMPERATURE);
        assert_eq!(y_to_level(0.0, 100.0), FAN_LEVEL_MAX);
        assert_eq!(y_to_level(100.0, 100.0), FAN_LEVEL_MIN);
    }

    #[test]
    fn step_curve_uses_horizontal_then_vertical_segments() {
        let levels = vec![level(40, 1), level(60, 3)];

        assert_eq!(
            step_points(&levels, 254.0, 84.0),
            vec![
                (0.0, 84.0),
                (80.0, 84.0),
                (120.0, 84.0),
                (120.0, 56.0),
                (254.0, 56.0),
            ]
        );
    }

    #[test]
    fn step_curve_handles_empty_single_and_duplicate_levels() {
        assert!(step_points(&[], 127.0, 126.0).is_empty());
        assert_eq!(
            step_points(&[level(60, 2)], 127.0, 126.0),
            vec![(0.0, 105.0), (60.0, 105.0), (127.0, 105.0)]
        );

        let levels = vec![level(40, 1), level(50, 2), level(60, 2), level(70, 4)];
        let points = step_points(&levels, 127.0, 126.0);
        assert!(points.contains(&(50.0, 105.0)));
        assert!(points.contains(&(60.0, 105.0)));
        assert!(
            points
                .windows(2)
                .all(|pair| pair[0].0 == pair[1].0 || pair[0].1 == pair[1].1)
        );
    }

    #[test]
    fn hysteresis_bands_show_effective_up_and_down_offsets() {
        let mut threshold = level(60, 2);
        threshold.hyst_up = 5;
        threshold.hyst_down = 4;

        assert_eq!(
            hysteresis_bands(&[threshold], 127.0, true),
            vec![
                HysteresisBand {
                    start: 60.0,
                    end: 65.0,
                    direction: HysteresisDirection::Up,
                },
                HysteresisBand {
                    start: 56.0,
                    end: 60.0,
                    direction: HysteresisDirection::Down,
                },
            ]
        );
        assert!(hysteresis_bands(&[threshold], 127.0, false).is_empty());
    }

    #[test]
    fn hysteresis_bands_omit_zero_and_superseded_plateau_offsets() {
        assert!(hysteresis_bands(&[level(60, 2)], 127.0, true).is_empty());

        let mut lower = level(50, 2);
        lower.hyst_up = 1;
        lower.hyst_down = 1;
        let mut higher = level(60, 2);
        higher.hyst_up = 5;
        higher.hyst_down = 4;
        let bands = hysteresis_bands(&[lower, higher], 127.0, true);

        assert_eq!(bands.len(), 2);
        assert_eq!(bands[0].start, 60.0);
        assert_eq!(bands[0].end, 65.0);
        assert_eq!(bands[1].start, 56.0);
        assert_eq!(bands[1].end, 60.0);
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

    fn level(temperature: u8, fan_level: u8) -> SmartLevel {
        SmartLevel {
            temperature,
            fan_level,
            hyst_up: 0,
            hyst_down: 0,
        }
    }

    #[test]
    fn dragging_cannot_make_fan_levels_decrease() {
        let mut levels = vec![level(40, 1), level(50, 2), level(60, 3)];

        // Drag the first point while asking for the highest level: it must stop
        // at the next point's fan level.
        assert!(move_level_at(&mut levels, 0, 30.0, 3));
        assert_eq!(levels[0], level(30, 2));

        // Drag the last point while asking for the lowest level: it must stay at
        // or above the previous point's fan level.
        assert!(move_level_at(&mut levels, 2, 65.0, 1));
        assert_eq!(levels[2], level(65, 2));
        assert!(
            levels
                .windows(2)
                .all(|pair| pair[0].fan_level <= pair[1].fan_level)
        );
    }

    #[test]
    fn dragging_moves_the_grabbed_point_not_the_nearest_one() {
        let mut levels = vec![level(40, 1), level(70, 3)];

        // The drag started on the 40 point, so passing 64 (nearer the 70 point)
        // must not move the neighbour.
        assert!(move_level_at(&mut levels, 0, 64.0, 1));
        assert_eq!(levels[0], level(64, 1));
        assert_eq!(levels[1], level(70, 3));
    }

    #[test]
    fn dragging_cannot_equal_or_cross_neighbouring_temperatures() {
        let mut levels = vec![level(40, 1), level(50, 2), level(60, 3)];

        // Dragging the first point past its right neighbour stops just below it.
        assert!(move_level_at(&mut levels, 0, 65.0, 1));
        assert_eq!(levels[0], level(49, 1));

        // Dragging the last point past its left neighbour stops just above it.
        assert!(move_level_at(&mut levels, 2, 45.0, 3));
        assert_eq!(levels[2], level(51, 3));

        // A tie requested at the middle point is clamped to its only valid value.
        assert!(move_level_at(&mut levels, 1, 51.0, 2));
        assert_eq!(levels[1], level(50, 2));
        assert!(
            levels
                .windows(2)
                .all(|pair| pair[0].temperature < pair[1].temperature)
        );
    }
}
