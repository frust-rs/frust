//! The adversarial corpus's golden gate: every
//! [`frust_testing::corpus::adversarial`] case rendered by both arms of the
//! oracle pair and compared against its own golden class, plus one dedicated
//! memory-budget assertion for the 5,000-nested-layer case.
//!
//! ```text
//! # the CPU arm — no GPU, no environment, part of the ordinary gate:
//! cargo test -p frust-testing --test adversarial
//!
//! # the classic (GPU) arm, on the pinned runner:
//! WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
//!   cargo test -p frust-testing --test adversarial -- --ignored --nocapture
//! ```
//!
//! This file mirrors `tests/goldens.rs`'s own gate structure (golden class
//! `cpu/`, `no_ref` recording instead of comparing, the GPU arm's
//! optional/promotable class) applied to
//! [`frust_testing::corpus::adversarial::adversarial_cases`] instead of the
//! unit corpus — see that file's module docs for the policy rationale this
//! one does not repeat.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use frust_testing::case::CaseSpec;
use frust_testing::corpus::{CorpusCase, adversarial_cases, render_case};
use frust_testing::frame::foreign_font_runs;
use frust_testing::golden::{compare_golden, goldens_root, update_goldens_enabled};
use frust_testing::meta::GoldenMeta;
use frust_testing::oracle_classic::ClassicOracle;
use frust_testing::oracle_cpu::CpuOracle;
use frust_testing::render::SceneRenderer;
use frust_testing::{ORACLE_ID, UNCLASSIFIED_CLASS};

/// The golden class the CPU oracle's baselines live in
/// (`testing/goldens/cpu/`) — the same class `unit_cases` promotes into; an
/// `adv-*` baseline lives beside a `unit-*` one, distinguished by its file
/// stem's prefix (`docs/TESTING.md`'s Golden Classes; the parallel widget/page
/// corpus task promotes `widget-*`/`page-*` filenames into the same
/// directory).
const CPU_CLASS: &str = "cpu";

/// The repository commit this run's baselines would be attributed to — see
/// `tests/goldens.rs`'s identical helper for why this shells out rather than
/// baking the SHA in at build time.
fn frust_commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|sha| sha.trim().to_string())
        .filter(|sha| !sha.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// The current UTC time as an RFC 3339 timestamp — see `tests/goldens.rs`'s
/// identical helper for the civil-date conversion this hand-rolls rather than
/// pulling in a date crate.
fn rfc3339_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let time_of_day = secs % 86_400;
    let (hour, minute, second) = (
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60,
    );

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = era * 400 + yoe + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// The `GoldenMeta` provenance record stored beside a promoted baseline or a
/// review artifact.
fn golden_meta(renderer: &dyn SceneRenderer, spec: &CaseSpec) -> GoldenMeta {
    GoldenMeta::new(
        &renderer.meta(),
        std::env::consts::OS,
        frust_commit(),
        spec.name,
        spec.tolerance,
        rfc3339_now(),
    )
}

/// Whether a baseline PNG already exists for `case` in `class`.
fn baseline_exists(class: &str, case: &CorpusCase) -> bool {
    goldens_root()
        .join(class)
        .join(format!("{}.png", case.spec.name))
        .is_file()
}

/// Whether a run is allowed to write baselines, and whether a missing one is
/// a failure — identical shape to `tests/goldens.rs`'s own `RunPolicy`.
struct RunPolicy {
    class: &'static str,
    require_baseline: bool,
    promotable: bool,
}

/// Renders every adversarial case on `renderer`, checks its probes, compares
/// (or records) its baseline, and panics with EVERY failure at once — the
/// same one-run-reports-everything shape `tests/goldens.rs`'s `run_corpus`
/// uses, so a rasterizer change surfaces every affected adversarial case in
/// one command.
fn run_corpus(renderer: &mut dyn SceneRenderer, policy: &RunPolicy) {
    let update = update_goldens_enabled() && policy.promotable;
    if update_goldens_enabled() && !policy.promotable {
        println!(
            "adversarial: UPDATE_GOLDENS=1 ignored — backend `{}` has no reviewed golden class",
            renderer.id()
        );
    }

    let mut failures: Vec<String> = Vec::new();
    let mut compared = 0_usize;
    let mut recorded = 0_usize;
    let mut promoted = 0_usize;
    let mut skipped = 0_usize;

    for case in adversarial_cases() {
        // Font determinism first, before this case's frame is even rendered —
        // the same ordering `tests/goldens.rs`'s `run_corpus` and
        // `frust_testing::run_cpu_goldens` both use.
        let foreign = foreign_font_runs(&case.scene());
        if !foreign.is_empty() {
            for message in foreign {
                failures.push(format!("[{}] font: {message}", case.spec.name));
            }
            continue;
        }

        let Some(image) = render_case(renderer, &case)
            .unwrap_or_else(|err| panic!("case `{}` failed to render: {err:#}", case.spec.name))
        else {
            skipped += 1;
            println!(
                "adversarial: `{}` skipped on `{}`",
                case.spec.name,
                renderer.id()
            );
            continue;
        };

        let probe_failures = case.failed_probes(&image);
        if !probe_failures.is_empty() {
            for message in probe_failures {
                failures.push(format!("[{}] probe: {message}", case.spec.name));
            }
            continue;
        }

        let mut spec = case.spec.clone();
        if !policy.require_baseline && !baseline_exists(policy.class, &case) {
            spec.no_ref = true;
        }

        let meta = golden_meta(renderer, &spec);
        let outcome = match compare_golden(
            policy.class,
            &spec,
            &image,
            &meta,
            case.eroded_interior,
            update,
        ) {
            Ok(outcome) => outcome,
            Err(err) => {
                failures.push(format!("[{}] {err:#}", case.spec.name));
                continue;
            }
        };

        if outcome.updated {
            promoted += 1;
        } else if spec.no_ref {
            recorded += 1;
        } else {
            compared += 1;
        }

        if !outcome.passed {
            let report = outcome
                .report
                .as_ref()
                .map(|r| {
                    format!(
                        "{} px differ ({:.4}%), max |delta| {:?}, bbox {:?}",
                        r.pixel_count, r.mismatched_percent, r.max_difference, r.bounding_box
                    )
                })
                .unwrap_or_else(|| "no report".to_string());
            let artifacts = outcome
                .artifact_dir
                .as_ref()
                .map(|dir| dir.display().to_string())
                .unwrap_or_else(|| "<none>".to_string());
            failures.push(format!(
                "[{}] golden mismatch under tolerance {:?}: {report}; artifacts in {artifacts}",
                case.spec.name, case.spec.tolerance
            ));
        }
    }

    println!(
        "adversarial: class `{}` — {compared} compared, {recorded} recorded (no baseline yet), \
         {promoted} promoted, {skipped} skipped",
        policy.class
    );

    assert!(
        failures.is_empty(),
        "{} adversarial corpus failure(s) in golden class `{}`:\n{}",
        failures.len(),
        policy.class,
        failures.join("\n")
    );
}

/// Stack given to the dedicated thread [`cpu_corpus_matches_its_goldens`] and
/// [`classic_corpus_matches_its_goldens`] render the WHOLE corpus on (see
/// [`run_on_oversized_stack`]'s docs for why).
const RENDER_THREAD_STACK_BYTES: usize = 64 * 1024 * 1024;

/// Runs `f` on a dedicated thread with [`RENDER_THREAD_STACK_BYTES`] of
/// stack, propagating a panic from `f` as this call's own panic.
///
/// `vello_cpu` 0.2.0's layer compositing recurses roughly once per open
/// layer rather than looping, so `adv-5k-layers`'s 5,000 nested
/// `PushLayer`s overflows the default ~8 MiB thread stack `cargo test`
/// spawns each test on — confirmed by bisection (8 MiB overflows, 16 MiB
/// does not) before this constant was picked. That is a STACK-depth
/// characteristic of the pinned rasterizer, orthogonal to the memory-BUDGET
/// concern `adv-5k-layers` actually exists to assert
/// ([`five_thousand_nested_layers_stay_within_a_bounded_memory_budget`]
/// below); every test that renders the whole corpus runs on this oversized
/// stack so an incidental stack limit never masks what the heap does.
/// Serializes every test in this binary that builds or renders corpus
/// scenes. The memory-budget test below measures a PROCESS-GLOBAL counting
/// allocator, so any sibling test allocating concurrently inflates its
/// observed peak (seen as a deterministic ~30 MB overshoot once
/// `no_case_shapes_against_a_host_font` joined this binary). The lock keeps
/// each allocation-heavy window single-tenant without forcing
/// `--test-threads=1` onto the whole run. Poison is ignored deliberately: one
/// test's failure must not cascade into every sibling.
static RENDER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn render_lock() -> std::sync::MutexGuard<'static, ()> {
    RENDER_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn run_on_oversized_stack<F: FnOnce() + Send + 'static>(f: F) {
    std::thread::Builder::new()
        .stack_size(RENDER_THREAD_STACK_BYTES)
        .spawn(f)
        .expect("spawning the oversized-stack thread")
        .join()
        .expect("oversized-stack thread panicked");
}

/// The gate that runs everywhere: the whole adversarial corpus against the
/// committed `cpu/` baselines, with no GPU involved.
#[test]
fn cpu_corpus_matches_its_goldens() {
    let _serialized = render_lock();
    run_on_oversized_stack(|| {
        let mut oracle = CpuOracle::new();
        run_corpus(
            &mut oracle,
            &RunPolicy {
                class: CPU_CLASS,
                require_baseline: true,
                promotable: true,
            },
        );
    });
}

/// The `cpu/` class directory is the pinned oracle's — the adversarial
/// corpus's own tripwire on the same mapping `tests/goldens.rs` asserts for
/// the unit corpus.
#[test]
fn the_cpu_class_is_backed_by_the_pinned_oracle() {
    assert_eq!(
        CpuOracle::new().id(),
        ORACLE_ID,
        "the `cpu/` golden class is defined as `{ORACLE_ID}`'s output"
    );
}

/// The unclassified fallback is never a committed golden directory — see
/// `tests/goldens.rs`'s identical guard.
#[test]
fn the_unclassified_class_is_not_a_committed_golden_directory() {
    assert!(
        !goldens_root().join(UNCLASSIFIED_CLASS).exists(),
        "`{UNCLASSIFIED_CLASS}` is the refuse-to-promote fallback — it must never become a \
         committed golden class directory"
    );
}

/// No case in the adversarial corpus shapes a glyph against a host font.
///
/// `run_corpus` above already refuses such a case as part of its own font
/// check, but it refuses it as one failure among many in a golden run. This is
/// the same property stated on its own — mirroring
/// `tests/page_goldens.rs`'s `no_case_shapes_against_a_host_font` for the
/// widget/page corpus — so a font regression reads as a font regression
/// rather than as a baseline mismatch.
#[test]
fn no_case_shapes_against_a_host_font() {
    let _serialized = render_lock();
    let mut failures = Vec::new();
    for case in adversarial_cases() {
        for message in foreign_font_runs(&case.scene()) {
            failures.push(format!("[{}] {message}", case.spec.name));
        }
    }
    assert!(
        failures.is_empty(),
        "{} non-portable frame(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The classic (GPU) arm, on real hardware — see `tests/goldens.rs`'s
/// identical test for the full rationale (adapter naming, promotion policy).
///
/// ```text
/// WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
///   cargo test -p frust-testing --test adversarial -- --ignored --nocapture
/// ```
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-testing --test adversarial -- --ignored`"]
fn classic_corpus_matches_its_goldens() {
    let _serialized = render_lock();
    run_on_oversized_stack(|| {
        let mut oracle = ClassicOracle::new(frust_render::HeadlessOptions::default())
            .expect("failed to create the classic (headless GPU) oracle");
        println!("adversarial: classic arm on {}", oracle.headless_meta());
        println!("adversarial: golden class `{}`", oracle.id());

        let class = oracle.id();
        let classified = oracle.is_classified();
        if !classified {
            println!(
                "adversarial: adapter has no reviewed golden class — rendering and probing \
                 every case, comparing none (see tests/goldens.rs's module docs)"
            );
        }
        run_corpus(
            &mut oracle,
            &RunPolicy {
                class,
                require_baseline: false,
                promotable: classified,
            },
        );
    });
}

// --- `adv-5k-layers`'s bounded-memory assertion -----------------------------
//
// A counting `GlobalAlloc` wrapper, scoped to this integration test binary
// only (each `tests/*.rs` file compiles as its own executable, so this never
// touches any other test target's allocator). It tracks BOTH the currently
// outstanding byte count and the highest value that counter has ever reached
// ("peak"), the same "counting allocator" shape the task names as one of its
// two sanctioned bounded-memory techniques (the other being a peak-RSS
// read-out, which `peak_rss_kib` below also provides as a cross-check on
// Linux — the rig this corpus is verified on).

/// Bytes currently allocated through [`CountingAllocator`], process-wide.
static CURRENT_BYTES: AtomicUsize = AtomicUsize::new(0);
/// The highest [`CURRENT_BYTES`] has ever reached.
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Wraps [`System`], counting every allocation/deallocation this process
/// makes so [`five_thousand_nested_layers_stay_within_a_bounded_memory_budget`]
/// can read a peak-usage delta around one specific render call.
struct CountingAllocator;

// SAFETY: every method delegates directly to `System`, which already upholds
// `GlobalAlloc`'s contract; this wrapper only observes the size `System` was
// called with before/after forwarding, and never itself allocates.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwards `layout` unchanged to `System::alloc`.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let now = CURRENT_BYTES.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK_BYTES.fetch_max(now, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwards `ptr`/`layout` unchanged to `System::dealloc`.
        unsafe { System.dealloc(ptr, layout) };
        CURRENT_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwards all arguments unchanged to `System::realloc`.
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            CURRENT_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
            let now = CURRENT_BYTES.fetch_add(new_size, Ordering::Relaxed) + new_size;
            PEAK_BYTES.fetch_max(now, Ordering::Relaxed);
        }
        new_ptr
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// The process's own peak resident-set size, in KiB, read from
/// `/proc/self/status`'s `VmHWM` — the peak-RSS cross-check on the T400 rig
/// (Linux). `None` off Linux or if the field cannot be parsed; the test below
/// treats that as "skip the RSS cross-check", never as a failure, since the
/// counting-allocator bound already stands on its own.
fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest.trim().trim_end_matches("kB").trim().parse().ok();
        }
    }
    None
}

/// Bytes allowed to accumulate as the counting allocator's peak-since-baseline
/// delta while rendering `adv-5k-layers`.
///
/// Generous on purpose: a 64x64 CPU frame with 5,000 open layer groups is a
/// few hundred KiB to a handful of MiB even with one buffer per open layer
/// (`O(depth)`), so 128 MiB comfortably clears that while still catching an
/// `O(depth^2)` blow-up (which would land in the multi-GiB range for
/// `depth = 5,000`) or an unbounded leak.
const LAYER_MEMORY_BUDGET_BYTES: usize = 128 * 1024 * 1024;

/// The peak-RSS cross-check's own (looser) budget, in KiB — looser than
/// [`LAYER_MEMORY_BUDGET_BYTES`] because `VmHWM` is a PROCESS-WIDE high-water
/// mark `cargo test`'s default multi-threaded runner can add unrelated noise
/// to (another test's allocations peaking concurrently), not an isolated
/// delta the way the counting allocator's own before/after read is.
const LAYER_RSS_BUDGET_KIB: u64 = 512 * 1024;

/// `adv-5k-layers` must render within a bounded memory budget — the
/// assertion its own doc comment
/// (`frust_testing::corpus::adversarial`'s `five_thousand_layers`) names as
/// this case's actual contract, since its pixels are an ordinary
/// (already-pinned-elsewhere) nested-alpha composite.
///
/// Runs on [`run_on_oversized_stack`]'s dedicated big-stack thread for the
/// same reason [`cpu_corpus_matches_its_goldens`] does — see that helper's
/// docs.
#[test]
fn five_thousand_nested_layers_stay_within_a_bounded_memory_budget() {
    let case = adversarial_cases()
        .into_iter()
        .find(|case| case.spec.name == "adv-5k-layers")
        .expect("adv-5k-layers must exist in the adversarial corpus");

    let _serialized = render_lock();
    let baseline_current = CURRENT_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline_current, Ordering::Relaxed);
    let rss_before = peak_rss_kib();

    run_on_oversized_stack(move || {
        let mut oracle = CpuOracle::new();
        render_case(&mut oracle, &case)
            .unwrap_or_else(|err| panic!("adv-5k-layers failed to render: {err:#}"));
    });

    let peak = PEAK_BYTES.load(Ordering::Relaxed);
    let delta = peak.saturating_sub(baseline_current);
    assert!(
        delta <= LAYER_MEMORY_BUDGET_BYTES,
        "rendering adv-5k-layers peaked at {delta} bytes above baseline, over the \
         {LAYER_MEMORY_BUDGET_BYTES}-byte budget for 5,000 nested layers — looks like \
         unbounded (or super-linear) per-layer growth rather than O(depth)"
    );

    if let (Some(before), Some(after)) = (rss_before, peak_rss_kib()) {
        let rss_delta = after.saturating_sub(before);
        assert!(
            rss_delta <= LAYER_RSS_BUDGET_KIB,
            "rendering adv-5k-layers grew process peak RSS by {rss_delta} KiB (before {before} \
             KiB, after {after} KiB), over the {LAYER_RSS_BUDGET_KIB} KiB cross-check budget"
        );
    }
}
