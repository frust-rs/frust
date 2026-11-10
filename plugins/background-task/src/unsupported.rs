//! Every target without a backend: `frust::BackgroundTask` stays the core no-op.

pub(crate) fn install() -> bool {
    log::debug!("frust-background-task: no backend on this platform");
    false
}
