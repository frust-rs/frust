//! [`NetworkImages`]: the app-wide [`ImageLoader`] every `network_image` view
//! falls back to.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use frust::{provide_context, use_context};

use crate::fetch::ImageFetcher;
use crate::loader::{ImageLoader, LoaderConfig};

/// The process-wide loader; the first install wins.
static GLOBAL: OnceLock<Arc<ImageLoader>> = OnceLock::new();

/// The context value [`NetworkImages::install_with`] provides.
#[derive(Clone)]
struct InstalledLoader(Arc<ImageLoader>);

/// Installs and resolves the shared [`ImageLoader`].
///
/// Call [`install`](Self::install) (or [`install_with`](Self::install_with))
/// once from `frust::app!`'s `setup = { .. }` block, the same call site a
/// design system's `install()` uses. The loader is stored process-wide and,
/// because `setup` runs under the root reactive owner, also provided as
/// context to the whole tree.
pub struct NetworkImages;

impl NetworkImages {
    /// Install a loader over the default `ReqwestFetcher` and
    /// [`LoaderConfig::default`]. A fetcher that fails to build is logged and
    /// nothing is installed, so views render their error slot.
    ///
    /// ```no_run
    /// use frust::{Component, View, text};
    /// use frust_network_image::NetworkImages;
    ///
    /// #[derive(Default)]
    /// pub struct App;
    ///
    /// impl Component for App {
    ///     type State = ();
    ///
    ///     fn init(&self) {}
    ///
    ///     fn build(&self, _state: &mut ()) -> impl View<()> {
    ///         text("Hello")
    ///     }
    /// }
    ///
    /// frust::app!(
    ///     App,
    ///     setup = {
    ///         NetworkImages::install();
    ///     }
    /// );
    /// # fn main() {}
    /// ```
    #[cfg(feature = "reqwest")]
    pub fn install() {
        match crate::ReqwestFetcher::new() {
            Ok(fetcher) => Self::install_with(Arc::new(fetcher), LoaderConfig::default()),
            Err(e) => log::error!("network-image: default fetcher unavailable ({e})"),
        }
    }

    /// Install a loader over `fetcher` with `config`. A second call keeps the
    /// first process-wide loader but still provides the new one as context
    /// under the current reactive owner.
    pub fn install_with(fetcher: Arc<dyn ImageFetcher>, config: LoaderConfig) {
        let loader = Arc::new(ImageLoader::new(fetcher, config));
        if GLOBAL.set(loader.clone()).is_err() {
            log::debug!("network-image: a loader is already installed process-wide");
        }
        provide_context(InstalledLoader(loader));
    }

    /// The loader in scope: the nearest context-provided one, else the
    /// process-wide one, else `None` (nothing installed).
    pub fn loader() -> Option<Arc<ImageLoader>> {
        use_context::<InstalledLoader>()
            .map(|installed| installed.0)
            .or_else(|| GLOBAL.get().cloned())
    }
}

/// Log the missing-install wiring bug at `warn` once, `debug` afterwards.
pub(crate) fn warn_not_installed() {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if WARNED.swap(true, Ordering::Relaxed) {
        log::debug!("network-image: no loader installed");
    } else {
        log::warn!(
            "network-image: no loader installed; call NetworkImages::install() from \
             frust::app!'s setup block or pass .loader(..) (further misses logged at debug)"
        );
    }
}
