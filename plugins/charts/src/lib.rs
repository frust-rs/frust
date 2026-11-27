//! `frust-charts`: live charts for Frust apps, platform-free and built on the facade alone.
//!
//! Data lives in shared handles, not in view state: a [`Series`] is a fixed-capacity ring of
//! evenly spaced samples (newest last, `None` a gap), a [`TimedSeries`] keeps timestamped
//! points bounded by a count and/or a time window, and [`Samples`] is a fixed set of current
//! values. [`Categories`] (labelled rows of values), [`Points`] (scatter points) and
//! [`Candles`] (a ring of open-high-low-close candles) feed the chart families. All are cheap
//! to clone and `Send + Sync`, so a background thread writes while a chart paints. Charts
//! repaint when a handle is written, with no rebuild and no frame loop: each write bumps the
//! handle's revision and calls [`frust::request_repaint`] once.
//!
//! Charts are display-only and tap-transparent by default, so a tap reaches whatever lies
//! beneath. Setting `on_scrub` on a `time_series` opts in to scrubbing: its plot then handles
//! presses. A [`YAxis`] adds tick rules and labels, also opt-in; on an automatic domain it asks
//! for one layout pass when new data changes its ticks, the only time a write leads to more
//! than a repaint. A chart's colours come from the caller first, then the installed theme,
//! then the fallback constants on [`Tone`]. [`YDomain`] and [`nice_max`] pick the y range.
//!
//! Beside the monitoring views (`time_series`, `sparkline`, `meter`, `heat_cells`) sit six
//! chart families: `bar_chart`, `pie_chart`, `gauge`, `scatter_chart`, `radar_chart` and
//! `candlestick_chart`. Each repaints on a write and is display-only, except the centre child
//! of `pie_chart` and `gauge`; `bar_chart` and `radar_chart` ask for one layout pass when a
//! label changes. [`YDomain::auto`] spans the data's minimum to maximum, and the polar helpers
//! ([`arc_path`], [`sector_path`] and others) work in radians, 0 at 3 o'clock, clockwise.
//!
//! The geometry helpers the charts are built from (value-to-pixel mapping, line and area paths,
//! snapping, grid rules) are public under [`geometry`], not at the crate root.
//!
//! ```
//! use frust_charts::{Samples, Series, YDomain};
//!
//! let load = Series::new(120); // two minutes at one sample a second
//! let worker = load.clone();
//! std::thread::spawn(move || worker.push(42.0)).join().unwrap();
//! assert_eq!(load.latest(), Some(42.0));
//! assert_eq!(YDomain::auto_from_zero().resolve(load.max()), (0.0, 50.0));
//!
//! let cores = Samples::new(8);
//! cores.slot(3).set(97.5);
//! ```

mod axis;
mod bar;
mod candlestick;
mod categories;
mod gauge;
pub mod geometry;
mod heat;
mod label;
mod meter;
mod pie;
mod polar;
mod radar;
mod samples;
mod scale;
mod scatter;
mod scrub;
mod series;
mod sparkline;
mod time_series;
mod timed;
mod tone;

pub use axis::{AxisSide, TickFormat, YAxis, ticks};
pub use categories::{Categories, CategoryRow};
pub use polar::{
    angle_of, arc_path, normalize_angle, point_on_circle, sector_path, sweep_contains,
};
pub use samples::{SampleSlot, Samples};
pub use scale::{YDomain, nice_max};
pub use scrub::{ScrubEvent, ScrubSample};
pub use series::{Series, SlotsView};
pub use timed::{SeriesSource, TimedSeries};
pub use tone::Tone;

pub use bar::{BarChartView, BarChartWidget, BarMode, bar_chart};
pub use candlestick::{
    Candle, CandleSlots, Candles, CandlestickView, CandlestickWidget, candlestick_chart,
};
pub use gauge::{GaugeCap, GaugeView, GaugeWidget, NeedleStyle, gauge};
pub use heat::{HeatCellsView, HeatCellsWidget, heat_cells};
pub use meter::{
    MeterStripView, MeterStripWidget, MeterValue, MeterView, MeterWidget, Segment, meter,
    meter_segments, meter_strip,
};
pub use pie::{PieChartView, PieChartWidget, pie_chart};
pub use radar::{RadarChartView, RadarChartWidget, RadarShape, RadarStyle, radar_chart};
pub use scatter::{
    Points, PointsView, ScatterChartView, ScatterChartWidget, ScatterPoint, ScatterShape,
    scatter_chart,
};
pub use sparkline::{SparkStyle, SparklineView, SparklineWidget, sparkline};
pub use time_series::{
    Corner, PlotStyle, SeriesFill, SeriesStyle, TimeSeriesView, TimeSeriesWidget, auto_domain,
    time_series,
};
