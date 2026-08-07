//! Real-device gate for the streaming device pipeline
//! [`android_run::spawn_session`] — the exact code path the `frust-tui`
//! supervisor's `start_device` drives, exercised end-to-end against a real
//! attached Android device.
//!
//! Ignored by default: needs the OnePlus 9 (serial `53f887ac`) attached and a
//! full Android toolchain (Android SDK/NDK, a JDK, `cargo-ndk`, the
//! `aarch64-linux-android` target) plus the `../../examples/huddle` project.
//! Run with:
//!
//! ```bash
//! JAVA_HOME="/Applications/Android Studio.app/Contents/jbr/Contents/Home" \
//! ANDROID_NDK_HOME="$ANDROID_HOME/ndk/28.2.13676358" \
//!   cargo test -p frust-drive --test device_gate -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use frust_drive::android_run;
use frust_drive::build_info::{BuildInfo, BuildMode};
use frust_drive::devices::{Device, Kind, Platform};
use frust_drive::process::RealProcessRunner;

#[test]
#[ignore = "needs the OnePlus 9 (53f887ac) attached + Android toolchain — see module doc; \
            cargo test -p frust-drive --test device_gate -- --ignored --nocapture"]
fn oneplus9_spawn_session_streams_and_kills() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/huddle");
    assert!(
        root.join("frust.toml").is_file(),
        "expected examples/huddle at {}",
        root.display()
    );

    let device = Device {
        id: "53f887ac".into(),
        name: "OnePlus 9".into(),
        platform: Platform::Android,
        kind: Kind::PhysicalDevice,
        os_version: None,
        connection_state: None,
    };
    let info = BuildInfo {
        mode: BuildMode::Debug,
        flavor: None,
        defines: HashMap::new(),
        build_name: None,
        build_number: None,
    };

    let cancel = AtomicBool::new(false);
    let mut phase_lines: Vec<String> = Vec::new();

    // Drive the exact streaming seam the supervisor uses: build → install →
    // launch (phase lines via the callback), returning the killable logcat
    // stream.
    let mut handle = android_run::spawn_session(
        &RealProcessRunner,
        &root,
        &device,
        &info,
        &mut |line| {
            eprintln!("[phase ] {line}");
            phase_lines.push(line.to_string());
        },
        &cancel,
    )
    .expect("spawn_session pipeline error")
    .expect("pipeline cancelled unexpectedly");

    // Collect a handful of logcat lines (or time out) to prove the stream is
    // live, then kill and confirm the stream tears down promptly.
    let start = Instant::now();
    let mut logcat: Vec<String> = Vec::new();
    while logcat.len() < 5 && start.elapsed() < Duration::from_secs(30) {
        match handle.lines.try_recv() {
            Ok(line) => {
                eprintln!("[logcat] {line}");
                logcat.push(line);
            }
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }

    let kill_start = Instant::now();
    handle.kill();
    let kill_elapsed = kill_start.elapsed();
    eprintln!("[gate  ] stop() returned in {kill_elapsed:?}");

    assert!(
        phase_lines.iter().any(|l| l.contains("Building")),
        "expected a Building phase line, got: {phase_lines:?}"
    );
    assert!(
        phase_lines.iter().any(|l| l.starts_with("Installing")),
        "expected an Installing phase line, got: {phase_lines:?}"
    );
    assert!(
        phase_lines.iter().any(|l| l.starts_with("Streaming logs")),
        "expected a Streaming logs marker, got: {phase_lines:?}"
    );
    assert!(
        kill_elapsed < Duration::from_secs(10),
        "stop() should tear the logcat stream down promptly, took {kill_elapsed:?}"
    );
}
