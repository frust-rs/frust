//! `start_barcode_stream`'s composition: [`DetectionPolicy`] (what an app
//! configures) and `DetectionFilter` (the pure state machine that
//! implements it), plus `build_callback` — the frame→decode→filter closure
//! [`crate::CameraSession::start_barcode_stream`] hands to the underlying
//! `start_image_stream(ImageFormat::Yuv420, ..)` call.
//!
//! # Why a policy at all
//!
//! A raw image stream decodes every frame at camera rate. For
//! `mobile_scanner`-style UX (a scan sheet, a live inventory count) that is
//! either wasted CPU — the same code re-decoded 30 times a second while
//! held in view — or a flood of duplicate detections an app has to
//! de-duplicate itself. [`DetectionPolicy`] moves that policy into the
//! plugin, matching `mobile_scanner`'s own three-way detection-mode split.
//!
//! # `DetectionFilter` is pure, no I/O
//!
//! Every input crossing its boundary ([`std::time::Instant`],
//! `Vec<Barcode>`) is a plain value it was handed, never read itself — that
//! is what makes it directly replay-testable below with synthetic frame
//! sequences and timestamps, no camera, no sleeping, no flakiness.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{Barcode, BarcodeFormat, BarcodeStreamOptions, decode_frame};
use crate::ImageFrame;

/// When [`crate::CameraSession::start_barcode_stream`]'s `on_detect` should
/// actually fire, relative to the underlying decode rate — three policies,
/// matching the `mobile_scanner` Flutter/Dart package's own detection-mode
/// split (empty/duplicate detections are exactly the UX problem it solves
/// for too).
///
/// `#[non_exhaustive]`: a later policy (e.g. a motion-debounced heuristic)
/// adds a variant here rather than widening some other type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DetectionPolicy {
    /// Emit each distinct `(format, raw_value)` once; a seen value re-arms
    /// — may emit again — only once it has been absent for
    /// `ABSENCE_FRAMES` (30) consecutive *processed* frames.
    /// `mobile_scanner` parity is "until it leaves view"; this crate has no
    /// separate lost-track signal, so it approximates that with a frame
    /// count instead.
    NoDuplicates,
    /// Decode — and therefore ever emit — at most once per `interval`.
    /// Frames arriving inside the window never even reach the decoder (the
    /// cheap path `DetectionFilter::should_decode` implements). Emits are
    /// **not** deduplicated: the same value re-emits every interval for as
    /// long as it stays in view.
    Throttled {
        /// The minimum time between decode attempts.
        interval: Duration,
    },
    /// Every non-empty decode emits — no throttling, no deduplication.
    Unrestricted,
}

impl Default for DetectionPolicy {
    /// [`Self::Throttled`] at `DEFAULT_THROTTLE_INTERVAL` (250ms) — matching
    /// `mobile_scanner`'s own default detection interval.
    fn default() -> Self {
        Self::Throttled {
            interval: DEFAULT_THROTTLE_INTERVAL,
        }
    }
}

/// [`DetectionPolicy::default`]'s interval.
const DEFAULT_THROTTLE_INTERVAL: Duration = Duration::from_millis(250);

/// How many consecutive *processed* frames a [`DetectionPolicy::NoDuplicates`]
/// value must be absent for before it re-arms.
///
/// **Community-approximate**: `mobile_scanner`'s "until it leaves view" has
/// no published frame count of its own — this crate has no separate
/// lost-track signal to substitute one for. 30 processed frames is roughly a
/// second at camera rate: comfortably past a single dropped/blurred frame's
/// false absence, while still re-arming quickly once the code is genuinely
/// gone.
const ABSENCE_FRAMES: u32 = 30;

/// The running state machine behind one [`DetectionPolicy`] — pure, no I/O,
/// which is what makes it directly unit-testable below with no camera
/// involved. [`Self::should_decode`] is the cheap-path gate a composition
/// calls *before* running the decoder at all; [`Self::push`] filters one
/// already-decoded frame's result.
struct DetectionFilter {
    policy: DetectionPolicy,
    /// [`DetectionPolicy::NoDuplicates`]'s bookkeeping: `(format,
    /// raw_value) -> frames since last seen`. Untouched by the other two
    /// policies, which need no memory.
    seen: HashMap<(BarcodeFormat, String), u32>,
    /// [`DetectionPolicy::Throttled`]'s bookkeeping: the last decode
    /// attempt's timestamp. `None` until the first attempt, so the very
    /// first frame always decodes.
    last_decode: Option<Instant>,
}

impl DetectionFilter {
    fn new(policy: DetectionPolicy) -> Self {
        Self {
            policy,
            seen: HashMap::new(),
            last_decode: None,
        }
    }

    /// Whether the composition should even call [`decode_frame`] for a
    /// frame arriving at `now` — [`DetectionPolicy::Throttled`]'s whole
    /// reason to exist: skip the decode entirely while still inside
    /// `interval`, rather than decoding and then discarding the result.
    /// Always `true` for [`DetectionPolicy::NoDuplicates`]/
    /// [`DetectionPolicy::Unrestricted`], which gate on the decode result
    /// instead, inside [`Self::push`].
    fn should_decode(&self, now: Instant) -> bool {
        match self.policy {
            DetectionPolicy::Throttled { interval } => match self.last_decode {
                Some(last) => now.saturating_duration_since(last) >= interval,
                None => true,
            },
            DetectionPolicy::NoDuplicates | DetectionPolicy::Unrestricted => true,
        }
    }

    /// Filter one already-decoded frame's raw detections — only call this
    /// after [`Self::should_decode`] returned `true` and a decode actually
    /// ran. Returns `Some(detections)` — **never** an empty `Vec` — exactly
    /// on the frame(s) `on_detect` should see; `None` on every other frame.
    fn push(&mut self, now: Instant, detections: Vec<Barcode>) -> Option<Vec<Barcode>> {
        let result = match self.policy {
            DetectionPolicy::Throttled { .. } => {
                self.last_decode = Some(now);
                if detections.is_empty() {
                    None
                } else {
                    Some(detections)
                }
            }
            DetectionPolicy::Unrestricted => {
                if detections.is_empty() {
                    None
                } else {
                    Some(detections)
                }
            }
            DetectionPolicy::NoDuplicates => self.push_no_duplicates(detections),
        };

        debug_assert!(
            match &result {
                Some(found) => !found.is_empty(),
                None => true,
            },
            "DetectionFilter::push must never emit an empty Vec"
        );
        result
    }

    /// [`DetectionPolicy::NoDuplicates`]'s own bookkeeping: assume every
    /// currently-tracked value was absent this frame, then correct each one
    /// actually redetected back to `0`; a value first seen (or re-armed
    /// after [`ABSENCE_FRAMES`] absent frames) is what gets emitted, never a
    /// repeat.
    fn push_no_duplicates(&mut self, detections: Vec<Barcode>) -> Option<Vec<Barcode>> {
        for counter in self.seen.values_mut() {
            *counter = counter.saturating_add(1);
        }

        let mut to_emit = Vec::new();
        for barcode in detections {
            let key = (barcode.format, barcode.raw_value.clone());
            match self.seen.get_mut(&key) {
                Some(counter) => *counter = 0,
                None => {
                    self.seen.insert(key, 0);
                    to_emit.push(barcode);
                }
            }
        }

        // Re-arm: a value absent for `ABSENCE_FRAMES` consecutive processed
        // frames is forgotten, so its next sighting emits again.
        self.seen.retain(|_, counter| *counter < ABSENCE_FRAMES);

        if to_emit.is_empty() {
            None
        } else {
            Some(to_emit)
        }
    }
}

/// Builds the frame callback [`crate::CameraSession::start_barcode_stream`]
/// hands to `start_image_stream(ImageFormat::Yuv420, ..)`: gate on
/// [`DetectionFilter::should_decode`] (the `Throttled` cheap path), decode
/// via [`decode_frame`], filter through [`DetectionFilter::push`], and
/// invoke `on_detect` only when it emits — never with an empty slice.
///
/// The filter lives behind a [`Mutex`] purely so the returned value stays a
/// plain `Fn` (matching [`crate::ImageFrameCallback`]'s bound) rather than
/// `FnMut`; both backends only ever invoke one session's callback from one
/// thread at a time (Android under `StreamSlot`'s own `Mutex`, Apple's
/// serial sample-buffer-delegate queue — see those modules' docs), so this
/// never actually contends.
pub(crate) fn build_callback(
    opts: BarcodeStreamOptions,
    on_detect: impl Fn(&[Barcode]) + Send + 'static,
) -> impl Fn(&ImageFrame<'_>) + Send + 'static {
    let filter = Mutex::new(DetectionFilter::new(opts.detection));
    let formats = opts.formats;
    move |frame: &ImageFrame<'_>| {
        let now = Instant::now();
        let found = {
            let mut filter = filter.lock().unwrap_or_else(|e| e.into_inner());
            if !filter.should_decode(now) {
                return;
            }
            let detections = decode_frame(frame, &formats);
            filter.push(now, detections)
        };
        if let Some(found) = found {
            on_detect(&found);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{ImageFormat, ImagePlane};

    fn sample_barcode(raw_value: &str) -> Barcode {
        Barcode {
            raw_value: raw_value.to_string(),
            raw_bytes: None,
            format: BarcodeFormat::QrCode,
            corners: None,
        }
    }

    // --- `DetectionFilter` unit tests (pure — synthetic instants, no real
    // camera, no sleeping) -------------------------------------------------

    #[test]
    fn throttled_gates_should_decode_on_the_interval() {
        let mut filter = DetectionFilter::new(DetectionPolicy::Throttled {
            interval: Duration::from_millis(100),
        });
        let t0 = Instant::now();
        assert!(filter.should_decode(t0), "the first frame always decodes");
        filter.push(t0, Vec::new());

        assert!(
            !filter.should_decode(t0 + Duration::from_millis(50)),
            "still inside the interval"
        );
        assert!(
            filter.should_decode(t0 + Duration::from_millis(100)),
            "interval elapsed"
        );
    }

    #[test]
    fn throttled_emits_at_most_once_per_interval_over_rapid_synthetic_frames() {
        let interval = Duration::from_millis(100);
        let mut filter = DetectionFilter::new(DetectionPolicy::Throttled { interval });
        let start = Instant::now();
        let mut emits = 0u32;

        // 1000 synthetic frames spaced 1ms apart, spanning exactly one
        // second — a much-faster-than-decode camera rate feeding the cheap
        // `should_decode` gate before ever reaching `push`.
        for i in 0..1000u64 {
            let now = start + Duration::from_millis(i);
            if filter.should_decode(now) {
                let emitted = filter.push(now, vec![sample_barcode("QR-A")]);
                if emitted.is_some() {
                    emits += 1;
                }
            }
        }

        // One second / 100ms interval => 10 windows; allow slack for
        // boundary rounding at the first/last window (the task's own "allow
        // timing slack" acceptance note).
        assert!(
            (9..=11).contains(&emits),
            "expected ~10 emits over 1s at a 100ms interval, got {emits}"
        );
    }

    #[test]
    fn unrestricted_emits_every_non_empty_decode_with_no_dedup() {
        let mut filter = DetectionFilter::new(DetectionPolicy::Unrestricted);
        let t0 = Instant::now();

        assert!(filter.should_decode(t0), "never gates on time");
        assert!(filter.push(t0, vec![sample_barcode("QR-A")]).is_some());
        assert!(
            filter.push(t0, vec![sample_barcode("QR-A")]).is_some(),
            "repeats still emit — no deduplication"
        );
        assert!(
            filter.push(t0, Vec::new()).is_none(),
            "an empty decode never emits"
        );
    }

    #[test]
    fn no_duplicates_emits_once_then_rearms_after_absence() {
        let mut filter = DetectionFilter::new(DetectionPolicy::NoDuplicates);
        let t0 = Instant::now();

        assert!(
            filter.push(t0, vec![sample_barcode("QR-A")]).is_some(),
            "first sighting emits"
        );
        for _ in 0..29 {
            assert!(
                filter.push(t0, vec![sample_barcode("QR-A")]).is_none(),
                "a repeat while still tracked never re-emits"
            );
        }
        for _ in 0..ABSENCE_FRAMES {
            assert!(
                filter.push(t0, Vec::new()).is_none(),
                "an absent frame never emits either"
            );
        }
        assert!(
            filter.push(t0, vec![sample_barcode("QR-A")]).is_some(),
            "re-armed after ABSENCE_FRAMES consecutive absent frames"
        );
    }

    // --- Composition tests: hand-built `ImageFrame`s over fixture luma
    // buffers reused from `barcode::conformance` -----------------------

    /// Feed one luma buffer through `callback` as a single Yuv420 frame —
    /// mirrors exactly what `start_image_stream`'s real callback receives
    /// (a borrowed plane valid only for the call), see [`ImageFrame`]'s own
    /// close-deadline contract.
    fn feed(callback: &impl Fn(&ImageFrame<'_>), buf: &[u8], side: usize) {
        let plane = ImagePlane {
            data: buf,
            row_stride: side,
            pixel_stride: 1,
        };
        let frame = ImageFrame {
            format: ImageFormat::Yuv420,
            width: side as u32,
            height: side as u32,
            rotation_degrees: 0,
            planes: std::slice::from_ref(&plane),
        };
        callback(&frame);
    }

    #[test]
    fn composition_no_duplicates_emits_once_then_rearms_after_absence() {
        let (qr_buf, side) = super::super::conformance::render_qr_luma("QR-A", 4, 0);
        let blank_buf = vec![0u8; qr_buf.len()];

        let emitted = Arc::new(Mutex::new(0usize));
        let emitted_clone = Arc::clone(&emitted);
        let callback = build_callback(
            BarcodeStreamOptions {
                formats: Vec::new(),
                detection: DetectionPolicy::NoDuplicates,
            },
            move |found: &[Barcode]| {
                assert!(!found.is_empty(), "on_detect never receives an empty slice");
                assert_eq!(found[0].raw_value, "QR-A");
                *emitted_clone.lock().unwrap() += 1;
            },
        );

        // 30 synthetic frames repeating fixture QR-A.
        for _ in 0..30 {
            feed(&callback, &qr_buf, side);
        }
        assert_eq!(
            *emitted.lock().unwrap(),
            1,
            "30 repeats of the same value emit exactly once"
        );

        // >= 30 blank frames, then QR-A's return.
        for _ in 0..ABSENCE_FRAMES {
            feed(&callback, &blank_buf, side);
        }
        feed(&callback, &qr_buf, side);
        assert_eq!(
            *emitted.lock().unwrap(),
            2,
            "re-emits once re-armed after ABSENCE_FRAMES blank frames"
        );
    }

    #[test]
    fn composition_unrestricted_emits_per_decodable_frame() {
        let (qr_buf, side) = super::super::conformance::render_qr_luma("QR-A", 4, 0);
        let blank_buf = vec![0u8; qr_buf.len()];

        let emitted = Arc::new(Mutex::new(Vec::<String>::new()));
        let emitted_clone = Arc::clone(&emitted);
        let callback = build_callback(
            BarcodeStreamOptions {
                formats: Vec::new(),
                detection: DetectionPolicy::Unrestricted,
            },
            move |found: &[Barcode]| {
                assert!(!found.is_empty(), "on_detect never receives an empty slice");
                emitted_clone
                    .lock()
                    .unwrap()
                    .extend(found.iter().map(|b| b.raw_value.clone()));
            },
        );

        feed(&callback, &qr_buf, side);
        feed(&callback, &blank_buf, side);
        feed(&callback, &qr_buf, side);

        assert_eq!(
            *emitted.lock().unwrap(),
            vec!["QR-A".to_string(), "QR-A".to_string()],
            "every decodable frame emits, the blank frame in between does not"
        );
    }

    #[test]
    fn on_detect_never_fires_for_an_all_blank_stream() {
        let (_qr_buf, side) = super::super::conformance::render_qr_luma("QR-A", 4, 0);
        let blank_buf = vec![0u8; side * side];

        for policy in [
            DetectionPolicy::NoDuplicates,
            DetectionPolicy::Throttled {
                interval: Duration::from_millis(1),
            },
            DetectionPolicy::Unrestricted,
        ] {
            let fired = Arc::new(Mutex::new(false));
            let fired_clone = Arc::clone(&fired);
            let callback = build_callback(
                BarcodeStreamOptions {
                    formats: Vec::new(),
                    detection: policy,
                },
                move |_found| {
                    *fired_clone.lock().unwrap() = true;
                },
            );
            for _ in 0..5 {
                feed(&callback, &blank_buf, side);
            }
            assert!(
                !*fired.lock().unwrap(),
                "{policy:?} must never call on_detect for an all-blank stream"
            );
        }
    }
}
