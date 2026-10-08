//! Cairo temperature-history chart.
//!
//! A small [`gtk::DrawingArea`] that keeps a rolling window of the maximum
//! sensor temperature and draws it as a line chart.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use gtk::cairo;
use gtk::prelude::*;

/// The number of samples kept in the history.
const HISTORY: usize = 120;

/// The lower bound of the chart's temperature axis (°C).
const MIN_TEMPERATURE: f64 = 20.0;

/// The upper bound of the chart's temperature axis (°C).
const MAX_TEMPERATURE: f64 = 100.0;

/// A rolling temperature-history chart.
#[derive(Clone)]
pub struct TemperatureChart {
    area: gtk::DrawingArea,
    history: Rc<RefCell<VecDeque<f64>>>,
}

impl TemperatureChart {
    /// Create an empty chart.
    pub fn new() -> Self {
        let area = gtk::DrawingArea::new();
        area.set_content_height(160);
        area.set_hexpand(true);
        let history = Rc::new(RefCell::new(VecDeque::with_capacity(HISTORY)));
        let history_for_draw = Rc::clone(&history);
        area.set_draw_func(move |_, cr, width, height| {
            draw(cr, width, height, &history_for_draw.borrow());
        });
        Self { area, history }
    }

    /// The underlying widget.
    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    /// Append a sample and redraw.
    pub fn push(&self, value: f64) {
        {
            let mut history = self.history.borrow_mut();
            if history.len() == HISTORY {
                history.pop_front();
            }
            history.push_back(value);
        }
        self.area.queue_draw();
    }
}

impl Default for TemperatureChart {
    fn default() -> Self {
        Self::new()
    }
}

/// Draw the chart.
fn draw(cr: &cairo::Context, width: i32, height: i32, history: &VecDeque<f64>) {
    let width = f64::from(width);
    let height = f64::from(height);

    // Background.
    cr.set_source_rgba(0.5, 0.5, 0.5, 0.08);
    cr.rectangle(0.0, 0.0, width, height);
    let _ = cr.fill();

    if history.len() < 2 {
        return;
    }

    let step = width / (HISTORY as f64 - 1.0);
    let offset = width - step * (history.len() as f64 - 1.0);

    cr.set_source_rgba(0.2, 0.6, 1.0, 0.9);
    cr.set_line_width(2.0);
    for (index, value) in history.iter().enumerate() {
        let x = offset + step * index as f64;
        let normalized =
            ((value - MIN_TEMPERATURE) / (MAX_TEMPERATURE - MIN_TEMPERATURE)).clamp(0.0, 1.0);
        let y = height - normalized * height;
        if index == 0 {
            cr.move_to(x, y);
        } else {
            cr.line_to(x, y);
        }
    }
    let _ = cr.stroke();
}
