//! The hot-patch canary's patched app: a headless one-package app shaped like
//! a `frust create` app (a lib with the template's crate types, a `main.rs`
//! bin, the anchor defined in the lib), with no window, GPU or devtools.
//!
//! [`run`] registers the anchor, then serves a line protocol on stdin/stdout
//! for the canary driver, calling the hot function [`reading`] through one
//! [`HotFn`] on every request:
//!
//! - startup prints `ready pid=<pid> anchor=<0x..> value=<n>`;
//! - `value` prints `value=<n> patches=<k>`;
//! - any other line is a [`JumpTable`] as JSON: it is applied in-process and
//!   answered `applied value=<n> hits=<h> patches=<k>`, or `refused <reason>`.
//!
//! EOF on stdin ends the loop. The driver edits this file while the app runs
//! (the [`reading`] return value, then a field added to [`Reading`]) and
//! restores it afterwards; it edits the local path dependency
//! `hotpatch_canary_shared` the same way.

use std::io::{BufRead, Write};

use frust_hotpatch::{HotFn, JumpTable};
use hotpatch_canary_shared::Offset;

/// The value the hot function returns: it crosses the patch seam by value,
/// so its layout (the path dependency's [`Offset`] inside it included) is
/// what the builder's L3 gate must protect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    pub value: u64,
    pub offset: Offset,
}

/// The hot function the canary patches: its own value plus the path
/// dependency's.
#[inline(never)]
pub fn reading() -> Reading {
    let offset = hotpatch_canary_shared::offset();
    let value = 1 + offset.value;
    Reading { value, offset }
}

/// The app-owned hot-patch anchor, as `frust::app!` emits it.
#[unsafe(no_mangle)]
pub extern "C" fn __frust_hotpatch_anchor() {}

/// The app's whole life: register the anchor, announce it, serve requests
/// until stdin closes.
pub fn run() {
    let anchor_fn: extern "C" fn() = __frust_hotpatch_anchor;
    let anchor = anchor_fn as usize;
    frust_hotpatch::set_anchor(anchor);
    let mut hot = HotFn::current(reading);
    let mut patches = 0u32;
    let mut out = std::io::stdout().lock();
    let value = hot.call(()).value;
    let pid = std::process::id();
    respond(
        &mut out,
        &format!("ready pid={pid} anchor={anchor:#x} value={value}"),
    );
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let answer = if line == "value" {
            format!("value={} patches={patches}", hot.call(()).value)
        } else {
            match apply(line) {
                Ok(()) => {
                    patches += 1;
                    let value = hot.call(()).value;
                    let hits = frust_hotpatch::seam_hits();
                    format!("applied value={value} hits={hits} patches={patches}")
                }
                Err(reason) => format!("refused {reason}"),
            }
        };
        respond(&mut out, &answer);
    }
}

/// Parses `json` as a jump table and applies it to this process.
pub fn apply(json: &str) -> Result<(), String> {
    let table: JumpTable =
        serde_json::from_str(json).map_err(|err| format!("not a jump table: {err}"))?;
    // SAFETY: the only sender is the canary driver, which built this table
    // for this very process: keys from this executable's symbol table, the
    // stub bound to the anchor address announced at startup, and its L3 gate
    // passed, so every mapped function keeps its argument and return
    // layouts. `apply_patch` itself refuses a table not anchored on
    // `__frust_hotpatch_anchor`.
    unsafe { frust_hotpatch::apply_patch(table) }.map_err(|err| err.to_string())
}

fn respond(out: &mut impl Write, line: &str) {
    // A closed stdout means the driver is gone; the next stdin read ends.
    let _ = writeln!(out, "{line}").and_then(|()| out.flush());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_malformed_table_is_refused_before_anything_loads() {
        let err = apply("{\"lib\": 3}").unwrap_err();
        assert!(err.starts_with("not a jump table"), "{err}");
    }

    #[test]
    fn the_hot_function_runs_unpatched_without_a_table() {
        assert_eq!(HotFn::current(reading).call(()), reading());
    }
}
