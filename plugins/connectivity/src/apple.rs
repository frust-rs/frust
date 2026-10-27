//! The Apple arm (iOS and macOS): a Network.framework `nw_path_monitor` on a
//! private serial queue, reporting into [`crate::state::report`].
//!
//! Not implemented yet: [`start`] only logs, so nothing is reported and
//! [`crate::Connectivity::current`] stays `None`.

/// Start the platform monitor. Called once per process.
pub(crate) fn start() {
    log::debug!("frust-connectivity: apple arm not yet implemented");
}
