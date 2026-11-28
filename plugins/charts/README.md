# frust-charts

Live charts for Frust apps, fed from shared data handles (platform-free).

<p align="center">
  <img src="https://raw.githubusercontent.com/frust-rs/frust/main/docs/assets/readme/charts-time-series.png" alt="time_series as an area, block columns and a mirrored pair, above sparkline rows" width="24%">
  <img src="https://raw.githubusercontent.com/frust-rs/frust/main/docs/assets/readme/charts-meters-heat-bars.png" alt="sparklines, meters, a per-core meter strip, heat cells and a grouped bar chart" width="24%">
  <img src="https://raw.githubusercontent.com/frust-rs/frust/main/docs/assets/readme/charts-donut-gauge-scatter.png" alt="a grouped bar chart, a donut, a gauge and a scatter chart" width="24%">
  <img src="https://raw.githubusercontent.com/frust-rs/frust/main/docs/assets/readme/charts-radar-candlesticks.png" alt="a donut, a gauge, a scatter chart, a radar chart and a candlestick chart" width="24%">
</p>

<p align="center"><em>The playground's Charts page on a Pixel 5, every chart fed live from one background thread.</em></p>

## Platform support

`frust-charts` is platform-free: every chart is painted on the facade's canvas, with no native
code, no manifest permission, no plist key and no setup call, so it sets no platform minimum. It
has been run on a Pixel 5 (the screenshots above).

## Add the dependency

```toml
[dependencies]
frust-charts = "0.6"
```

## Data handles

```rust
use frust_charts::{Samples, Series, Tone, YDomain};

// A ring of 120 evenly spaced slots, newest last; `None` is a gap.
let load = Series::new(120);
let writer = load.clone();          // cheap, Send + Sync
std::thread::spawn(move || {
    writer.push(42.0);              // non-finite values become gaps
    writer.push_gap();
});

// A fixed set of current values, e.g. per-core load.
let cores = Samples::new(8);
cores.set(0, 12.5);
let meter_value = cores.slot(3);    // one index, for a single meter

let (min, max) = YDomain::auto_from_zero().resolve(load.max());
let tone = Tone::load(calm, busy, critical); // busy from 60, critical from 85
```

A write never rebuilds or lays out: it bumps the handle's revision and calls
`frust::request_repaint()` once per call, so a chart painting from the handle
updates on the next frame with no frame loop. The one exception is a
`time_series` with a y axis on an automatic domain: when new data changes its
ticks, the chart asks for one layout pass to shape the new labels. Charts are
display-only and tap-transparent unless a `time_series` sets `on_scrub` (see
Scrub). Colours come from the caller, then the installed theme, then the
fallback constants on `Tone`.

`YDomain` picks the y range: `fixed(min, max)`, `percent()` (0..100) or
`auto_from_zero()`, which rounds the data's top up with `nice_max` (1, 2, 2.5,
5 or 10 times a power of ten). `resolve` always returns `max > min`.

## Y axis

```rust
use frust_charts::{AxisSide, SeriesStyle, TickFormat, YAxis, time_series};

time_series::<App>()
    .series(load.clone(), SeriesStyle::color(blue))
    .y_axis(YAxis::new().side(AxisSide::Right).gutter(true).format(TickFormat::percent()));
```

Off by default. `ticks(min, max, target_count)` picks round tick values (`tick_count`
defaults to 3) and they replace the `.grid` fractions. `TickFormat` also has `plain`, `metric`,
`bytes`, `bytes_per_second` and `custom`; a custom closure is compared by identity, so build it
once rather than on every rebuild.

## Timestamped data

```rust
use frust_charts::TimedSeries;

let rtt = TimedSeries::windowed(60.0).gap_after(5.0); // last 60 s; a break over 5 s is a gap
rtt.push(now_secs, 31.5);                              // (seconds, value), kept in time order
time_series::<App>().series(rtt.clone(), SeriesStyle::color(blue));
```

`TimedSeries::new(capacity)` bounds by count instead; `window` adds a time bound to either.
`series`, `mirrored` and `sparkline` take a `Series` or a `TimedSeries`. Block styles draw a
line for timed data.

## Scrub

```rust
use frust_charts::{ScrubEvent, time_series};

time_series::<App>()
    .series(load.clone(), SeriesStyle::color(blue))
    .crosshair(white, 1.0)
    .on_scrub(|app: &mut App, e: Option<ScrubEvent>| app.hover = e.map(|e| e.values));
```

Off by default. With `on_scrub` set the plot handles presses, so it is no longer tap-transparent.
The callback gets a `ScrubEvent` (`sample`, one value per series, `x`, `position`) while the
pointer is down, and `None` on release. Inside a vertical scroll view a sideways drag stays with
the chart, while a horizontal scroll view takes the drag after the touch slop. Whenever a scroll
view takes the drag, the `None` arrives on the rebuild that follows.

## Line refinements

`TimeSeriesView::smooth(true)` draws a monotone curve through the samples that never overshoots
them. `SeriesStyle::tone_along_value(tone)` changes the line (and its area fill) colour where it
crosses a `Tone` threshold; `SeriesStyle::tone` still colours block columns only.

## Chart families

Six more families draw from handles the same way: a write repaints with no rebuild, and every
family is display-only and tap-transparent, except the centre child of `pie_chart` and `gauge`,
which receives input normally.

```rust
use frust_charts::{BarMode, Categories, Tone, YAxis, YDomain, bar_chart};

let sales = Categories::from_rows([("Q1", [12.0, 8.0]), ("Q2", [15.0, 9.5])]);
bar_chart(sales.clone())                  // grouped by default; one colour per value column
    .mode(BarMode::Stacked)
    .colors([blue, green])
    .y_axis(YAxis::new());                // default domain: automatic, always contains 0
```

`bar_chart` can also run `.horizontal()` (the axis then runs along the bottom), colour by value
with `.tone(Tone)`, and round the outer ends with `.radius`. Category labels are thinned to every
k-th when crowded, never shortened. `YAxis::gutter` has no effect here (a measured gutter is
always reserved), and neither does `YAxis::side` in horizontal mode.

```rust
use frust_charts::pie_chart;

pie_chart::<App>(sales.clone())           // one section per row, from its first value
    .donut(0.55)                          // inner radius as a fraction; 0 is a full pie
    .gap(2.0)                             // constant-width gaps
    .corner_radius(3.0)
    .start_angle(0.0)                     // radians
    .center(text("Sales"));               // in the donut hole; receives input
```

```rust
use frust_charts::{GaugeCap, NeedleStyle, Samples, gauge};

let cores = Samples::new(8);
gauge::<App>(cores.slot(0))               // a fixed f32 or a live SampleSlot
    .zones([(0.0, 60.0, calm), (60.0, 100.0, critical)])
    .needle(NeedleStyle::new())
    .ticks(11, 4.0, muted)                // count, length, colour
    .cap(GaugeCap::Round)
    .angles(std::f32::consts::PI, std::f32::consts::PI); // a half dial over the top
```

`gauge` is the radial sibling of `meter`; `.center(view)` puts a child in the middle.

```rust
use frust_charts::{Points, ScatterPoint, ScatterShape, scatter_chart};

let points = Points::new(1_000);          // a bounded handle of ScatterPoints
points.push(1.0, 20.0);
points.push_point(ScatterPoint::new(2.0, 35.0).with_radius(6.0));
scatter_chart::<App>(points.clone()).shape(ScatterShape::Cross).colors([blue, green]);
```

Both axes default to an automatic domain, and a point outside a domain is skipped. Points and
grid rules share one data area, inset by the largest radius in the series, so a point on the
range edge is drawn whole and a point on a grid fraction sits on its rule.

```rust
use frust_charts::{RadarShape, RadarStyle, radar_chart};

let skills = Categories::from_rows([
    ("Speed", [70.0, 50.0]),
    ("Power", [40.0, 80.0]),
    ("Range", [60.0, 65.0]),              // three axes or more; fewer draws the web only
]);
radar_chart(skills.clone())               // each row is an axis, each value column a dataset
    .shape(RadarShape::Circle)            // the web: Polygon (default) or Circle
    .dataset_styles([RadarStyle::color(blue), RadarStyle::color(green)]);
```

Axis titles are kept inside the widget.

```rust
use frust_charts::{Candle, Candles, candlestick_chart};

let candles = Candles::new(60);           // a fixed-capacity ring, newest right-aligned
candles.push(Candle::new(10.0, 12.0, 9.0, 11.0));  // open, high, low, close
candles.update_last(Candle::new(10.0, 13.0, 9.0, 12.5)); // the forming candle
candlestick_chart(candles.clone()).up(green).down(red);
```

The automatic domain is not anchored at zero.

`Categories` holds labelled rows of values (`set_value`, `set_label`, `set_rows`). A value write
repaints; a label change (`bar_chart` and `radar_chart` draw the labels) asks for one layout pass
to shape the new text. `YDomain::auto()` is an automatic range from the data's minimum to its
maximum with both ends rounded to round numbers; `resolve_range(Some((min, max)))` resolves it.
The polar helpers `arc_path`, `sector_path`, `point_on_circle`, `angle_of`, `normalize_angle` and
`sweep_contains` work in radians with 0 at 3 o'clock, positive clockwise.
