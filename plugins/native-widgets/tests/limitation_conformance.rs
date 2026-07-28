//! Source-scan conformance test for the two documented limitations of the
//! public `NativeComponent` seam (task m-04): **one list, kept honest in both
//! directions.**
//!
//! # Read this if you are the author who makes a limitation stop being true
//!
//! Phase 3's review found an *incomplete routing*: two true facts about this
//! plugin were asserted in some files and contradicted in others. The fix
//! wrote the same claim into nine places, which closed the defect and left the
//! mirror-image hazard: **when Phase 4 wires listener attachment, whoever does
//! it must find and update every surface, or this repo ends up documenting a
//! limitation it no longer has.** That is what this file exists to prevent.
//!
//! So, when you make one of these claims false:
//!
//! 1. Run this test. It fails once per listed surface whose canonical marker
//!    you have not yet rewritten, naming each file — that failure list *is*
//!    your worklist.
//! 2. Rewrite every one of them, then delete that limitation's entry from
//!    [`LIMITATIONS`] below (delete the whole file once both are gone —
//!    a list of zero claims proves nothing).
//! 3. Update the one surface this scan deliberately cannot see, by hand:
//!    `examples/glyph-catalog/src/pages/native_widgets.rs`'s composite-card
//!    caption restates the **FFI wall** (it does not state the display-only
//!    claim, as of m-04), and this crate's tests do not reach into
//!    `examples/**` — the boundary p3-03 established, see
//!    `tests/kotlin_conformance.rs`'s own module doc. It is named here
//!    precisely because nothing enforces it.
//!
//! Do **not** narrow the marker set or shorten the site list to make this
//! green. A list that no longer matches the tree is the failure mode this test
//! is about; the bare-`dev.frust` allowlist went vacuous earlier in this phase
//! for exactly the missing half of it
//! (`crates/frust-drive/tests/plugin_package_conformance.rs`'s own liveness
//! guard is the remedy that came out of that).
//!
//! # The idiom, and the precedents
//!
//! A plain `std::fs` source scan run as an ordinary `cargo test` — this repo
//! has no lint-plugin tooling (`docs/CODE_STANDARDS.md`), and the same shape
//! already guards three comparable invariants: this crate's own
//! `tests/kotlin_conformance.rs` (Kotlin↔Rust constant parity),
//! `crates/frust/tests/surface_mode_conformance.rs` (who may write a
//! process-global slot) and `crates/frust-drive/tests/print_free_cores.rs`
//! (which functions may print). Two properties are borrowed from the last two
//! deliberately, because a list-based scan without them is worth very little:
//! a **stale entry fails** (`print_free_cores`' "every allowlist entry must
//! actually fire") and an **unlisted hit fails**
//! (`surface_mode_conformance`'s ban direction).
//!
//! # What is actually true about the two claims (as of m-04)
//!
//! The markers below are phrased the way they are on purpose:
//!
//! - **The FFI wall** is unconditional: implementing `NativeComponent` means
//!   naming `jni::objects::JObject`/`objc2-ui-kit` types in the implementing
//!   crate, and this plugin re-exports neither FFI crate.
//! - **Display-only** is *scoped*, not absolute: no **production path**
//!   attaches a listener to a component-built view, so overriding `on_event`
//!   has no effect. It is NOT "can never fire" — `NativeRuntime::on_event`
//!   routes on the slot id alone, and Android's `nativeOnEvent` export
//!   validates only that the incoming `jlong` is non-negative
//!   (`SlotId::try_from`, task m-02), so a listener carrying a *fabricated*
//!   non-negative id that names a live component's slot is delivered like any
//!   other — a misroute rather than a route, which is exactly why
//!   `src/demo.rs` warns against fabricating one. A future edit that
//!   "simplifies" a listed surface back to the absolute is a regression this
//!   test cannot catch; the marker keeps the *claim* present, not its
//!   precision.
//!
//! # Scan scope
//!
//! `plugins/native-widgets/src/**/*.rs`, this plugin's `README.md`, and the
//! charter docs `docs/*.md` (top level only). Deliberately excluded:
//! `tests/**` (this very file quotes every marker, and would match itself),
//! `docs/learning/**` (lab curriculum, not a contract surface) and
//! `examples/**` (the p3-03 boundary above). `src/demo.rs` is scanned even
//! though it only compiles under the non-default `demo-components` feature —
//! a doc claim does not stop being wrong when its file is feature-gated out.
//!
//! # What this is NOT
//!
//! A substring scan over whitespace-normalized text, not a parser (the same
//! scope note every precedent above carries). Two consequences worth knowing:
//! a marker phrase interrupted by markup (`no **production path attaches**`)
//! is not seen, and a surface that keeps its marker while rewording the claim
//! around it still passes. [`normalized`] is what lets a marker span a
//! hard-wrapped line — `docs/DEVELOPMENT.md` genuinely wraps "an app / crate
//! cannot implement that trait" mid-phrase — and its own behaviour is pinned
//! by [`the_scan_sees_through_comment_prefixes_and_line_wrapping`].

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The workspace root, resolved from this crate's manifest dir
/// (`plugins/native-widgets`) so the scan is working-directory-independent —
/// the same resolution `tests/kotlin_conformance.rs` uses.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("plugins/native-widgets has a grandparent (the workspace root)")
        .to_path_buf()
}

/// `path` relative to the workspace root, forward-slashed, so failure messages
/// and site keys are independent of host path separators.
fn rel(path: &Path) -> String {
    path.strip_prefix(workspace_root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// One documented limitation of the public `NativeComponent` seam, plus every
/// surface that states it.
struct Limitation {
    /// Short name, used in failure messages.
    name: &'static str,
    /// The claim itself, so a failure explains what the marker stands for
    /// rather than only that a string went missing.
    claim: &'static str,
    /// The canonical phrases that count as stating the claim. A surface
    /// satisfies the limitation by carrying **any** of them (the nine surfaces
    /// word the same fact differently — a crate doc, a trait doc, a README
    /// bullet and a Module Structure table row cannot share one sentence), and
    /// every phrase must be lowercase, since [`normalized`] lowercases what it
    /// scans ([`the_scan_sees_through_comment_prefixes_and_line_wrapping`]
    /// pins that).
    markers: &'static [&'static str],
    /// Every workspace-relative file that states the claim — no more, no
    /// fewer. Both directions are checked: see
    /// [`every_listed_surface_still_states_its_limitation`] and
    /// [`no_unlisted_surface_states_a_limitation`].
    sites: &'static [&'static str],
}

/// An app crate cannot implement `NativeComponent` — the FFI wall.
const FFI_WALL_MARKERS: &[&str] = &["app crate cannot", "cannot implement `nativecomponent`"];

/// Nine surfaces state the FFI wall. `docs/DEVELOPMENT.md` is one of them
/// because the `demo-components` feature only exists for this reason (the demo
/// composite ships inside the plugin rather than in an example app).
const FFI_WALL_SITES: &[&str] = &[
    "docs/ARCHITECTURE.md",
    "docs/DEVELOPMENT.md",
    "plugins/native-widgets/README.md",
    "plugins/native-widgets/src/api/mod.rs",
    "plugins/native-widgets/src/api/mount.rs",
    "plugins/native-widgets/src/component.rs",
    "plugins/native-widgets/src/demo.rs",
    "plugins/native-widgets/src/lib.rs",
    "plugins/native-widgets/src/runtime.rs",
];

/// A component is display-only — the scoped "no production path attaches"
/// claim (see the module doc for why the scope is load-bearing).
const DISPLAY_ONLY_MARKERS: &[&str] = &[
    "no production path attaches",
    "can receive events in this build",
];

/// Seven surfaces state the display-only claim. `docs/DEVELOPMENT.md` is
/// absent on purpose — it documents the feature gate, not the event gap — and
/// `src/api/mod.rs` likewise states only the FFI wall.
const DISPLAY_ONLY_SITES: &[&str] = &[
    "docs/ARCHITECTURE.md",
    "plugins/native-widgets/README.md",
    "plugins/native-widgets/src/api/mount.rs",
    "plugins/native-widgets/src/component.rs",
    "plugins/native-widgets/src/demo.rs",
    "plugins/native-widgets/src/lib.rs",
    "plugins/native-widgets/src/runtime.rs",
];

/// The two limitations this scan pins. **Delete an entry when its claim stops
/// being true** — see the module doc's numbered instruction.
const LIMITATIONS: &[Limitation] = &[
    Limitation {
        name: "app-crate FFI wall",
        claim: "an app crate cannot implement `NativeComponent` (it needs raw \
                `jni`/`objc2-ui-kit` deps this plugin does not re-export), so the practical \
                audience is plugin authors",
        markers: FFI_WALL_MARKERS,
        sites: FFI_WALL_SITES,
    },
    Limitation {
        name: "display-only component",
        claim: "no production path attaches a listener to a component-built view, so \
                overriding `NativeComponent::on_event` has no effect in this build (a deferred \
                Phase 4 gap)",
        markers: DISPLAY_ONLY_MARKERS,
        sites: DISPLAY_ONLY_SITES,
    },
];

/// Recurse `dir` collecting `.rs` files. Panics loudly on an unreadable
/// directory (`print_free_cores.rs`'s own `walk` does the same): a swallowed
/// error here would return an empty `Vec` and make every assertion below pass
/// vacuously, which is the defect class this whole file is about.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display())) {
        let path = entry
            .unwrap_or_else(|e| panic!("dir entry under {}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Every file this scan judges — see the module doc's *Scan scope* for what is
/// in, what is out, and why.
fn scanned_files() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut out = Vec::new();

    rust_files(&root.join("plugins/native-widgets/src"), &mut out);
    out.push(root.join("plugins/native-widgets/README.md"));

    // `docs/*.md` only, not `docs/**`: the charter docs are contract surfaces,
    // `docs/learning/**` is curriculum.
    let docs = root.join("docs");
    for entry in fs::read_dir(&docs).unwrap_or_else(|e| panic!("reading {}: {e}", docs.display())) {
        let path = entry
            .unwrap_or_else(|e| panic!("dir entry under {}: {e}", docs.display()))
            .path();
        if path.is_file() && path.extension().is_some_and(|ext| ext == "md") {
            out.push(path);
        }
    }

    out.sort();
    out
}

/// `contents` as one lowercase line: leading comment/blockquote markers
/// stripped per line, then every whitespace run collapsed to a single space.
///
/// This is what lets a marker phrase span a hard-wrapped doc comment or
/// markdown paragraph — the wrapping is invisible to the match, so the same
/// canonical phrase pins a `//!` crate doc, a `///` item doc, a `>` README
/// blockquote and a plain markdown table row alike.
fn normalized(contents: &str) -> String {
    let mut joined = String::with_capacity(contents.len());
    for line in contents.lines() {
        let mut text = line.trim();
        // `//!`/`///` before the bare `//` both start with, or the strip would
        // leave a stray `!`/`/` glued to the first word.
        for prefix in ["//!", "///", "//", ">"] {
            if let Some(rest) = text.strip_prefix(prefix) {
                text = rest.trim();
                break;
            }
        }
        joined.push_str(text);
        joined.push(' ');
    }
    joined
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
        .to_lowercase()
}

/// Whether `normalized_contents` states `limitation` — any one marker is
/// enough (see [`Limitation::markers`]).
fn states(normalized_contents: &str, limitation: &Limitation) -> bool {
    limitation
        .markers
        .iter()
        .any(|marker| normalized_contents.contains(marker))
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// Forward direction: every listed surface must still exist, still be inside
/// the scan scope, and still carry a canonical marker.
///
/// The three failures are reported distinctly because they mean different
/// things: a vanished file is a stale entry, an out-of-scope file is an entry
/// the reverse check can never police, and a marker-less file is either a
/// reworded surface or — the interesting case — a claim that stopped being
/// true in one place and nowhere else.
#[test]
fn every_listed_surface_still_states_its_limitation() {
    let root = workspace_root();
    let in_scope: BTreeSet<String> = scanned_files().iter().map(|p| rel(p)).collect();
    let mut failures = Vec::new();

    for limitation in LIMITATIONS {
        for site in limitation.sites {
            let path = root.join(site);
            if !path.is_file() {
                failures.push(format!(
                    "  [{}] {site}: STALE ENTRY — that file does not exist. Either it moved (fix \
                     the path) or the surface is gone (drop the entry)",
                    limitation.name
                ));
                continue;
            }
            if !in_scope.contains(*site) {
                failures.push(format!(
                    "  [{}] {site}: OUT OF SCAN SCOPE — this file exists but \
                     `scanned_files()` never visits it, so the unlisted-hit check could never \
                     police it. Widen the scan deliberately or drop the entry (module doc's \
                     *Scan scope*)",
                    limitation.name
                ));
                continue;
            }
            if !states(&normalized(&read(&path)), limitation) {
                failures.push(format!(
                    "  [{}] {site}: THE CLAIM IS GONE — none of the canonical markers {:?} \
                     appears in it any more",
                    limitation.name, limitation.markers
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} listed surface(s) no longer state their limitation as expected.\n\n{}\n\nIf you are \
         mid-way through making a claim FALSE (Phase 4 wiring listener attachment, say), this \
         list is your worklist: fix every surface for that limitation, THEN delete its entry \
         from LIMITATIONS in tests/limitation_conformance.rs — never the other way round, and \
         never a partial pass, which is precisely the incomplete routing this test exists to \
         prevent. The two claims, for reference:\n{}",
        failures.len(),
        failures.join("\n"),
        LIMITATIONS
            .iter()
            .map(|l| format!("  [{}] {}", l.name, l.claim))
            .collect::<Vec<String>>()
            .join("\n"),
    );
}

/// Reverse direction: a surface stating a limitation must be on that
/// limitation's list.
///
/// This is the half whose absence let the bare-`dev.frust` allowlist go
/// vacuous (module doc). Without it, a tenth surface could restate either
/// claim and the Phase 4 author would have no way to discover it.
#[test]
fn no_unlisted_surface_states_a_limitation() {
    let files = scanned_files();
    assert!(
        !files.is_empty(),
        "the scan visited zero files — either the walk started from the wrong root or the \
         plugin's `src`/`README.md` moved. Every check in this file is satisfied vacuously by an \
         empty scan, so this guard runs first"
    );

    let normalized_files: Vec<(String, String)> = files
        .iter()
        .map(|path| (rel(path), normalized(&read(path))))
        .collect();

    let mut failures = Vec::new();
    for limitation in LIMITATIONS {
        let stating: Vec<&String> = normalized_files
            .iter()
            .filter(|(_, contents)| states(contents, limitation))
            .map(|(relp, _)| relp)
            .collect();

        // Liveness for the marker set itself: a marker nothing matches (a typo,
        // an uppercase phrase, a canonical wording that moved on) would make
        // this whole check pass while measuring nothing.
        assert!(
            !stating.is_empty(),
            "no scanned file states the '{}' limitation at all — its markers {:?} match nothing, \
             so this check is measuring nothing. Fix the markers (or, if the claim really is gone \
             everywhere, delete its LIMITATIONS entry)",
            limitation.name,
            limitation.markers,
        );

        for relp in stating {
            if !limitation.sites.contains(&relp.as_str()) {
                failures.push(format!(
                    "  [{}] {relp}: states the limitation but is not on its site list",
                    limitation.name
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} surface(s) state a limitation without being listed.\n\n{}\n\nAdd each to the matching \
         `*_SITES` list in tests/limitation_conformance.rs — a restatement nothing tracks is one \
         the next author will miss when the claim changes (this is the same list, seen from the \
         other side).",
        failures.len(),
        failures.join("\n"),
    );
}

/// Guards the scanner itself: [`normalized`] is the single point of failure for
/// every check above (a normalizer that quietly stopped stripping `//!`, or
/// stopped joining wrapped lines, would let marker-less surfaces pass), and a
/// non-lowercase marker could never match its output at all.
#[test]
fn the_scan_sees_through_comment_prefixes_and_line_wrapping() {
    // The real shape in `docs/DEVELOPMENT.md`: the canonical phrase is split
    // across a hard wrap, which a naive per-line `contains` would never see.
    let wrapped_markdown = "shipped inside the plugin rather than in an example because an app\n\
                            crate cannot implement that trait without raw deps.\n";
    assert!(normalized(wrapped_markdown).contains("app crate cannot"));

    // The same, wrapped inside a `//!` crate doc, and inside a `>` blockquote
    // with markdown emphasis around (not inside) the phrase.
    let wrapped_doc_comment = "//! composite). And **a component is\n\
                               //! display-only**: no production path attaches a listener\n";
    assert!(normalized(wrapped_doc_comment).contains("no production path attaches"));
    assert!(normalized("> **An app crate cannot** implement it\n").contains("app crate cannot"));

    // Prefix stripping and whitespace collapsing, exactly.
    assert_eq!(normalized("///   a  B\n//  c\n"), "a b c");
    assert_eq!(normalized("//! x\n\n//! y\n"), "x y");

    for limitation in LIMITATIONS {
        assert!(
            !limitation.markers.is_empty(),
            "limitation '{}' has no markers, so nothing pins it",
            limitation.name
        );
        assert!(
            !limitation.sites.is_empty(),
            "limitation '{}' has no sites — delete the whole entry instead of leaving an empty \
             list that passes vacuously",
            limitation.name
        );
        for marker in limitation.markers {
            assert_eq!(
                *marker,
                marker.to_lowercase(),
                "limitation '{}' has a non-lowercase marker, which `normalized`'s lowercased \
                 output can never contain",
                limitation.name
            );
        }
    }
}
