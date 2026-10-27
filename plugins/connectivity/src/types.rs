//! The value types a [`crate::ConnectivityEvent`] carries.

/// Identifies one network for as long as the platform keeps it.
///
/// Opaque and process-local: the number means nothing outside this process
/// and nothing across a restart, and its only use is equality — two reports
/// with the same id are the same network (Android: the same `Network`
/// handle; Apple: the same path interface). Never persist or compare it
/// numerically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NetworkId(u64);

impl NetworkId {
    /// Wrap a platform arm's raw network identity.
    // Only the Android/Apple arms (and tests) construct one; a no-op build
    // never does.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw value, for logging or for keying a map. Meaningful only for
    /// equality — see the type docs.
    #[must_use]
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// The link the default network runs over.
///
/// A VPN, and any transport the platform does not name, maps to
/// [`Transport::Other`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Transport {
    /// Wi-Fi.
    Wifi,
    /// A cellular radio.
    Cellular,
    /// Wired Ethernet.
    Ethernet,
    /// A VPN, or anything else the platform reports.
    Other,
}

/// The default network as the platform last described it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct NetworkPath {
    /// Which network this is; equality-only (see [`NetworkId`]).
    pub id: NetworkId,
    /// The link it runs over.
    pub transport: Transport,
    /// The platform considers this network metered or expensive (cellular,
    /// a personal hotspot).
    pub expensive: bool,
    /// The platform asks apps to restrict data on it (Data Saver, Low Data
    /// Mode).
    pub constrained: bool,
}

/// A change in default-network availability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConnectivityEvent {
    /// A usable default network: on Android one with both `INTERNET` and
    /// `VALIDATED`, on Apple a path whose status is satisfied.
    ///
    /// Reported when the default network first becomes usable, and again
    /// whenever it changes to a different network or transport. A change of
    /// only [`NetworkPath::expensive`]/[`NetworkPath::constrained`] on the
    /// same network is not an event; [`crate::Connectivity::current`]
    /// reflects it.
    Available {
        /// The network that is now the default.
        path: NetworkPath,
        /// `true` for the first report after the monitor starts, `false` for
        /// every later one.
        initial: bool,
    },
    /// The usable default network went away and no other replaced it.
    Lost,
}
