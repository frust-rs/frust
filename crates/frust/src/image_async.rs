//! Async image decode: [`decode_image_async`] is Frust's
//! off-thread counterpart to the existing synchronous
//! [`ImageSource::decode`](frust_widgets::ImageSource::decode) — a thin
//! `frust_reactive::spawn_blocking` wrapper meant to compose with
//! [`use_task`](frust_reactive::use_task):
//!
//! ```ignore
//! let image = use_task(|| async { frust::decode_image_async(bytes).await });
//! ```
//!
//! # Why this lives in the facade, not `frust-widgets`
//!
//! `frust-widgets` is reactive-free by charter (`docs/ARCHITECTURE.md`'s Layer
//! Dependencies: `frust-widgets = core + scene + text + theme`, no
//! `frust-reactive`/tokio edge) — adding an async decode entry point there
//! would pull `frust-reactive` (and transitively tokio) into a crate that
//! must stay usable with no reactive runtime at all (bare-core tests, a
//! pre-reactive app). The synchronous `ImageSource::decode` stays exactly
//! where it is, unchanged, for that reason and for embedded-asset callers
//! that have no need to leave the calling thread. `decode_image_async` is a
//! plain function over the facade's own `frust-reactive`/`frust-widgets`
//! dependencies — no new crate, no widget-side change.
//!
//! # The Arc move
//!
//! [`ImageSource`] is already an `Arc`-backed handle
//! (`frust-widgets::image`'s module docs): decoding happens exactly once,
//! inside the `spawn_blocking` closure, and the resulting `ImageSource`
//! *moves* back across the thread boundary — the returned value on the UI
//! thread shares the same underlying `Arc<peniko::ImageData>` allocation the
//! background thread decoded into, observable via
//! [`ImageSource::same`](frust_widgets::ImageSource::same) (an `Arc` pointer
//! check, never a pixel comparison). No second decode, no pixel copy.

use frust_reactive::spawn_blocking;
use frust_widgets::{ImageError, ImageSource};

/// The error [`decode_image_async`] can fail with.
///
/// Two distinct failure modes, kept apart rather than collapsed into one
/// message: the synchronous decode itself can fail
/// ([`Decode`](Self::Decode), wrapping the existing
/// [`ImageError`](frust_widgets::ImageError) unchanged), or the background
/// `spawn_blocking` task can fail to *run* to completion at all
/// ([`TaskFailed`](Self::TaskFailed) — a panic inside the decode closure, or
/// the task being dropped/aborted before it finished; mirrors
/// `tokio::task::JoinError`'s own two cases). The join-failure text is kept
/// as a plain `String` here rather than naming `tokio::task::JoinError`
/// directly: `frust` has no *direct* dependency on `tokio` (only a
/// transitive one through `frust-reactive`), and this is the one place that
/// would otherwise need one just to name the type.
#[derive(Debug)]
pub enum ImageDecodeError {
    /// `ImageSource::decode` ran to completion and reported a decode
    /// failure (bad bytes, unsupported format, ...).
    Decode(ImageError),
    /// The background decode task itself panicked or was cancelled before it
    /// could report a result.
    TaskFailed(String),
}

impl std::fmt::Display for ImageDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageDecodeError::Decode(e) => write!(f, "{e}"),
            ImageDecodeError::TaskFailed(msg) => write!(f, "image decode task failed: {msg}"),
        }
    }
}

impl std::error::Error for ImageDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ImageDecodeError::Decode(e) => Some(e),
            ImageDecodeError::TaskFailed(_) => None,
        }
    }
}

/// Decodes PNG/JPEG `bytes` off the UI thread via
/// [`frust_reactive::spawn_blocking`], returning the same
/// [`ImageSource`](frust_widgets::ImageSource) [`ImageSource::decode`] would
/// build synchronously.
///
/// `bytes` moves into the background closure (no copy beyond the initial
/// `Vec` transfer); the decoded [`ImageSource`] moves back — see this
/// module's docs for the Arc-move contract.
///
/// Compose with [`use_task`](frust_reactive::use_task) for the full
/// load/error/ready idiom:
///
/// ```ignore
/// let image = use_task(|| async { frust::decode_image_async(bytes.clone()).await });
/// ```
pub async fn decode_image_async(bytes: Vec<u8>) -> Result<ImageSource, ImageDecodeError> {
    decode_with_probe(
        bytes,
        |_thread_id, _result: Result<&ImageSource, &ImageError>| {},
    )
    .await
}

/// The shared decode entry both [`decode_image_async`] and this module's
/// tests call: identical to `decode_image_async`, except a `probe` closure
/// runs *inside* the `spawn_blocking` closure, immediately after decoding and
/// before the result crosses back to the caller — the hooked/spied path the
/// tests below use to observe the executing thread id and to clone the
/// decoded `ImageSource`'s `Arc` before the cross, so it can be compared by
/// pointer identity against the value that arrives on the calling thread.
async fn decode_with_probe<P>(bytes: Vec<u8>, probe: P) -> Result<ImageSource, ImageDecodeError>
where
    P: FnOnce(std::thread::ThreadId, Result<&ImageSource, &ImageError>) + Send + 'static,
{
    let outcome = spawn_blocking(move || {
        let result = ImageSource::decode(&bytes);
        probe(std::thread::current().id(), result.as_ref());
        result
    })
    .await;

    match outcome {
        Ok(inner) => inner.map_err(ImageDecodeError::Decode),
        Err(join_err) => Err(ImageDecodeError::TaskFailed(join_err.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_reactive::{Owner, ReactiveRuntime, use_task};
    use reactive_graph::traits::GetUntracked;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// Pump the UI-thread local queue until `cond` holds or the deadline
    /// passes; returns whether `cond` became true. Mirrors
    /// `frust-reactive::task`'s own test helper — the background
    /// `spawn_blocking` work runs on the reactive runtime's own worker
    /// threads regardless; this only drains the UI-side coordinator that
    /// awaits it.
    fn pump_until(rt: &ReactiveRuntime, timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        loop {
            rt.pump_local();
            if cond() {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// A minimal, hand-encoded 2x2 RGBA PNG, built via the `image` crate's own
    /// encoder so this test has no external fixture file to keep in sync
    /// (mirrors `frust-widgets::image`'s own test fixture).
    fn two_by_two_png() -> Vec<u8> {
        use image::{ImageEncoder, codecs::png::PngEncoder};
        let pixels: [u8; 2 * 2 * 4] = [
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 0, 255, // yellow
        ];
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(&pixels, 2, 2, image::ExtendedColorType::Rgba8)
            .expect("encoding a tiny in-memory PNG must not fail");
        out
    }

    /// Acceptance criterion 1 + 2: `decode_image_async`'s underlying decode
    /// runs off the calling (UI) thread, the decoded `ImageSource` crosses
    /// back as an `Arc` move (same allocation observable via
    /// `ImageSource::same` on a clone taken before/after the cross), and the
    /// whole thing composes with `use_task`.
    #[test]
    fn decode_runs_off_ui_thread_and_result_is_an_arc_move_via_use_task() {
        let rt = ReactiveRuntime::init(Arc::new(|| {}));
        let ui_thread = std::thread::current().id();

        let observed_thread: Arc<Mutex<Option<std::thread::ThreadId>>> = Arc::new(Mutex::new(None));
        // Cloned *inside* the spawn_blocking closure, before the result
        // crosses back to the UI thread — the "before" half of the
        // before/after Arc-identity check.
        let before_cross: Arc<Mutex<Option<ImageSource>>> = Arc::new(Mutex::new(None));

        let bytes = two_by_two_png();
        let owner = Owner::new();
        let task = {
            let observed_thread = observed_thread.clone();
            let before_cross = before_cross.clone();
            owner.with(|| {
                use_task(move || {
                    let observed_thread = observed_thread.clone();
                    let before_cross = before_cross.clone();
                    let bytes = bytes.clone();
                    async move {
                        decode_with_probe(bytes, move |thread_id, result| {
                            *observed_thread.lock().expect("observed_thread poisoned") =
                                Some(thread_id);
                            if let Ok(source) = result {
                                *before_cross.lock().expect("before_cross poisoned") =
                                    Some(source.clone());
                            }
                        })
                        .await
                    }
                })
            })
        };

        assert!(
            pump_until(rt, Duration::from_secs(5), || task
                .signal()
                .get_untracked()
                .is_ready()),
            "decode_image_async must resolve to Ready via use_task"
        );

        let decoded_thread = observed_thread
            .lock()
            .expect("observed_thread poisoned")
            .expect("probe must have recorded a thread id");
        assert_ne!(
            decoded_thread, ui_thread,
            "the decode must run off the calling (UI) thread"
        );

        let after_cross = task
            .signal()
            .get_untracked()
            .ready()
            .cloned()
            .expect("Ready must carry the decoded ImageSource");
        let before_cross = before_cross
            .lock()
            .expect("before_cross poisoned")
            .clone()
            .expect("probe must have captured a pre-cross clone");
        assert!(
            before_cross.same(&after_cross),
            "the ImageSource observed inside spawn_blocking (pre-cross) must share the \
             same Arc allocation as the value observed on the UI thread (post-cross) — \
             the result crosses as an Arc move, never a re-decode or pixel copy"
        );

        owner.cleanup();
    }

    /// The ordinary happy path, exactly at the call site the module docs
    /// promise (`use_task(|| async { frust::decode_image_async(bytes).await
    /// })`), with no probe involved.
    #[test]
    fn decode_image_async_composes_with_use_task() {
        let rt = ReactiveRuntime::init(Arc::new(|| {}));
        let bytes = two_by_two_png();
        let owner = Owner::new();
        let task = owner.with(|| {
            use_task(move || {
                let bytes = bytes.clone();
                async move { decode_image_async(bytes).await }
            })
        });

        assert!(
            pump_until(rt, Duration::from_secs(5), || task
                .signal()
                .get_untracked()
                .is_ready()),
            "a valid PNG must decode to Ready"
        );
        let source = task.signal().get_untracked().ready().cloned().unwrap();
        assert_eq!(source.natural_size(), kurbo::Size::new(2.0, 2.0));

        owner.cleanup();
    }

    /// A decode failure surfaces through `use_task`'s `Error` state, carrying
    /// `ImageDecodeError` — proving the error type composes with `use_task`'s
    /// `E: std::error::Error + Send + Sync + 'static` bound.
    #[test]
    fn decode_failure_surfaces_as_use_task_error() {
        let rt = ReactiveRuntime::init(Arc::new(|| {}));
        let owner = Owner::new();
        let task = owner
            .with(|| use_task(|| async { decode_image_async(b"not an image".to_vec()).await }));

        assert!(
            pump_until(rt, Duration::from_secs(5), || task
                .signal()
                .get_untracked()
                .is_error()),
            "invalid bytes must resolve to Error"
        );
        assert!(matches!(
            task.signal().get_untracked().error().map(|e| e.to_string()),
            Some(msg) if msg.contains("failed to decode image")
        ));

        owner.cleanup();
    }
}
