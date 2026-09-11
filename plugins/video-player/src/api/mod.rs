//! The app-facing half of this crate, behind the default-on `frust-api`
//! feature: the declarative view that pairs a [`crate::PlayerSession`] with
//! its native platform-view slot, so an app writes a video the way it writes
//! any other widget rather than wiring `view_type`/`params_json` by hand.
//!
//! Nothing is published here yet. The module exists so the feature it is
//! gated by — and therefore the charter line `--no-default-features` holds —
//! is real and exercised from the first commit: this crate's default build
//! genuinely depends on `frust`, and its `--no-default-features` build
//! genuinely does not.

// The `frust` dependency this feature gates, named so that enabling the
// feature actually pulls the facade into the graph (and disabling it
// actually removes it) before any view type here uses it.
#[allow(unused_imports)]
use frust as _;
