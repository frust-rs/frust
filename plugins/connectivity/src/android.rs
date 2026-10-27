//! The Android arm: `ConnectivityManager.registerDefaultNetworkCallback`
//! through the plugin's `FrustConnectivityHost`, reporting into
//! [`crate::state::report`].
//!
//! Not implemented yet: [`start`] only logs, so nothing is reported and
//! [`crate::Connectivity::current`] stays `None`.

/// Start the platform monitor. Called once per process.
pub(crate) fn start() {
    log::debug!("frust-connectivity: android arm not yet implemented");
}
