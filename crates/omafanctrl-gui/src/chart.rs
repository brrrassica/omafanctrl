//! Cairo history charts.
//!
//! A small [`gtk::DrawingArea`] that keeps a rolling window of one or more
//! named series and draws them as a composite line chart. It is used for the
//! CPU + GPU temperature history and for the fan-speed (RPM) history.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use gtk::cairo;
use gtk::prelude::*;

/// The number of samples kept in the history.
///
/// At the daemon's 4 Hz telemetry cadence this is 150 seconds of history.
const HISTORY: usize = 600;

/// A single named series drawn on a chart.
struct Series {
    /// The legend label.
    label: &'static str,
    /// The line colour as `(red, green, blue)` in `0.0..=1.0`.
    color: (f64, f64, f64),
    /// The rolling history; `None` marks a gap where the value was absent.
    history: VecDeque<Option<f64>>,
}

impl Series {
    /// Create an empty series.
    fn new(label: &'static str, color: (f64, f64, f64)) -> Self {
        Self {
            label,
            color,
            history: VecDeque::with_capacity(HISTORY),
        }
    }

    /// Append a sample, evicting the oldest when the window is full.
    fn push(&mut self, value: Option<f64>) {
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(value);
    }
}

/// A rolling, multi-series line chart.
#[derive(Clone)]
pub struct HistoryChart {
    area: gtk::DrawingArea,
    series: Rc<RefCell<Vec<Series>>>,
}

impl HistoryChart {
    /// Create an empty chart.
    ///
    /// `series` lists the `(label, colour)` of each line, in the order samples
    /// are passed to [`HistoryChart::push`]. `min` and `max` bound the value
    /// axis.
    pub fn new(series: &[(&'static str, (f64, f64, f64))], min: f64, max: f64) -> Self {
        let area = gtk::DrawingArea::new();
        area.set_content_height(160);
        area.set_hexpand(true);
        let series = Rc::new(RefCell::new(
            series
                .iter()
                .map(|(label, color)| Series::new(label, *color))
                .collect::<Vec<_>>(),
        ));
        let series_for_draw = Rc::clone(&series);
        area.set_draw_func(move |_, cr, width, height| {
            draw(cr, width, height, &series_for_draw.borrow(), min, max);
        });
        Self { area, series }
    }

    /// The underlying widget.
    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    /// Append one sample per series (in declaration order) and redraw.
    ///
    /// A `None` sample records a gap, so a missing value does not shift the
    /// remaining series out of alignment.
    pub fn push(&self, samples: &[Option<f64>]) {
        {
            let mut series = self.series.borrow_mut();
            for (series, sample) in series.iter_mut().zip(samples) {
                series.push(*sample);
            }
        }
        self.area.queue_draw();
    }
}

/// Draw the chart.
fn draw(cr: &cairo::Context, width: i32, height: i32, series: &[Series], min: f64, max: f64) {
    let width = f64::from(width);
    let height = f64::from(height);

    // Background.
    cr.set_source_rgba(0.5, 0.5, 0.5, 0.08);
    cr.rectangle(0.0, 0.0, width, height);
    let _ = cr.fill();

    let span = (max - min).max(f64::EPSILON);
    let step = width / (HISTORY as f64 - 1.0);

    for series in series {
        if series.history.len() < 2 {
            continue;
        }
        let offset = width - step * (series.history.len() as f64 - 1.0);
        cr.set_source_rgba(series.color.0, series.color.1, series.color.2, 0.9);
        cr.set_line_width(2.0);
        let mut started = false;
        for (index, value) in series.history.iter().enumerate() {
            let Some(value) = value else {
                started = false;
                continue;
            };
            let x = offset + step * index as f64;
            let normalized = ((value - min) / span).clamp(0.0, 1.0);
            let y = height - normalized * height;
            if started {
                cr.line_to(x, y);
            } else {
                cr.move_to(x, y);
                started = true;
            }
        }
        let _ = cr.stroke();
    }

    draw_legend(cr, series);
}

/// Draw the series legend in the top-left corner.
fn draw_legend(cr: &cairo::Context, series: &[Series]) {
    cr.set_font_size(11.0);
    let mut x = 8.0;
    let y = 14.0;
    for series in series {
        cr.set_source_rgba(series.color.0, series.color.1, series.color.2, 0.9);
        cr.rectangle(x, y - 8.0, 8.0, 8.0);
        let _ = cr.fill();
        x += 12.0;

        cr.move_to(x, y);
        let _ = cr.show_text(series.label);
        if let Ok(extents) = cr.text_extents(series.label) {
            x += extents.width() + 16.0;
        } else {
            x += 40.0;
        }
    }
}
