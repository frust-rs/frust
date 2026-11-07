//! Memory section: the `frust-device-info` device-gate vehicle. It shows the
//! five figures of [`DeviceMemory::query`] and re-reads them on a **Query**
//! press and on every [`AppLifecycleListener::on_memory_pressure`] event.
//! The gate greps the platform log for `frust-gate device-memory:`.
//!
//! Every query writes exactly one line, `frust-gate device-memory: total=...
//! low_ram=... system_available=... headroom=... system_low=...`, with `none`
//! for a figure the platform does not report. A pressure event first writes
//! `frust-gate device-memory: pressure=<level>` and then the query line.
//!
//! # Threading
//!
//! Memory-pressure hooks run synchronously on the UI thread, the same as the
//! Network page's lifecycle hooks (see
//! [`AppLifecycleListener::on_memory_pressure`]). The listener is an event
//! handler, so it queries and writes the readout and the log signals directly;
//! `build` only reads them.

use std::fmt::Display;
use std::time::Instant;

use frust::{
    AppLifecycleListener, Component, EdgeInsets, Get, MemoryPressure, Padding, RwSignal, Set,
    SizedBox, Theme, Update, View, button, column, component, text, use_context,
};
use frust_device_info::DeviceMemory;

/// Most rows the log keeps; the oldest fall off the end.
const LOG_CAP: usize = 20;

/// The grep prefix every gate line carries in the platform log.
const GATE_PREFIX: &str = "frust-gate device-memory:";

/// The text a figure shows when the platform does not report it.
const NOT_REPORTED: &str = "not reported on this platform";

/// See the page-fn contract in [`crate::pages`].
pub fn page(_state: &crate::PlaygroundState) -> impl View<crate::PlaygroundState> {
    component(DeviceMemoryPage)
}

#[derive(Default)]
struct DeviceMemoryPage;

struct DeviceMemoryPageState {
    reading: RwSignal<Option<DeviceMemory>>,
    log: RwSignal<Vec<String>>,
    start: Instant,
    _pressure: AppLifecycleListener,
}

impl Component for DeviceMemoryPage {
    type State = DeviceMemoryPageState;

    fn init(&self) -> DeviceMemoryPageState {
        let start = Instant::now();
        let first = query_and_log("open", start);
        let reading = RwSignal::new(first.reading);
        let log = RwSignal::new(vec![first.row]);
        let pressure = Self::pressure_listener(reading, log, start);

        DeviceMemoryPageState {
            reading,
            log,
            start,
            _pressure: pressure,
        }
    }

    fn build(&self, state: &mut DeviceMemoryPageState) -> impl View<DeviceMemoryPageState> {
        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let accent = theme.scheme().primary;
        let muted = theme.scheme().on_surface_variant;

        let reading = state.reading.get();
        let mut readout = column();
        for line in readout_lines(reading) {
            readout = readout.child(text(line).size(13.0));
        }
        let mut rows = column();
        for row in state.log.get() {
            rows = rows.child(text(row).size(11.0));
        }

        Padding(
            EdgeInsets::all(16.0),
            column()
                .child(text("Device memory").size(13.0).color(accent))
                .child(
                    text(
                        "Device memory from DeviceMemory::query(). Re-read on Query and on \
                         every memory-pressure event; each read is logged under \
                         \"frust-gate device-memory:\".",
                    )
                    .size(11.0)
                    .color(muted),
                )
                .child(
                    text("Android 14+ delivers no foreground trim level; poll this instead.")
                        .size(11.0)
                        .color(muted),
                )
                .child(SizedBox(None, Some(12.0)))
                .child(readout)
                .child(SizedBox(None, Some(8.0)))
                .child(Self::query_button())
                .child(SizedBox(None, Some(8.0)))
                .child(rows),
        )
    }
}

impl DeviceMemoryPage {
    /// Registers the pressure hook. The hook runs synchronously on the UI
    /// thread, so it is an event handler: it logs the level, queries, and
    /// writes the readout and the log signals directly.
    fn pressure_listener(
        reading: RwSignal<Option<DeviceMemory>>,
        log: RwSignal<Vec<String>>,
        start: Instant,
    ) -> AppLifecycleListener {
        AppLifecycleListener::new().on_memory_pressure(move |level: MemoryPressure| {
            log::info!("{GATE_PREFIX} pressure={level:?}");
            let sample = query_and_log(&format!("pressure={level:?}"), start);
            reading.set(sample.reading);
            log.update(|rows| push_row(rows, sample.row));
        })
    }

    fn query_button() -> impl View<DeviceMemoryPageState> {
        button("Query", |state: &mut DeviceMemoryPageState| {
            let sample = query_and_log("query", state.start);
            state.reading.set(sample.reading);
            let row = sample.row;
            state.log.update(|rows| push_row(rows, row));
        })
    }
}

/// One query's result: the reading and its page row.
struct Sample {
    reading: Option<DeviceMemory>,
    row: String,
}

/// Re-reads the device memory, writes the gate line and returns the sample
/// with its page row. `reason` names what triggered the read.
fn query_and_log(reason: &str, start: Instant) -> Sample {
    let reading = DeviceMemory::query();
    let fields = gate_fields(reading);
    log::info!("{GATE_PREFIX} {fields}");
    let ms = start.elapsed().as_millis();
    Sample {
        reading,
        row: format!("[{ms:>7} ms] {reason}: {fields}"),
    }
}

/// Prepends a row and keeps the log at [`LOG_CAP`].
fn push_row(rows: &mut Vec<String>, row: String) {
    rows.insert(0, row);
    rows.truncate(LOG_CAP);
}

/// The `key=value` fields of one gate line. `none` marks a figure the platform
/// does not report, and every field reads `none` when the query returned `None`.
fn gate_fields(reading: Option<DeviceMemory>) -> String {
    match reading {
        Some(memory) => format!(
            "total={} low_ram={} system_available={} headroom={} system_low={}",
            memory.total_bytes,
            memory.low_ram_device,
            or_none(memory.system_available_bytes),
            or_none(memory.process_headroom_bytes),
            or_none(memory.system_low),
        ),
        None => "total=none low_ram=none system_available=none headroom=none system_low=none"
            .to_string(),
    }
}

/// The five readout lines. A figure the platform does not report reads
/// [`NOT_REPORTED`], and a missing reading reads it for every line.
fn readout_lines(reading: Option<DeviceMemory>) -> Vec<String> {
    let Some(memory) = reading else {
        return [
            "Total",
            "Low-RAM device",
            "System available",
            "Process headroom",
            "System low",
        ]
        .iter()
        .map(|label| format!("{label}: {NOT_REPORTED}"))
        .collect();
    };
    vec![
        format!("Total: {}", bytes_with_gib(Some(memory.total_bytes))),
        format!("Low-RAM device: {}", yes_no(memory.low_ram_device)),
        format!(
            "System available: {}",
            bytes_with_gib(memory.system_available_bytes)
        ),
        format!(
            "Process headroom: {}",
            bytes_with_gib(memory.process_headroom_bytes)
        ),
        format!(
            "System low: {}",
            memory
                .system_low
                .map_or_else(|| NOT_REPORTED.to_string(), yes_no)
        ),
    ]
}

/// `"8.00 GiB (8589934592 bytes)"`, or [`NOT_REPORTED`] for `None`.
fn bytes_with_gib(bytes: Option<u64>) -> String {
    match bytes {
        Some(bytes) => format!("{:.2} GiB ({bytes} bytes)", gib(bytes)),
        None => NOT_REPORTED.to_string(),
    }
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1u64 << 30) as f64
}

fn yes_no(flag: bool) -> String {
    if flag { "yes" } else { "no" }.to_string()
}

fn or_none<T: Display>(value: Option<T>) -> String {
    value.map_or_else(|| "none".to_string(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_reading_is_none_in_every_field() {
        assert_eq!(
            gate_fields(None),
            "total=none low_ram=none system_available=none headroom=none system_low=none"
        );
    }

    #[test]
    fn missing_reading_reads_not_reported_on_every_line() {
        let lines = readout_lines(None);
        assert_eq!(lines.len(), 5);
        assert!(
            lines.iter().all(|line| line.ends_with(NOT_REPORTED)),
            "{lines:?}"
        );
    }

    #[test]
    fn absent_figure_reads_not_reported() {
        assert_eq!(bytes_with_gib(None), NOT_REPORTED);
        assert_eq!(or_none::<u64>(None), "none");
        assert_eq!(or_none(Some(7u64)), "7");
        assert_eq!(or_none(Some(false)), "false");
    }

    #[test]
    fn bytes_show_gib_to_two_decimals() {
        assert_eq!(
            bytes_with_gib(Some(8 * (1u64 << 30))),
            "8.00 GiB (8589934592 bytes)"
        );
    }

    #[test]
    fn push_row_caps_the_log_newest_first() {
        let mut rows = Vec::new();
        for i in 0..(LOG_CAP + 5) {
            push_row(&mut rows, format!("row {i}"));
        }
        assert_eq!(rows.len(), LOG_CAP);
        assert_eq!(rows[0], format!("row {}", LOG_CAP + 4));
    }
}
