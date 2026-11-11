//! Reading one library asset's EXIF: the options type and the platform-independent half of
//! [`PhotoLibrary::asset_metadata`](crate::PhotoLibrary::asset_metadata).

use std::fmt;

use crate::cancel::CancelToken;

/// Options for [`PhotoLibrary::asset_metadata`](crate::PhotoLibrary::asset_metadata).
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct MetadataOptions {
    /// Whether an original that is only in iCloud may be downloaded to read its metadata
    /// (default `false`). When `false` such an asset fails with
    /// [`PhotoError::NetworkRequired`](crate::PhotoError::NetworkRequired). Android makes no
    /// network distinction, so this has no effect there.
    pub allow_network: bool,
    // Read only by the metadata paths, which the `library` feature compiles.
    #[cfg_attr(not(feature = "library"), allow(dead_code))]
    pub(crate) cancel: Option<CancelToken>,
}

impl MetadataOptions {
    /// Local data only: no network download.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets whether an iCloud-only original may be downloaded.
    pub fn allow_network(mut self, allow: bool) -> Self {
        self.allow_network = allow;
        self
    }

    /// Lets the caller cancel the read through `token`.
    pub fn cancel(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// Whether the caller's token, if any, has been cancelled.
    #[cfg(feature = "library")]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(CancelToken::is_cancelled)
    }
}

impl fmt::Debug for MetadataOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetadataOptions")
            .field("allow_network", &self.allow_network)
            .field("cancel", &self.cancel.is_some())
            .finish()
    }
}

/// A cancelled token first, then `read`: the whole of
/// [`PhotoLibrary::asset_metadata`](crate::PhotoLibrary::asset_metadata). `read` is the platform
/// arm, which refuses its UI thread itself; tests pass fakes.
#[cfg(feature = "library")]
pub(crate) fn asset_metadata_with<R>(
    id: &crate::AssetId,
    options: &MetadataOptions,
    read: R,
) -> Result<crate::ExifData, crate::PhotoError>
where
    R: FnOnce(&crate::AssetId, &MetadataOptions) -> Result<crate::ExifData, crate::PhotoError>,
{
    if options.is_cancelled() {
        return Err(crate::PhotoError::Cancelled);
    }
    read(id, options)
}

/// The platform-independent half of a streamed metadata read: the bounded buffer the chunks go
/// into and the wait for the transfer to end. Only the iOS arm streams an original into memory,
/// but the logic is plain Rust, so it compiles and is tested on every host.
#[cfg(feature = "library")]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) mod transfer {
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::time::{Duration, Instant};

    use crate::PhotoError;

    /// What the transfer reports to the waiting thread.
    #[derive(Debug, PartialEq)]
    pub(crate) enum Event {
        /// A chunk was buffered: the transfer is alive.
        Chunk,
        /// The transfer ended, or the buffer refused a chunk and the read must stop.
        Done(Result<(), PhotoError>),
    }

    /// How the wait paces itself.
    #[derive(Clone, Copy, Debug)]
    pub(crate) struct Timing {
        /// How often the wait wakes to look for a cancel.
        pub(crate) poll: Duration,
        /// How long the transfer may go without a chunk or its completion before it is
        /// abandoned.
        pub(crate) inactivity: Duration,
    }

    /// The error of a read abandoned by the inactivity deadline.
    pub(crate) fn timed_out(inactivity: Duration) -> PhotoError {
        PhotoError::Platform(format!(
            "reading the asset timed out: no data for {} s",
            inactivity.as_secs()
        ))
    }

    /// The error of an original over the byte ceiling.
    pub(crate) fn too_large() -> PhotoError {
        PhotoError::InvalidRequest {
            reason: "the original is too large for a metadata read",
        }
    }

    /// The bytes received so far, bounded by a ceiling, or the reason the read stopped growing
    /// them.
    pub(crate) struct Received {
        bytes: Vec<u8>,
        limit: usize,
        failure: Option<PhotoError>,
    }

    impl Received {
        /// An empty buffer that refuses to grow past `limit` bytes.
        pub(crate) fn with_limit(limit: usize) -> Self {
            Self {
                bytes: Vec::new(),
                limit,
                failure: None,
            }
        }

        /// Appends one chunk. A chunk that would take the total past the ceiling, or one the
        /// allocator refuses room for, frees the buffer and fails the read; once failed, every
        /// later chunk is dropped and answers the same error.
        ///
        /// Room grows by doubling, capped at the ceiling, so the capacity is at most twice the
        /// bytes received and never above the ceiling.
        pub(crate) fn append(&mut self, chunk: &[u8]) -> Result<(), PhotoError> {
            if let Some(failure) = &self.failure {
                return Err(failure.clone());
            }
            let needed = match self.bytes.len().checked_add(chunk.len()) {
                Some(needed) if needed <= self.limit => needed,
                _ => return Err(self.fail(too_large())),
            };
            self.reserve(needed)?;
            self.bytes.extend_from_slice(chunk);
            Ok(())
        }

        /// Makes room for `needed` bytes in total (at most the ceiling, checked by the caller).
        fn reserve(&mut self, needed: usize) -> Result<(), PhotoError> {
            if needed <= self.bytes.capacity() {
                return Ok(());
            }
            let target = needed
                .max(self.bytes.capacity().saturating_mul(2))
                .min(self.limit);
            if self
                .bytes
                .try_reserve_exact(target - self.bytes.len())
                .is_err()
            {
                return Err(self.fail(PhotoError::Platform(
                    "not enough memory for the asset's data".to_owned(),
                )));
            }
            Ok(())
        }

        /// Frees the buffer and drops every later chunk; for a read the caller has abandoned.
        pub(crate) fn discard(&mut self) {
            if self.failure.is_none() {
                self.fail(PhotoError::Cancelled);
            }
        }

        /// The bytes of a transfer that completed without an error, or the error that stopped
        /// the buffer.
        pub(crate) fn finish(&mut self) -> Result<Vec<u8>, PhotoError> {
            match &self.failure {
                Some(failure) => Err(failure.clone()),
                None => Ok(std::mem::take(&mut self.bytes)),
            }
        }

        fn fail(&mut self, error: PhotoError) -> PhotoError {
            self.bytes = Vec::new();
            self.failure = Some(error.clone());
            error
        }
    }

    /// Waits for the transfer to end. Returns its outcome, [`PhotoError::Cancelled`] once
    /// `cancelled` answers `true` (checked every [`Timing::poll`] and after every chunk), or
    /// [`timed_out`] when neither a chunk nor the completion arrives for
    /// [`Timing::inactivity`]. Every chunk restarts the deadline.
    pub(crate) fn wait_for_completion(
        rx: &Receiver<Event>,
        timing: Timing,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), PhotoError> {
        let origin = Instant::now();
        wait_with(
            |timeout| rx.recv_timeout(timeout),
            || origin.elapsed(),
            timing,
            cancelled,
        )
    }

    /// [`wait_for_completion`] over an injected receive and clock (`now` is the time elapsed
    /// since a fixed origin), so tests never sleep.
    fn wait_with(
        mut next: impl FnMut(Duration) -> Result<Event, RecvTimeoutError>,
        now: impl Fn() -> Duration,
        timing: Timing,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), PhotoError> {
        let mut last_activity = now();
        loop {
            let idle = now().saturating_sub(last_activity);
            let left = timing.inactivity.saturating_sub(idle);
            match next(timing.poll.min(left)) {
                Ok(Event::Done(outcome)) => return outcome,
                Ok(Event::Chunk) => {
                    last_activity = now();
                    if cancelled() {
                        return Err(PhotoError::Cancelled);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if cancelled() {
                        return Err(PhotoError::Cancelled);
                    }
                    if now().saturating_sub(last_activity) >= timing.inactivity {
                        return Err(timed_out(timing.inactivity));
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(PhotoError::Platform(
                        "Photos dropped the read without answering".to_owned(),
                    ));
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::cell::{Cell, RefCell};
        use std::collections::VecDeque;
        use std::sync::mpsc;

        const TIMING: Timing = Timing {
            poll: Duration::from_millis(100),
            inactivity: Duration::from_secs(1),
        };

        /// One scripted answer of the fake receive.
        enum Step {
            /// Nothing arrives: the receive times out after the full timeout it was given.
            Quiet,
            /// `event` arrives `after` into the receive (at most its timeout).
            Arrive(Duration, Event),
        }

        /// Runs [`wait_with`] over `script` on a fake clock; once the script is spent the
        /// transfer goes quiet. Returns the outcome and the fake time it took.
        fn run(
            script: Vec<Step>,
            cancelled: &dyn Fn() -> bool,
        ) -> (Result<(), PhotoError>, Duration) {
            let clock = Cell::new(Duration::ZERO);
            let script = RefCell::new(VecDeque::from(script));
            let outcome = wait_with(
                |timeout| match script.borrow_mut().pop_front().unwrap_or(Step::Quiet) {
                    Step::Quiet => {
                        clock.set(clock.get() + timeout);
                        Err(RecvTimeoutError::Timeout)
                    }
                    Step::Arrive(after, event) => {
                        assert!(after <= timeout, "an event cannot arrive after the timeout");
                        clock.set(clock.get() + after);
                        Ok(event)
                    }
                },
                || clock.get(),
                TIMING,
                cancelled,
            );
            (outcome, clock.get())
        }

        #[test]
        fn no_chunk_and_no_completion_times_out_at_the_deadline() {
            let (outcome, took) = run(Vec::new(), &|| false);
            assert_eq!(outcome, Err(timed_out(TIMING.inactivity)));
            assert_eq!(took, TIMING.inactivity);
            assert!(
                matches!(&outcome, Err(PhotoError::Platform(m)) if m.contains("timed out")),
                "{outcome:?}"
            );
        }

        #[test]
        fn chunks_keep_a_long_transfer_alive_and_each_restarts_the_deadline() {
            // Ten chunks, each 900 ms after the last: ten times the deadline in total, but never
            // a full second without data.
            let mut script = Vec::new();
            for _ in 0..10 {
                script.extend((0..9).map(|_| Step::Quiet));
                script.push(Step::Arrive(Duration::ZERO, Event::Chunk));
            }
            script.push(Step::Arrive(Duration::ZERO, Event::Done(Ok(()))));
            let (outcome, took) = run(script, &|| false);
            assert_eq!(outcome, Ok(()));
            assert_eq!(took, Duration::from_millis(9000));

            // After the last chunk the deadline is counted from that chunk, not from the start.
            let mut script: Vec<Step> = (0..5).map(|_| Step::Quiet).collect();
            script.push(Step::Arrive(Duration::from_millis(50), Event::Chunk));
            let (outcome, took) = run(script, &|| false);
            assert_eq!(outcome, Err(timed_out(TIMING.inactivity)));
            assert_eq!(took, Duration::from_millis(550) + TIMING.inactivity);
        }

        #[test]
        fn a_cancel_wins_over_quiet_and_over_chunks() {
            let (outcome, took) = run(Vec::new(), &|| true);
            assert_eq!(outcome, Err(PhotoError::Cancelled));
            assert_eq!(took, TIMING.poll);

            let script = (0..100)
                .map(|_| Step::Arrive(Duration::ZERO, Event::Chunk))
                .collect();
            let (outcome, _) = run(script, &|| true);
            assert_eq!(outcome, Err(PhotoError::Cancelled));
        }

        #[test]
        fn the_completion_is_returned_and_a_dropped_sender_is_an_error() {
            let script = vec![
                Step::Quiet,
                Step::Arrive(
                    Duration::ZERO,
                    Event::Done(Err(PhotoError::NetworkRequired)),
                ),
            ];
            assert_eq!(run(script, &|| false).0, Err(PhotoError::NetworkRequired));

            let (tx, rx) = mpsc::channel::<Event>();
            drop(tx);
            assert!(matches!(
                wait_for_completion(&rx, TIMING, &|| false),
                Err(PhotoError::Platform(_))
            ));
        }

        #[test]
        fn the_channel_wait_times_out_on_a_short_interval() {
            let (_tx, rx) = mpsc::channel::<Event>();
            let timing = Timing {
                poll: Duration::from_millis(5),
                inactivity: Duration::from_millis(20),
            };
            assert_eq!(
                wait_for_completion(&rx, timing, &|| false),
                Err(timed_out(timing.inactivity))
            );
            let (tx, rx) = mpsc::channel();
            tx.send(Event::Chunk).unwrap();
            tx.send(Event::Done(Ok(()))).unwrap();
            assert_eq!(wait_for_completion(&rx, timing, &|| false), Ok(()));
        }

        #[test]
        fn chunks_append_in_order_up_to_the_ceiling_and_not_one_byte_past_it() {
            let mut received = Received::with_limit(6);
            assert_eq!(received.append(b"ab"), Ok(()));
            assert_eq!(received.append(b""), Ok(()));
            assert_eq!(received.append(b"cdef"), Ok(()));
            assert!(
                received.bytes.capacity() <= 6,
                "never reserves past the ceiling"
            );
            assert_eq!(received.finish(), Ok(b"abcdef".to_vec()));

            let mut received = Received::with_limit(6);
            assert_eq!(received.append(b"abcdef"), Ok(()));
            assert_eq!(received.append(b"g"), Err(too_large()));
            assert_eq!(received.bytes.capacity(), 0, "the buffer is freed");
            // Later chunks are dropped and answer the same error.
            assert_eq!(received.append(b"h"), Err(too_large()));
            assert_eq!(received.finish(), Err(too_large()));

            // One chunk over the ceiling on its own.
            let mut received = Received::with_limit(6);
            assert_eq!(received.append(b"abcdefg"), Err(too_large()));
            assert!(received.bytes.is_empty());
        }

        #[test]
        fn capacity_doubles_but_stays_within_the_ceiling() {
            let mut received = Received::with_limit(100);
            received.append(&[0; 30]).unwrap();
            assert_eq!(received.bytes.capacity(), 30);
            received.append(&[0; 1]).unwrap();
            assert_eq!(received.bytes.capacity(), 60);
            received.append(&[0; 40]).unwrap();
            assert_eq!(received.bytes.capacity(), 100);
            assert_eq!(received.bytes.len(), 71);
        }

        #[test]
        fn a_refused_allocation_fails_the_read() {
            let mut received = Received::with_limit(usize::MAX);
            received.append(b"ab").unwrap();
            // Room past `isize::MAX` bytes is refused by `try_reserve_exact`, never an abort.
            assert!(matches!(
                received.reserve(usize::MAX / 2 + 10),
                Err(PhotoError::Platform(_))
            ));
            assert!(received.bytes.is_empty(), "the buffer is freed");
            assert!(matches!(
                received.append(b"cd"),
                Err(PhotoError::Platform(_))
            ));
            assert!(matches!(received.finish(), Err(PhotoError::Platform(_))));
        }

        #[test]
        fn a_discarded_read_drops_later_chunks() {
            let mut received = Received::with_limit(10);
            received.append(b"ab").unwrap();
            received.discard();
            assert!(received.bytes.is_empty());
            assert_eq!(received.append(b"cd"), Err(PhotoError::Cancelled));
            // A failure already recorded is kept.
            let mut received = Received::with_limit(1);
            assert_eq!(received.append(b"ab"), Err(too_large()));
            received.discard();
            assert_eq!(received.finish(), Err(too_large()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AssetId, PhotoError, PhotoLibrary};

    #[test]
    fn options_default_to_local_data_and_never_print_the_token() {
        let o = MetadataOptions::new();
        assert!(!o.allow_network);
        assert!(o.cancel.is_none());
        assert_eq!(
            format!("{o:?}"),
            "MetadataOptions { allow_network: false, cancel: false }"
        );
        let o = o.allow_network(true).cancel(CancelToken::new());
        assert!(o.allow_network);
        assert_eq!(
            format!("{o:?}"),
            "MetadataOptions { allow_network: true, cancel: true }"
        );
    }

    #[cfg(not(feature = "library"))]
    #[test]
    fn asset_metadata_is_feature_gated() {
        let token = CancelToken::new();
        token.cancel();
        for options in [MetadataOptions::new(), MetadataOptions::new().cancel(token)] {
            assert_eq!(
                PhotoLibrary::asset_metadata(&AssetId::from_raw("a"), &options),
                Err(PhotoError::FeatureDisabled { feature: "library" })
            );
        }
    }

    #[cfg(feature = "library")]
    #[test]
    fn a_cancelled_token_stops_the_read_before_the_platform() {
        let token = CancelToken::new();
        token.cancel();
        let options = MetadataOptions::new().cancel(token);
        assert_eq!(
            asset_metadata_with(&AssetId::from_raw("a"), &options, |_, _| {
                panic!("the platform must not run after a cancel")
            }),
            Err(PhotoError::Cancelled)
        );
        // Through the public call as well, on every platform.
        assert_eq!(
            PhotoLibrary::asset_metadata(&AssetId::from_raw("a"), &options),
            Err(PhotoError::Cancelled)
        );
    }

    #[cfg(feature = "library")]
    #[test]
    fn the_platform_answer_is_returned_unchanged() {
        use crate::ExifData;
        let exif = ExifData {
            camera_make: Some("Frust".to_owned()),
            ..ExifData::default()
        };
        let passed = exif.clone();
        let options = MetadataOptions::new().allow_network(true);
        assert_eq!(
            asset_metadata_with(&AssetId::from_raw("a"), &options, |id, o| {
                assert_eq!(id.as_str(), "a");
                assert!(o.allow_network);
                Ok(passed)
            }),
            Ok(exif)
        );
        // An asset without EXIF is an empty value, not an error.
        assert_eq!(
            asset_metadata_with(&AssetId::from_raw("a"), &options, |_, _| Ok(
                ExifData::default()
            )),
            Ok(ExifData::default())
        );
        assert_eq!(
            asset_metadata_with(&AssetId::from_raw("a"), &options, |_, _| Err(
                PhotoError::NetworkRequired
            )),
            Err(PhotoError::NetworkRequired)
        );
    }

    #[cfg(all(
        feature = "library",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    #[test]
    fn desktop_asset_metadata_is_unsupported() {
        assert_eq!(
            PhotoLibrary::asset_metadata(&AssetId::from_raw("a"), &MetadataOptions::new()),
            Err(PhotoError::Unsupported)
        );
    }
}
