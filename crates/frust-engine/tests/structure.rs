//! Structural guards G1, G2, E17, and BF: design-rule tripwires over the engine
//! crate's shape rather than its runtime behavior. Host-only — no GPU device
//! or adapter is created anywhere in this file.
//!
//! - **G1**: `frust-gpu`'s WGSL directory lint, run over
//!   `crates/frust-engine/shaders/` — must now find real files (p3-05 ported
//!   the strip/clear/copy programs there) and report zero design-rule
//!   violations.
//! - **G2**: the engine's downlevel GPU-limits profile stays within the
//!   WebGL2/GLES3.0 ceiling (`wgpu::Limits::check_limits` against
//!   `Limits::downlevel_webgl2_defaults()`), the only `wgpu::Features` bit
//!   the substrate's device-request policy may ever ask for is
//!   `TIMESTAMP_QUERY` (and only under a `perf-trace` build), and
//!   `frust-gpu`'s `HeadlessTarget` never requests `STORAGE_BINDING` — it
//!   renders through ordinary render passes, never compute
//!   (`crates/frust-gpu/src/headless.rs`'s own module doc states the same
//!   rule; this guard pins it against silent drift).
//! - **E17**: every rendering path under `src/{cache,compile,effects,filters,gpu,
//!   schedule,renderer,text}.rs` returns an error rather than panicking — no
//!   bare `unwrap()`/`expect(`/`panic!(` outside a `#[cfg(test)]` module.
//!   Currently scanned: `cache/`, `compile/`, `effects/`, `filters/`, `gpu/`,
//!   and `text/`. Conditionally scanned when present: `renderer.rs` and
//!   `schedule/` — these stay unscanned until merge, then the guard becomes
//!   load-bearing with no edit needed here.
//! - **BF**: every curve-capable fill, stroke and clip reaches `vello_common`
//!   through `compile/cull.rs`; the scan and its allow-lists are in the BF
//!   section below.

use std::path::{Path, PathBuf};

/// `crates/frust-engine`, resolved from this crate's own manifest dir.
fn engine_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `crates/frust-gpu`, a sibling of [`engine_root`] under `crates/`.
fn gpu_crate_root() -> PathBuf {
    engine_root()
        .parent()
        .expect("crates/frust-engine has a parent: crates/")
        .join("frust-gpu")
}

// ---------------------------------------------------------------------
// G1: the engine's shipped shaders pass frust-gpu's downlevel lint
// ---------------------------------------------------------------------

#[test]
fn g1_engine_shader_directory_has_real_files_and_passes_the_downlevel_lint() {
    let shaders_dir = engine_root().join("shaders");
    assert!(
        shaders_dir.is_dir(),
        "{} must exist and hold the ported strip/clear/copy shaders",
        shaders_dir.display()
    );

    let wgsl_files: Vec<PathBuf> = std::fs::read_dir(&shaders_dir)
        .unwrap_or_else(|err| panic!("reading {}: {err}", shaders_dir.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("wgsl"))
        .collect();
    assert!(
        !wgsl_files.is_empty(),
        "{} has no .wgsl files — an empty directory would let `lint_wgsl_dir` pass vacuously, \
         hiding the fact that the shader port never landed",
        shaders_dir.display()
    );

    let violations = frust_gpu::lint_wgsl_dir(&shaders_dir)
        .unwrap_or_else(|err| panic!("scanning {}: {err}", shaders_dir.display()));
    assert!(
        violations.is_empty(),
        "the engine's shipped shaders violate frust-gpu's downlevel design rules: {violations:?}"
    );
}

// ---------------------------------------------------------------------
// G2: downlevel GPU-limits/features policy, and HeadlessTarget usage
// ---------------------------------------------------------------------

/// The only `wgpu::Features` bit the engine's GPU substrate device-request
/// policy (`frust_gpu::context`'s `required_features`) is ever allowed to
/// ask for: empty by default, `TIMESTAMP_QUERY` only under a `perf-trace`
/// build and only when the adapter actually offers it.
const ALLOWED_ENGINE_FEATURES: wgpu::Features = wgpu::Features::TIMESTAMP_QUERY;

#[test]
fn g2_webgl2_downlevel_limits_satisfy_the_webgl2_ceiling() {
    let caps = frust_gpu::TierCaps::fake(frust_gpu::DownlevelProfile::WebGl2);
    let webgl2_defaults = wgpu::Limits::downlevel_webgl2_defaults();

    // The subset of `wgpu::Limits` fields `TierCaps` actually tracks, laid
    // over the WebGL2 defaults for every field it does not — the same shape
    // `frust_gpu::context::create_device` requests under
    // `DownlevelProfile::WebGl2` (`downlevel_webgl2_defaults().using_resolution(..)`).
    let declared = wgpu::Limits {
        max_texture_dimension_2d: caps.max_texture_dimension_2d,
        max_texture_array_layers: caps.max_texture_array_layers,
        max_bind_groups: caps.max_bind_groups,
        max_uniform_buffer_binding_size: u64::from(caps.max_uniform_buffer_binding_size),
        min_uniform_buffer_offset_alignment: caps.min_uniform_buffer_offset_alignment,
        max_vertex_attributes: caps.max_vertex_attributes,
        ..webgl2_defaults.clone()
    };

    assert!(
        declared.check_limits(&webgl2_defaults),
        "the engine's downlevel-profile GPU limits exceed the WebGL2 ceiling: {declared:?}"
    );
    assert!(
        frust_gpu::check_limits_against_webgl2(&declared).is_empty(),
        "frust-gpu's own WebGL2 lint disagrees with wgpu's `check_limits` for {declared:?}"
    );
}

#[test]
fn g2_requested_features_never_exceed_the_perf_trace_timestamp_query_allowance() {
    for profile in [
        frust_gpu::DownlevelProfile::Full,
        frust_gpu::DownlevelProfile::WebGl2,
    ] {
        let caps = frust_gpu::TierCaps::fake(profile);
        // Mirrors `frust_gpu::context::required_features`'s documented
        // policy exactly: empty unless a perf-trace build's adapter actually
        // offers the feature.
        let would_request_under_perf_trace = if caps.has_timestamp_query {
            wgpu::Features::TIMESTAMP_QUERY
        } else {
            wgpu::Features::empty()
        };
        assert!(
            ALLOWED_ENGINE_FEATURES.contains(would_request_under_perf_trace),
            "{profile:?} caps would let a perf-trace build request \
             {would_request_under_perf_trace:?}, outside the engine's allowed feature set \
             {ALLOWED_ENGINE_FEATURES:?}"
        );
    }
}

#[test]
fn g2_headless_target_usage_excludes_storage_binding() {
    let path = gpu_crate_root().join("src/headless.rs");
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));

    let usage_lines: Vec<&str> = contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//"))
        .filter(|line| line.contains("usage") && line.contains("TextureUsages"))
        .collect();
    assert!(
        !usage_lines.is_empty(),
        "expected to find HeadlessTarget's texture-usage declaration (a `usage: \
         wgpu::TextureUsages::...` line) in {}",
        path.display()
    );

    for line in usage_lines {
        assert!(
            !line.contains("STORAGE_BINDING"),
            "HeadlessTarget must never request STORAGE_BINDING — it renders through ordinary \
             render passes, never compute (see {}'s module doc): {line}",
            path.display()
        );
    }
}

// ---------------------------------------------------------------------
// E17: no unwrap()/expect(/panic-family/assert-family macro outside #[cfg(test)] under
// src/{cache,compile,gpu,schedule,renderer}.rs
// ---------------------------------------------------------------------

/// Substrings that fail this guard wherever they appear in production code.
/// Deliberately literal (not a parser): `unwrap()` (no arguments — an
/// `unwrap_or`/`unwrap_or_else` never matches), `.expect(`, `panic!(`,
/// `unreachable!(`, `todo!(`, `unimplemented!(`, and the release-active
/// assertions `assert!(`, `assert_eq!(`, `assert_ne!(`.
///
/// `debug_assert!(` and its `_eq`/`_ne` forms stay allowed on purpose: they
/// compile out of release builds, so they cannot panic a shipped render path
/// (see [`needle_hits`] for how the `debug_` prefix is excluded). Likewise a
/// compile-time assertion (`const _: () = assert!(..)`, or `const { assert!(..) }`)
/// fails the build, never a frame, so it is allowed.
///
/// Known gap: there is no `[` needle. Slice and array indexing can panic, but
/// has no cheap textual rule that does not drown in false positives (every
/// type, attribute and array literal contains a bracket); indexing on render
/// paths is left to review and to clippy's `indexing_slicing` where enabled.
const BANNED_NEEDLES: &[&str] = &[
    "unwrap()",
    ".expect(",
    "panic!(",
    "unreachable!(",
    "todo!(",
    "unimplemented!(",
    "assert!(",
    "assert_eq!(",
    "assert_ne!(",
];

/// Whether `line` contains `needle` as a banned use. An `assert*!(` needle only
/// counts when the character before it is not part of an identifier, so
/// `debug_assert!(` (and any other `*_assert!(` macro) is not a hit, and a line
/// that opens a `const` item or `const { .. }` block is evaluated at compile
/// time, so its assertions never run on a render path.
fn needle_hits(line: &str, needle: &str) -> bool {
    let guard_prefix = needle.starts_with("assert");
    if guard_prefix {
        let trimmed = line.trim_start();
        if trimmed.starts_with("const _:") || trimmed.contains("const {") {
            return false;
        }
    }
    line.match_indices(needle).any(|(at, _)| {
        !guard_prefix
            || !line[..at]
                .chars()
                .next_back()
                .is_some_and(|prev| prev.is_alphanumeric() || prev == '_')
    })
}

/// Every `.rs` file directly under `dir` and every subdirectory, or an empty
/// list when `dir` does not exist — the same "not-yet-added is a clean scan"
/// convention `frust_gpu::lint::lint_wgsl_dir` documents for a shader
/// directory, applied here to a source directory that may not exist yet on
/// this base (`schedule/`, `renderer.rs`'s sibling module).
fn rust_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry
            .unwrap_or_else(|err| panic!("dir entry in {}: {err}", dir.display()))
            .path();
        if path.is_dir() {
            rust_files_recursive(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// The E17 grep scope:
/// `src/{cache,compile,effects,filters,gpu,text,schedule,renderer}.rs` —
/// `cache/`, `compile/`, `effects/`, `filters/`, `gpu/`, and `text/` are
/// trees that exist on this base; `renderer.rs` and `schedule/` are grepped
/// only when present.
///
/// `cache/`, `effects/`, `filters/` and `text/` all execute on the frame path:
/// a gradient ramp is built, keyed and evicted during frame rendering (cache);
/// shader effects are compiled and rendered during frame rendering (effects);
/// a blurred layer's kernel is prepared and its pass sequence planned during
/// frame rendering (filters); and glyphs are compiled and shaped during frame
/// rendering (text). Every directory the compiler and renderer call into on
/// that path is in scope.
fn e17_scanned_files() -> Vec<PathBuf> {
    let src = engine_root().join("src");
    let mut files = Vec::new();

    for name in ["cache", "compile", "effects", "filters", "gpu", "text"] {
        rust_files_recursive(&src.join(name), &mut files);
    }

    let renderer_rs = src.join("renderer.rs");
    if renderer_rs.is_file() {
        files.push(renderer_rs);
    }
    rust_files_recursive(&src.join("schedule"), &mut files);

    files.sort();
    files
}

/// `contents`' `(0-based line index, raw text)` pairs up to (excluding) the
/// file's `#[cfg(test)]`/`mod tests` marker — every engine module in the E17
/// scope keeps its test module last-in-file, the same convention
/// `frust-drive/tests/print_free_cores.rs` relies on for the identical scan
/// shape. Comment-only lines are dropped so a prose mention of a banned
/// needle (e.g. this file's own doc comments) is never mistaken for code.
fn production_lines(contents: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    for (i, raw) in contents.lines().enumerate() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("mod tests") {
            break;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        out.push((i, raw));
    }
    out
}

#[test]
fn e17_engine_render_paths_never_unwrap_expect_or_panic_outside_tests() {
    let mut failures = Vec::new();

    for path in e17_scanned_files() {
        let contents = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
        for (i, line) in production_lines(&contents) {
            for needle in BANNED_NEEDLES {
                if needle_hits(line, needle) {
                    failures.push(format!(
                        "{}:{}: `{needle}` outside #[cfg(test)] — E17 requires every rendering \
                         path to return an error rather than panic: {}",
                        path.display(),
                        i + 1,
                        line.trim()
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "E17 violated ({} hit(s)):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn e17_widened_needles_flag_release_panics_and_spare_debug_and_const_asserts() {
    for hit in [
        "    assert!(ok);",
        "    assert_eq!(a, b);",
        "    assert_ne!(a, b);",
        "    unreachable!(\"x\")",
        "    todo!()",
        "    unimplemented!()",
        "    x = (assert!(ok), 1);",
    ] {
        assert!(
            BANNED_NEEDLES.iter().any(|n| needle_hits(hit, n)),
            "{hit} must be flagged"
        );
    }
    for spared in [
        "    debug_assert!(ok);",
        "    debug_assert_eq!(a, b);",
        "    debug_assert_ne!(a, b);",
        "const _: () = assert!(N > 0);",
        "    const { assert!(N > 0) };",
    ] {
        assert!(
            !BANNED_NEEDLES.iter().any(|n| needle_hits(spared, n)),
            "{spared} must be allowed"
        );
    }
}

#[test]
fn e17_scan_currently_covers_at_least_cache_compile_effects_filters_gpu_and_text() {
    // Documents the scan's current floor so a future refactor that
    // accidentally empties `e17_scanned_files()` (e.g. a typo'd directory
    // name) fails loudly here rather than the main guard above silently
    // passing over nothing. Named per directory rather than merely counted:
    // a scan that quietly stopped covering one of the six would otherwise
    // still satisfy a non-empty assertion.
    let files = e17_scanned_files();
    for name in ["cache", "compile", "effects", "filters", "gpu", "text"] {
        assert!(
            files
                .iter()
                .any(|path| path.components().any(|c| c.as_os_str() == name)),
            "the E17 scan covers no .rs file under src/{name}/: {files:?}"
        );
    }
}

// ---------------------------------------------------------------------
// BF: curve-capable paths reach vello_common only through compile/cull.rs
// ---------------------------------------------------------------------

// The bounded-flatten contract: every curve-capable fill, stroke and clip
// reaches `vello_common`'s strip generator through `compile/cull.rs`
// (`generate_fill`, `generate_stroke` and `ViewportSplit`), which culls the
// path to the viewport before it is flattened. Two shapes may bypass those
// wrappers, and the scan accepts only these: a `kurbo::Rect` filled as a path,
// because its four straight edges flatten at a fixed cost however large the
// rectangle is; and a `ViewportSplit` pushed as a clip mask, because that
// pushed path is the pre-pass itself. Anything else must be routed through
// `cull.rs`, or the allow-lists below extended deliberately.

/// The one file allowed to call the raw strip generator and its clip context.
/// Excluded from the scan by name.
const CULL_FILE: &str = "compile/cull.rs";

/// One accepted raw `generate_filled_path(` call shape: its path argument is
/// `<receiver>.path_elements(..)`, and `receiver` is a `kurbo::Rect` at each of
/// the `count` sites in `file`. A new site on the same receiver changes the
/// count and fails the guard until its author reviews it.
struct RectFillSite {
    file: &'static str,
    receiver: &'static str,
    count: usize,
}

const ALLOWED_RECT_FILL_SITES: &[RectFillSite] = &[
    // `Geometry::RectPath`, the rectangle geometry arm.
    RectFillSite {
        file: "compile/parallel.rs",
        receiver: "rect",
        count: 1,
    },
    // The clear hole punch: `StagedPunch::device`.
    RectFillSite {
        file: "compile/mod.rs",
        receiver: "punch.device",
        count: 1,
    },
    // A glyph's slot `area`.
    RectFillSite {
        file: "text/backend.rs",
        receiver: "area",
        count: 1,
    },
    // A COLR `LayerShape::Rect` (`let rect = *rect`) and an atlas glyph's
    // `fill_rect(&Rect)`.
    RectFillSite {
        file: "text/backend.rs",
        receiver: "rect",
        count: 2,
    },
];

/// `(file, receiver)` pairs that name a vello `ClipContext`. Every `push_clip(`
/// on one must take a `ViewportSplit::new(..)` path. The engine has exactly
/// one: the clip-mask stack.
const VELLO_CLIP_CONTEXTS: &[(&str, &str)] = &[("compile/clip.rs", "self.masks")];

/// `(file, receiver)` pairs whose `push_clip(` is frust-owned, not vello's: the
/// clip-group stack, the COLR bracket bookkeeping, and the frust scene builder.
const FRUST_OWNED_PUSH_CLIP: &[(&str, &str)] = &[
    ("compile/mod.rs", "self.groups"),
    ("text/backend.rs", "self.color"),
    ("renderer.rs", "b"),
];

/// What the bounded-flatten scan found across the engine sources.
#[derive(Default)]
struct BoundedFlatten {
    /// Every file the scanner read, as `src/`-relative paths.
    files_seen: Vec<String>,
    /// Every accepted `generate_filled_path` site, as `(file, receiver)`.
    rect_fills: Vec<(String, String)>,
    /// Human-readable `file:line: reason` violations.
    violations: Vec<String>,
}

/// `src/`-relative path with `/` separators, so allow-list keys are the same
/// on every host.
fn src_relative(path: &Path) -> String {
    path.strip_prefix(engine_root().join("src"))
        .unwrap_or_else(|_| panic!("{} is not under the engine's src/", path.display()))
        .to_string_lossy()
        .replace('\\', "/")
}

/// Every `.rs` file under `src/` except [`CULL_FILE`].
fn bounded_flatten_scanned_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    rust_files_recursive(&engine_root().join("src"), &mut files);
    files.retain(|path| src_relative(path) != CULL_FILE);
    files.sort();
    files
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Byte offsets of every call `name(` in `text`: the name must not be part of
/// a longer identifier, and a `fn name(` definition is not a call.
fn call_offsets(text: &str, name: &str) -> Vec<usize> {
    let needle = format!("{name}(");
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find(&needle) {
        let at = from + found;
        from = at + needle.len();
        if text[..at].chars().next_back().is_some_and(is_ident_char) {
            continue;
        }
        let is_definition = text[..at]
            .trim_end()
            .strip_suffix("fn")
            .is_some_and(|rest| !rest.chars().next_back().is_some_and(is_ident_char));
        if !is_definition {
            out.push(at);
        }
    }
    out
}

/// The argument text after the `(` at `open`, up to the first top-level `,`
/// or closing bracket, trimmed.
fn first_arg(text: &str, open: usize) -> &str {
    let body = &text[open + 1..];
    let mut depth = 0usize;
    for (i, c) in body.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => return body[..i].trim(),
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => return body[..i].trim(),
            _ => {}
        }
    }
    body.trim()
}

/// Whether the `(` at byte `open` of `s` closes on the last byte of `s`.
fn closes_at_end(s: &str, open: usize) -> bool {
    let mut depth = 0usize;
    for (i, c) in s[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return open + i + 1 == s.len();
                }
            }
            _ => {}
        }
    }
    false
}

/// The receiver `r` of a whole-argument `r.path_elements(..)` expression, or
/// `None` when the argument is anything else (including a chained call after
/// the `path_elements(..)`).
fn path_elements_receiver(arg: &str) -> Option<&str> {
    const CALL: &str = ".path_elements(";
    let idx = arg.find(CALL)?;
    let receiver = &arg[..idx];
    let well_formed = !receiver.is_empty()
        && receiver
            .split('.')
            .all(|seg| !seg.is_empty() && seg.chars().all(is_ident_char));
    (well_formed && closes_at_end(arg, idx + CALL.len() - 1)).then_some(receiver)
}

/// The receiver of the method call whose `.name(` starts at byte `at`, read
/// backwards through whitespace so a chain split across lines still resolves.
/// `None` when the call has no dotted-identifier receiver (a free function, a
/// `Type::name(` path, or a call on a bracketed expression).
fn method_receiver(text: &str, at: usize) -> Option<&str> {
    let head = text[..at].trim_end().strip_suffix('.')?.trim_end();
    let start = head
        .char_indices()
        .rev()
        .find(|&(_, c)| !(is_ident_char(c) || c == '.'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let receiver = &head[start..];
    let well_formed = !receiver.is_empty()
        && receiver
            .split('.')
            .all(|seg| !seg.is_empty() && seg.chars().all(is_ident_char));
    well_formed.then_some(receiver)
}

fn is_viewport_split(arg: &str) -> bool {
    arg.starts_with("ViewportSplit::new(") && closes_at_end(arg, "ViewportSplit::new".len())
}

fn listed(list: &[(&str, &str)], file: &str, receiver: &str) -> bool {
    list.iter().any(|&(f, r)| f == file && r == receiver)
}

fn listed_rect_fill(file: &str, receiver: &str) -> bool {
    ALLOWED_RECT_FILL_SITES
        .iter()
        .any(|site| site.file == file && site.receiver == receiver)
}

/// Scan one file's production text (test modules and comment-only lines
/// excluded, as for E17) and record its bounded-flatten sites into `scan`.
/// `rel` is the file's `src/`-relative path; [`CULL_FILE`] is never passed in.
fn scan_bounded_flatten(rel: &str, contents: &str, scan: &mut BoundedFlatten) {
    scan.files_seen.push(rel.to_owned());

    let lines = production_lines(contents);
    let mut text = String::new();
    let mut starts = Vec::with_capacity(lines.len());
    for (_, line) in &lines {
        starts.push(text.len());
        text.push_str(line);
        text.push('\n');
    }
    let line_of = |at: usize| {
        let idx = starts
            .partition_point(|&start| start <= at)
            .saturating_sub(1);
        lines.get(idx).map_or(0, |(i, _)| i + 1)
    };

    // (a) No stroke reaches the raw generator outside cull.rs.
    for at in call_offsets(&text, "generate_stroked_path") {
        scan.violations.push(format!(
            "{rel}:{}: generate_stroked_path outside compile/cull.rs — a stroke must go \
             through cull::generate_stroke",
            line_of(at)
        ));
    }

    // (b) A raw fill is accepted only as `<rect>.path_elements(..)` on a
    // receiver listed in ALLOWED_RECT_FILL_SITES.
    let fill_name = "generate_filled_path";
    for at in call_offsets(&text, fill_name) {
        let line = line_of(at);
        let arg = first_arg(&text, at + fill_name.len());
        match path_elements_receiver(arg) {
            Some(receiver) if listed_rect_fill(rel, receiver) => {
                scan.rect_fills.push((rel.to_owned(), receiver.to_owned()));
            }
            Some(receiver) => scan.violations.push(format!(
                "{rel}:{line}: generate_filled_path on `{receiver}.path_elements(..)`, but \
                 `{receiver}` is not a Rect in ALLOWED_RECT_FILL_SITES — route the path \
                 through cull::generate_fill"
            )),
            None => scan.violations.push(format!(
                "{rel}:{line}: generate_filled_path whose path argument is not \
                 `<rect>.path_elements(..)` — route the path through cull::generate_fill"
            )),
        }
    }

    // (c) A vello ClipContext takes only a ViewportSplit; a frust-owned
    // push_clip is listed by receiver; any other receiver fails closed.
    for at in call_offsets(&text, "push_clip") {
        let line = line_of(at);
        let Some(receiver) = method_receiver(&text, at) else {
            scan.violations.push(format!(
                "{rel}:{line}: push_clip without a named receiver — the guard cannot tell a \
                 vello ClipContext from a frust-owned push_clip"
            ));
            continue;
        };
        if listed(VELLO_CLIP_CONTEXTS, rel, receiver) {
            let arg = first_arg(&text, at + "push_clip".len());
            if !is_viewport_split(arg) {
                scan.violations.push(format!(
                    "{rel}:{line}: vello ClipContext `{receiver}.push_clip(..)` takes a path \
                     that is not `ViewportSplit::new(..)` — clip paths go through the cull \
                     pre-pass"
                ));
            }
        } else if !listed(FRUST_OWNED_PUSH_CLIP, rel, receiver) {
            scan.violations.push(format!(
                "{rel}:{line}: push_clip on `{receiver}` is not classified — add it to \
                 VELLO_CLIP_CONTEXTS (vello, must be ViewportSplit) or FRUST_OWNED_PUSH_CLIP \
                 (frust-owned)"
            ));
        }
    }
}

/// Run the scan over every file in [`bounded_flatten_scanned_files`].
fn run_bounded_flatten_scan() -> BoundedFlatten {
    let mut scan = BoundedFlatten::default();
    for path in bounded_flatten_scanned_files() {
        let rel = src_relative(&path);
        let contents = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
        scan_bounded_flatten(&rel, &contents, &mut scan);
    }
    scan
}

#[test]
fn bf_engine_sources_route_curve_capable_paths_through_cull() {
    let mut scan = run_bounded_flatten_scan();

    // Each allow-list entry must match exactly `count` sites: a removed site
    // or a new one on the same receiver fails here, not silently.
    for site in ALLOWED_RECT_FILL_SITES {
        let found = scan
            .rect_fills
            .iter()
            .filter(|(file, receiver)| file == site.file && receiver == site.receiver)
            .count();
        if found != site.count {
            scan.violations.push(format!(
                "ALLOWED_RECT_FILL_SITES expects {} `{}.path_elements(..)` fill site(s) in {}, \
                 found {found} — review the site and update the count deliberately",
                site.count, site.receiver, site.file
            ));
        }
    }

    assert!(
        scan.violations.is_empty(),
        "bounded-flatten contract violated ({} hit(s)):\n{}",
        scan.violations.len(),
        scan.violations.join("\n")
    );
}

#[test]
fn bf_scan_sees_every_scanned_file_and_every_listed_file() {
    let scan = run_bounded_flatten_scan();
    let expected: Vec<String> = bounded_flatten_scanned_files()
        .iter()
        .map(|path| src_relative(path))
        .collect();
    assert!(
        !expected.is_empty(),
        "the BF scan covers no .rs file under src/"
    );
    assert_eq!(
        scan.files_seen, expected,
        "the BF scan must read every .rs file under src/ except compile/cull.rs"
    );
    assert!(
        engine_root().join("src").join(CULL_FILE).is_file(),
        "{CULL_FILE} must exist, so its exclusion is by name and not vacuous"
    );
    for name in ["compile", "text"] {
        assert!(
            expected
                .iter()
                .any(|rel| rel.starts_with(&format!("{name}/"))),
            "the BF scan covers no .rs file under src/{name}/"
        );
    }
    let listed_files = ALLOWED_RECT_FILL_SITES
        .iter()
        .map(|site| site.file)
        .chain(VELLO_CLIP_CONTEXTS.iter().map(|&(file, _)| file))
        .chain(FRUST_OWNED_PUSH_CLIP.iter().map(|&(file, _)| file));
    for file in listed_files {
        assert!(
            expected.iter().any(|rel| rel == file),
            "{file} is on a BF allow-list but is not scanned"
        );
    }
}

/// Violations the scanner reports for `src` as if it were `rel`.
fn bf_violations_in(rel: &str, src: &str) -> Vec<String> {
    let mut scan = BoundedFlatten::default();
    scan_bounded_flatten(rel, src, &mut scan);
    scan.violations
}

#[test]
fn bf_negative_control_direct_bezpath_fill_is_a_violation() {
    let src = r#"
fn paint(generator: &mut StripGenerator, path: &BezPath) {
    generator.generate_filled_path(path.iter(), Fill::NonZero, Affine::IDENTITY, None, storage, clip);
}
"#;
    let violations = bf_violations_in("text/backend.rs", src);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with("text/backend.rs:3:"),
        "{violations:?}"
    );
}

#[test]
fn bf_negative_control_direct_stroke_is_a_violation() {
    let src = "fn stroke(g: &mut StripGenerator, p: &BezPath) {\n    \
               g.generate_stroked_path(p, &stroke, Affine::IDENTITY, None, storage, clip);\n}\n";
    let violations = bf_violations_in("text/backend.rs", src);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].starts_with("text/backend.rs:2:"),
        "{violations:?}"
    );
}

#[test]
fn bf_rect_fill_on_a_listed_site_passes() {
    let src = "fn rect_path(g: &mut StripGenerator, rect: Rect) {\n    \
               g.generate_filled_path(\n        rect.path_elements(FLATTEN_TOLERANCE),\n        \
               Fill::NonZero,\n    );\n}\n";
    assert!(bf_violations_in("compile/parallel.rs", src).is_empty());
}

#[test]
fn bf_rect_fill_outside_the_allow_list_is_a_violation() {
    let src = "fn rect_path(g: &mut StripGenerator, rect: Rect) {\n    \
               g.generate_filled_path(\n        rect.path_elements(FLATTEN_TOLERANCE),\n        \
               Fill::NonZero,\n    );\n}\n";
    let violations = bf_violations_in("compile/layers.rs", src);
    assert_eq!(violations.len(), 1, "{violations:?}");
}

#[test]
fn bf_rect_fill_with_a_chained_path_argument_is_a_violation() {
    let src = "fn rect_path(g: &mut StripGenerator, rect: Rect) {\n    \
               g.generate_filled_path(rect.path_elements(X).into_iter(), Fill::NonZero);\n}\n";
    let violations = bf_violations_in("compile/parallel.rs", src);
    assert_eq!(violations.len(), 1, "{violations:?}");
}

#[test]
fn bf_viewport_split_clip_on_the_mask_stack_passes() {
    let src = "fn push_mask(&mut self, path: P, transform: Affine) {\n    \
               self.masks.push_clip(\n        ViewportSplit::new(path, transform, viewport),\n        \
               generator,\n    );\n}\n";
    assert!(bf_violations_in("compile/clip.rs", src).is_empty());
}

#[test]
fn bf_bezpath_clip_on_the_mask_stack_is_a_violation() {
    let src = "fn push_mask(&mut self, path: &BezPath) {\n    \
               self.masks.push_clip(path.iter(), generator, Fill::NonZero);\n}\n";
    let violations = bf_violations_in("compile/clip.rs", src);
    assert_eq!(violations.len(), 1, "{violations:?}");
}

#[test]
fn bf_frust_owned_group_push_clip_split_across_lines_passes() {
    let src = "fn push(&mut self, path: &BezPath) {\n    \
               self.groups\n        .push_clip(transform.transform_rect_bbox(path.bounding_box()));\n}\n";
    assert!(bf_violations_in("compile/mod.rs", src).is_empty());
}

#[test]
fn bf_unclassified_push_clip_receiver_fails_closed() {
    let src = "fn push(&mut self, path: &BezPath) {\n    self.cache.push_clip(path.iter());\n}\n";
    let violations = bf_violations_in("compile/mod.rs", src);
    assert_eq!(violations.len(), 1, "{violations:?}");
}

#[test]
fn bf_definitions_and_test_modules_are_not_sites() {
    let src = "pub(crate) fn push_clip(&mut self, path: &BezPath) {}\n\
               #[cfg(test)]\nmod tests {\n    \
               fn t(g: &mut G) { g.generate_stroked_path(p); self.masks.push_clip(p); }\n}\n";
    assert!(bf_violations_in("compile/clip.rs", src).is_empty());
}

#[test]
fn bf_longer_identifiers_are_not_sites() {
    let src = "fn f(g: &mut G, p: &BezPath) {\n    g.my_generate_filled_path(p.iter());\n}\n";
    assert!(bf_violations_in("text/backend.rs", src).is_empty());
}
