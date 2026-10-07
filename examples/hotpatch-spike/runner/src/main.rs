//! Desktop entry point of the hot-patch spike.
//!
//! A spike-only deviation from the template's "never hand-edit `main.rs`" rule: in a debug desktop
//! build, `main` first connects to dx's devserver (`connect_devserver`: `dioxus_devtools::connect_at`
//! behind a loopback guard, a no-op when the binary was not launched by `dx serve`) and applies each
//! patch dx builds for the `app` package through frust-hotpatch. The template's `wasm32` arm is
//! absent on purpose: it exists only because a one-package app's `[lib]` and `[[bin]]` collide on
//! one `.wasm` output name, and here the lib is another package.

#[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
fn main() {
    // `dioxus-devtools` is a desktop-only dependency; iOS shares this arm only to reach the
    // `__frust_main` stub `frust::app!` emits there. The devserver connection (and the `apply_patch`
    // it performs, which loads and jumps into code the devserver sends) is debug-only and desktop-only:
    // `connect_devserver` and `apply_hot_patch` carry one identical cfg, so no target sees one without
    // the other. frust-hotpatch itself consults its jump table only under `debug_assertions`.
    #[cfg(all(
        debug_assertions,
        not(any(target_os = "android", target_os = "ios", target_arch = "wasm32"))
    ))]
    connect_devserver();
    hotpatch_spike_app::__frust_main();
}

/// Connects to the devserver named by `DIOXUS_DEVSERVER_IP` / `DIOXUS_DEVSERVER_PORT` (set by `dx
/// serve`; unset means not launched by dx, so nothing happens) and refuses any non-loopback address.
/// The devserver speaks plaintext `ws://` with no authentication, so connecting to a remote host
/// would let that host choose code this process loads and runs.
///
/// Both refusals print with `eprintln!`, not `log`: this runs before `__frust_main`, so the desktop
/// shell has not installed its logger yet and a `log` record would be dropped silently.
#[cfg(all(
    debug_assertions,
    not(any(target_os = "android", target_os = "ios", target_arch = "wasm32"))
))]
fn connect_devserver() {
    let (Ok(ip), Ok(port)) = (
        std::env::var("DIOXUS_DEVSERVER_IP"),
        std::env::var("DIOXUS_DEVSERVER_PORT"),
    ) else {
        return;
    };
    let addr = match format!("{ip}:{port}").parse::<std::net::SocketAddr>() {
        Ok(addr) => addr,
        Err(err) => {
            eprintln!("frust-hotpatch: unparsable devserver address {ip}:{port}: {err}");
            return;
        }
    };
    if !addr.ip().is_loopback() {
        eprintln!("frust-hotpatch: refusing non-loopback devserver {addr}; not connecting");
        return;
    }
    dioxus_devtools::connect_at(format!("ws://{addr}/_dioxus"), apply_hot_patch);
}

/// Applies a devserver hot-patch addressed to this process through frust-hotpatch, the same filter
/// `dioxus_devtools::connect_subsecond` applies. dx sends a `subsecond_types::JumpTable`; the
/// frust-hotpatch table has the same serde shape, so a JSON round-trip converts it.
#[cfg(all(
    debug_assertions,
    not(any(target_os = "android", target_os = "ios", target_arch = "wasm32"))
))]
fn apply_hot_patch(msg: dioxus_devtools::DevserverMsg) {
    let dioxus_devtools::DevserverMsg::HotReload(msg) = msg else {
        return;
    };
    let Some(table) = msg.jump_table else {
        return;
    };
    if msg.for_pid != Some(std::process::id()) {
        return;
    }
    let table: frust_hotpatch::JumpTable =
        match serde_json::to_value(table).and_then(serde_json::from_value) {
            Ok(table) => table,
            Err(err) => {
                log::error!("frust-hotpatch: jump table conversion failed: {err}");
                return;
            }
        };
    // SAFETY: this is only as safe as the devserver it trusts. `apply_patch` loads `table.lib`, a
    // path the server chose, and jumps into it. The process obeys whatever
    // DIOXUS_DEVSERVER_IP/PORT point it at over plaintext ws:// with no authentication; the pid
    // filter above is a routing check, not authentication. `connect_devserver` therefore refuses
    // non-loopback addresses, so the trusted peer is a local process (the `dx serve` that launched
    // us). A malicious local process on that port could still deliver code: debug-only spike.
    if let Err(err) = unsafe { frust_hotpatch::apply_patch(table) } {
        log::error!("frust-hotpatch: apply_patch failed: {err}");
    }
}

#[cfg(target_os = "android")]
fn main() {}
