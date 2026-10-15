//! Parallel strip generation for the command walk.
//!
//! Generating a command's strips — flattening, tiling and strip rendering —
//! is the bulk of the walk on a frame of moving shapes, and it is the one part
//! of a drawing command that depends on nothing but the command itself: its
//! geometry, its composed transform and the viewport. Everything around it is
//! order-coupled — the depth counter only advances for a draw that produced
//! coverage, the encoded-paint table and the image residency are appended to
//! in painter order, the clip, group and retained stacks are a function of
//! every bracket before the command, and a glyph run's atlas route and
//! resident-run cache are stateful across the whole frame. So the walk splits
//! in three:
//!
//! 1. **Planning**, sequential and cheap ([`plan`]). One pass over the display
//!    list resolves each drawing command's [`Geometry`] against its composed
//!    transform — tracking the retained correction and whether a coverage mask
//!    is open, on the walk's own terms — and lists the ones worth handing to a
//!    worker as [`Job`]s. A pixel-aligned rectangle is never one (writing its
//!    coverage costs less than copying it), a command drawn under a mask is
//!    never one (the mask is generated during the walk itself, so it does not
//!    exist yet), and a glyph run is never one (see [`crate::text`]).
//! 2. **Rasterisation**, on helper threads ([`Pool::run`]). Each helper owns a
//!    [`StripGenerator`] retained across frames and generates jobs into the
//!    job's own [`StripStorage`] slot, unclipped and against the full
//!    viewport — exactly what the walk's own generator would have produced for
//!    the command before its scissor is applied. Helpers claim chunks of jobs
//!    sized by estimated cost from the *back* of the list, so they work away
//!    from the walk rather than racing it.
//! 3. **Merge**, which is the walk itself, run concurrently with the helpers
//!    on the calling thread. At each planned command the walk claims the
//!    command's slot: a slot a helper finished is appended to the frame's
//!    storage with every strip's alpha index rebased onto the frame's alpha
//!    buffer ([`append_rebased`]); a slot no helper reached is generated
//!    inline, exactly as without the pool. Either way the run then takes the
//!    unchanged path — scissor, empty-run rollback, paint encoding, depth —
//!    so the frame is byte-identical to a sequential walk's.
//!
//! The planner's clip tracking only decides which commands are *offered* to a
//! helper. It never decides what a frame contains: the walk takes a helper's
//! run only when the live clip stack has no mask open and the command's
//! composed transform is bit-identical to the one the job was planned under,
//! and generates inline otherwise. A planning mistake costs wasted work, never
//! a wrong pixel.
//!
//! Helpers are spawned per frame inside [`std::thread::scope`] — there is no
//! persistent pool, and no thread outlives the compile that spawned it — and
//! only when the planned work is estimated to outweigh the spawn
//! ([`DEFAULT_PARALLEL_MIN_COST`]). A helper that cannot be spawned (a target
//! with no threads, an exhausted process) leaves its jobs to the walk, which
//! generates them inline. `FRUST_ENGINE_NO_PARALLEL` turns the pool off; the
//! walk is then the plain sequential one.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError, TryLockError};

use kurbo::{Affine, BezPath, PathEl, Rect, RoundedRect, Shape};
use peniko::Fill;

use frust_scene::{Command, DashPattern, PathStyle, Scene};

use vello_common::clip::PathDataRef;
use vello_common::fearless_simd::Level;
use vello_common::strip::Strip;
use vello_common::strip_generator::{GenerationMode, StripGenerator, StripStorage};

use super::blur_rrect::inflated_bounds;
use super::cull::{generate_fill, generate_stroke, rounded_rect_elements};
use super::layers::RetainedStack;
use super::{
    FLATTEN_TOLERANCE, check_finite, command_on_grid, dash_path, fast_rect, retained_entry,
    round_stroke, rounded_rect_radii,
};

/// The most threads a frame's strip generation runs on, the calling thread
/// included, before the machine's own parallelism caps it.
///
/// Three: the walk, which merges, encodes paints and draws glyph runs as well
/// as generating whatever the helpers have not reached, and two helpers — a
/// phone's big cluster is two to four cores. A helper spawned per frame is
/// placed by the scheduler, usually on a little core, so each takes a smaller
/// share of the work than the walk does; the split is decided job by job at
/// run time rather than up front, so a slow helper only ever takes less.
pub const DEFAULT_PARALLEL_WORKERS: usize = 3;

/// The estimated cost a frame's planned work must reach before a helper is
/// spawned for it.
///
/// A planned command costs a fixed 64 units plus one unit per device pixel of
/// its bounding box's half-perimeter — a proxy for the tiles its outline
/// crosses, which is what flattening, tiling and strip rendering scale with.
/// A 40-pixel circle costs about 150, so this is a few dozen shapes: below it
/// the per-frame spawn costs more than the generation a helper could take off
/// the walk, and a frame of a few shapes, or of one full-screen rectangle
/// (which plans nothing), never spawns at all.
pub const DEFAULT_PARALLEL_MIN_COST: u64 = 8_000;

/// The fixed part of a job's estimated cost: the flattener, tiler and strip
/// renderer's per-call work, independent of the shape's extent.
const JOB_BASE_COST: u64 = 64;

/// How many slots the pool keeps across frames, and how many strips and
/// alpha bytes one kept slot keeps its capacity for.
///
/// A slot holds one command's run until the walk takes it, so a frame of a
/// thousand planned shapes needs a thousand slots that frame — and would
/// otherwise keep them, each at the capacity of the largest run it ever
/// held, for the compiler's lifetime. After each run every slot is emptied
/// and the pool is trimmed back to these bounds; a frame past them pays the
/// allocations again, a frame within them pays nothing. The most a pool can
/// hold is therefore `MAX_RETAINED_SLOTS * (RETAINED_SLOT_STRIPS *
/// size_of::<Strip>() + RETAINED_SLOT_ALPHAS)`, about 20 MiB, and in practice
/// the sum of the largest run each slot ever held: the S1 bench frame plans
/// about 500 jobs totalling 8.7 k strips and 350 KB of alphas on the two
/// phones, a few dozen strips and a few hundred bytes per slot.
const MAX_RETAINED_SLOTS: usize = 1024;
const RETAINED_SLOT_STRIPS: usize = 256;
const RETAINED_SLOT_ALPHAS: usize = 16 * 1024;

/// The most threads [`crate::compile::SceneCompiler::set_parallel_workers`]
/// accepts, the walk included: a caller's count past this is a mistake, not a
/// bigger machine, and a count past the machine only pays for threads that
/// cannot run. The variable's path is capped by the machine instead.
pub const MAX_PARALLEL_WORKERS: usize = 8;

/// How many chunks each helper's share of the planned cost is split into.
///
/// More chunks balance several helpers better; fewer cost fewer claims. The
/// walk claims job by job regardless, so this never decides who generates a
/// job the walk reaches first.
const CHUNKS_PER_HELPER: u64 = 8;

/// The largest alpha index a strip can carry: its top bit is the fill-gap
/// flag (see [`Strip::set_alpha_idx`]).
const MAX_ALPHA_INDEX: u32 = u32::MAX >> 1;

/// How many threads the machine offers, read once per process: the standard
/// library answers it from the affinity mask and cgroup quotas, which is file
/// I/O that has no place on a per-frame path.
pub(crate) fn available_threads() -> usize {
    static THREADS: OnceLock<usize> = OnceLock::new();
    *THREADS.get_or_init(|| std::thread::available_parallelism().map_or(1, usize::from))
}

/// The worker count a compiler starts with: [`DEFAULT_PARALLEL_WORKERS`] — or
/// `FRUST_ENGINE_PARALLEL_WORKERS` — capped by the machine, or one, the
/// sequential walk, under `FRUST_ENGINE_NO_PARALLEL`.
#[must_use]
pub(crate) fn default_workers() -> usize {
    if crate::config::parallel_walk_disabled() {
        1
    } else {
        crate::config::parallel_workers()
            .unwrap_or(DEFAULT_PARALLEL_WORKERS)
            .min(available_threads())
            .max(1)
    }
}

/// What one drawing command rasterizes, resolved against its composed
/// transform: the single description both the walk and a helper generate a
/// command's strips from, so the two can never lower a command differently.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Geometry<'a> {
    /// A pixel-aligned device rectangle, written directly as coverage.
    FastRect(Rect),
    /// A rectangle that needs full path processing.
    RectPath {
        /// The rectangle in the command's own space.
        rect: Rect,
        /// The composed transform.
        transform: Affine,
    },
    /// A filled rounded rectangle.
    Rounded {
        /// The shape in the command's own space.
        shape: RoundedRect,
        /// The composed transform.
        transform: Affine,
    },
    /// A stroked line segment, round-capped.
    Line {
        /// The segment's two elements.
        line: [PathEl; 2],
        /// The stroke width.
        width: f64,
        /// The composed transform.
        transform: Affine,
    },
    /// A filled path.
    Fill {
        /// The path in the command's own space.
        path: &'a BezPath,
        /// The composed transform.
        transform: Affine,
    },
    /// A stroked path, round-capped and round-joined, dashed when `dash` is
    /// an effective pattern.
    Stroke {
        /// The path in the command's own space.
        path: &'a BezPath,
        /// The stroke width.
        width: f64,
        /// The dash pattern, present only when it is effective.
        dash: Option<DashPattern>,
        /// The composed transform.
        transform: Affine,
    },
}

impl<'a> Geometry<'a> {
    /// The geometry of a rectangle drawn under `transform`: the fast
    /// rectangle path whenever [`fast_rect`] admits it.
    pub(crate) fn rect(rect: Rect, transform: Affine) -> Self {
        match fast_rect(rect, transform) {
            Some(device) => Self::FastRect(device),
            None => Self::RectPath { rect, transform },
        }
    }

    /// The geometry `command` rasterizes under `combined` (the frame root with
    /// any retained correction), or `None` for a command that is not a
    /// strip-generated shape — a bracket, a glyph run, an external texture.
    pub(crate) fn of(command: &'a Command, combined: Affine) -> Option<Self> {
        let geometry = match command {
            Command::FillRect {
                rect, transform, ..
            } => Self::rect(*rect, combined * *transform),
            Command::RoundedRect {
                rect,
                radii,
                transform,
                ..
            } => Self::Rounded {
                shape: RoundedRect::from_rect(*rect, rounded_rect_radii(*radii)),
                transform: combined * *transform,
            },
            Command::Line {
                p0,
                p1,
                width,
                transform,
                ..
            } => Self::Line {
                // The two elements `kurbo::Line::path_elements` yields, spelled
                // out so the stroke helper can walk them twice.
                line: [PathEl::MoveTo(*p0), PathEl::LineTo(*p1)],
                width: *width,
                transform: combined * *transform,
            },
            Command::Path {
                path,
                style,
                transform,
                ..
            } => match style {
                PathStyle::Fill => Self::Fill {
                    path,
                    transform: combined * *transform,
                },
                PathStyle::Stroke { width, dash } => Self::Stroke {
                    path,
                    width: *width,
                    dash: dash.filter(DashPattern::is_effective),
                    transform: combined * *transform,
                },
            },
            Command::Image {
                dest, transform, ..
            } => Self::rect(*dest, combined * *transform),
            // The strip generator rasterizes the padded bounding rectangle,
            // not the rounded shape (see [`super::blur_rrect`]).
            Command::BlurredRoundedRect {
                rect,
                std_dev,
                transform,
                ..
            } => Self::rect(inflated_bounds(*rect, *std_dev), combined * *transform),
            _ => return None,
        };
        Some(geometry)
    }

    /// Whether this is the fast rectangle path.
    pub(crate) fn is_fast_rect(&self) -> bool {
        matches!(self, Self::FastRect(_))
    }

    /// Generate this geometry's strips into `storage` under `clip`.
    ///
    /// The one place a shape command's generator call is spelled, for the walk
    /// and the helpers alike.
    pub(crate) fn generate(
        &self,
        generator: &mut StripGenerator,
        storage: &mut StripStorage,
        clip: Option<PathDataRef<'_>>,
    ) {
        match *self {
            Self::FastRect(device) => {
                generator.generate_filled_rect_fast(&device, storage, clip);
            }
            Self::RectPath { rect, transform } => {
                generator.generate_filled_path(
                    rect.path_elements(FLATTEN_TOLERANCE),
                    Fill::NonZero,
                    transform,
                    None,
                    storage,
                    clip,
                );
            }
            Self::Rounded { shape, transform } => {
                generate_fill(
                    generator,
                    rounded_rect_elements(&shape),
                    transform,
                    storage,
                    clip,
                );
            }
            Self::Line {
                line,
                width,
                transform,
            } => {
                generate_stroke(
                    generator,
                    line,
                    &round_stroke(width),
                    transform,
                    storage,
                    clip,
                );
            }
            Self::Fill { path, transform } => {
                generate_fill(generator, path.iter(), transform, storage, clip);
            }
            Self::Stroke {
                path,
                width,
                dash,
                transform,
            } => {
                let stroke = round_stroke(width);
                // A dash pattern is expanded into its own sub-paths before the
                // stroker runs, the same lowering the display list's other
                // consumers apply: the pattern never reaches a backend's own
                // dash support, so every rasterizer sees the identical
                // geometry.
                match dash {
                    Some(dash) => {
                        let dashed = dash_path(path, dash);
                        generate_stroke(
                            generator,
                            dashed.iter(),
                            &stroke,
                            transform,
                            storage,
                            clip,
                        );
                    }
                    None => {
                        generate_stroke(generator, path.iter(), &stroke, transform, storage, clip);
                    }
                }
            }
        }
    }

    /// This geometry's estimated generation cost in [`DEFAULT_PARALLEL_MIN_COST`]'s
    /// units: [`JOB_BASE_COST`] plus the half-perimeter of its device-space
    /// control box, clamped to the viewport.
    ///
    /// An estimate for balancing and for the spawn decision only — nothing
    /// that decides what a frame contains reads it.
    fn cost(&self, viewport: Rect) -> u64 {
        let (bounds, transform, reach) = match *self {
            Self::FastRect(device) => (device, Affine::IDENTITY, 0.0),
            Self::RectPath { rect, transform } => (rect, transform, 0.0),
            Self::Rounded { shape, transform } => (shape.rect(), transform, 0.0),
            Self::Line {
                line,
                width,
                transform,
            } => (control_box(line.iter().copied()), transform, width),
            Self::Fill { path, transform } => (control_box(path.iter()), transform, 0.0),
            Self::Stroke {
                path,
                width,
                transform,
                ..
            } => (control_box(path.iter()), transform, width),
        };
        let device = transform
            .transform_rect_bbox(bounds.inflate(reach, reach))
            .intersect(viewport);
        let extent = (device.width().max(0.0) + device.height().max(0.0)).min(1e9);
        // Truncation is the intent: this is an estimate, and a NaN extent
        // (a degenerate transform) reads as zero.
        JOB_BASE_COST.saturating_add(extent as u64)
    }
}

/// The bounding box of every point `elements` names, control points included.
fn control_box(elements: impl Iterator<Item = PathEl>) -> Rect {
    let mut bounds: Option<Rect> = None;
    let mut include = |point: kurbo::Point| {
        bounds = Some(bounds.map_or(Rect::from_points(point, point), |b| b.union_pt(point)));
    };
    for element in elements {
        match element {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => include(p),
            PathEl::QuadTo(p1, p2) => {
                include(p1);
                include(p2);
            }
            PathEl::CurveTo(p1, p2, p3) => {
                include(p1);
                include(p2);
                include(p3);
            }
            PathEl::ClosePath => {}
        }
    }
    bounds.unwrap_or(Rect::ZERO)
}

/// One drawing command offered to a helper.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Job<'a> {
    /// The command's index in the display list.
    command: usize,
    /// The frame root with the retained correction the planner composed for
    /// this command — compared bit for bit against the walk's own before a
    /// helper's run is taken.
    combined: Affine,
    /// What to generate.
    geometry: Geometry<'a>,
    /// The estimated cost, for chunking.
    cost: u64,
}

/// A frame's planned helper work: the jobs in painter order and their total
/// estimated cost.
#[derive(Debug, Default)]
pub(crate) struct Plan<'a> {
    jobs: Vec<Job<'a>>,
    cost: u64,
}

impl Plan<'_> {
    /// Whether no command was offered to a helper.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// The planned work's total estimated cost.
    #[must_use]
    pub(crate) fn cost(&self) -> u64 {
        self.cost
    }
}

/// Plan `scene`'s helper work for a `viewport`-sized frame under `root`.
///
/// Mirrors the walk's bracket handling on its exact terms — the same
/// [`command_on_grid`] check, the same [`retained_entry`] substitution, the
/// same retained-layer rule — but tracks only what decides whether a command
/// is worth offering: the retained correction its transform composes with, and
/// whether any open bracket lowered to a coverage mask. An off-grid bracket is
/// tracked as the scissor that admits nothing it lowers to. The walk re-checks
/// both before it takes a helper's run, so an approximation here can only
/// waste work.
#[cfg(test)]
pub(crate) fn plan<'a>(scene: &'a Scene, root: Affine, viewport: Rect) -> Plan<'a> {
    plan_with(scene, root, viewport, false)
}

/// [`plan`] for a compiler whose retention is `retain`
/// (`SceneCompiler::set_retained_layers`).
///
/// Retention changes one bracket decision the mirror has to make the same way
/// the walk does: with it on, the outermost `PushRetained` opens a group
/// whatever its alpha — a clipped layer below full opacity, an unclipped one at
/// it — so the group count a stray pop inside the body is measured against
/// agrees with the walk's. A `DrawRetained` stays a zero-strip command either
/// way: the walk records an empty layer for it, which opens no bracket.
pub(crate) fn plan_with<'a>(
    scene: &'a Scene,
    root: Affine,
    viewport: Rect,
    retain: bool,
) -> Plan<'a> {
    let mut plan = Plan::default();
    let mut retained_stack = RetainedStack::new();
    // One entry per open bracket: whether its clip lowered to a mask.
    let mut brackets: Vec<bool> = Vec::new();
    let mut masks = 0_usize;
    let push = |brackets: &mut Vec<bool>, masks: &mut usize, mask: bool| {
        brackets.push(mask);
        *masks += usize::from(mask);
    };
    let pop = |brackets: &mut Vec<bool>, masks: &mut usize| {
        if brackets.pop() == Some(true) {
            *masks = masks.saturating_sub(1);
        }
    };

    for (index, command) in scene.commands().iter().enumerate() {
        // Every match below is exhaustive on purpose: a command variant added
        // to the walk fails to compile here until it is given a planning
        // decision too, so the mirror cannot drift silently.
        match command {
            // Neither a shape nor a bracket: nothing to plan and nothing to
            // track, so a page of text costs the planner one match per run.
            Command::GlyphRun(_)
            | Command::ClearRect { .. }
            | Command::ShaderQuad { .. }
            | Command::SceneTexture { .. }
            // A zero-strip command: the walk records at most an empty layer
            // for it (and, with retention off, the up-front validation refuses
            // a frame carrying one).
            | Command::DrawRetained { .. }
            // A layer of its own the walk opens and closes around one draw it
            // generates itself, under whatever mask is open: no bracket stays
            // open past it, and its coverage is never offered to a helper.
            | Command::BackdropBlur { .. } => continue,
            Command::FillRect { .. }
            | Command::RoundedRect { .. }
            | Command::Line { .. }
            | Command::Image { .. }
            | Command::BlurredRoundedRect { .. }
            | Command::Path { .. }
            | Command::PushClip { .. }
            | Command::PushClipRounded { .. }
            | Command::PushClipPath { .. }
            | Command::PopClip
            | Command::PushLayer { .. }
            | Command::PopLayer
            | Command::PushRetained { .. }
            | Command::PopRetained => {}
        }
        let combined = root * retained_stack.correction();
        let on_grid = command_on_grid(command, combined);
        if !on_grid {
            match command {
                Command::PushClip { .. }
                | Command::PushClipRounded { .. }
                | Command::PushClipPath { .. }
                | Command::PushLayer { .. } => push(&mut brackets, &mut masks, false),
                Command::PushRetained {
                    rect,
                    scale,
                    transform,
                    ..
                } => {
                    let (scale, transform) = retained_entry(on_grid, *scale, *transform);
                    retained_stack.enter(*rect, scale, transform);
                }
                // An off-grid shape is refused by the walk, and a pop carries
                // no transform to be off the grid with.
                Command::FillRect { .. }
                | Command::RoundedRect { .. }
                | Command::Line { .. }
                | Command::Image { .. }
                | Command::BlurredRoundedRect { .. }
                | Command::Path { .. }
                | Command::PopClip
                | Command::PopLayer
                | Command::PopRetained
                | Command::GlyphRun(_)
                | Command::ClearRect { .. }
                | Command::ShaderQuad { .. }
                | Command::SceneTexture { .. }
                | Command::DrawRetained { .. }
                | Command::BackdropBlur { .. } => {}
            }
            continue;
        }

        match command {
            Command::PushClip { rect, transform }
            | Command::PushLayer {
                rect, transform, ..
            } => {
                let mask = fast_rect(*rect, combined * *transform).is_none();
                push(&mut brackets, &mut masks, mask);
            }
            Command::PushClipRounded {
                rect,
                radii,
                transform,
            } => {
                let square = radii.top_left <= 0.0
                    && radii.top_right <= 0.0
                    && radii.bottom_right <= 0.0
                    && radii.bottom_left <= 0.0;
                let mask = !square || fast_rect(*rect, combined * *transform).is_none();
                push(&mut brackets, &mut masks, mask);
            }
            // Always a mask (`ClipStack::push_path`): every draw under it is
            // generated during the walk itself.
            Command::PushClipPath { .. } => push(&mut brackets, &mut masks, true),
            Command::PopClip | Command::PopLayer => pop(&mut brackets, &mut masks),
            Command::PushRetained {
                rect,
                alpha,
                scale,
                transform,
                ..
            } => {
                if retained_stack.enter(*rect, *scale, *transform) {
                    let corrected = root * retained_stack.correction() * *transform;
                    if (retain || *alpha < 1.0) && check_finite(corrected).is_ok() {
                        // Below full opacity the walk clips the body to its
                        // rectangle; at it (retention on) the group clips
                        // nothing at all.
                        let mask = *alpha < 1.0 && fast_rect(*rect, corrected).is_none();
                        push(&mut brackets, &mut masks, mask);
                        retained_stack.record_layer(brackets.len());
                    }
                }
            }
            Command::PopRetained => {
                if retained_stack.leave(brackets.len()) {
                    pop(&mut brackets, &mut masks);
                }
            }
            Command::FillRect { .. }
            | Command::RoundedRect { .. }
            | Command::Line { .. }
            | Command::Image { .. }
            | Command::BlurredRoundedRect { .. }
            | Command::Path { .. } => {
                // Under a mask the run is generated during the walk itself.
                if masks == 0
                    && let Some(geometry) = Geometry::of(command, combined)
                    && !geometry.is_fast_rect()
                {
                    let cost = geometry.cost(viewport);
                    plan.cost = plan.cost.saturating_add(cost);
                    plan.jobs.push(Job {
                        command: index,
                        combined,
                        geometry,
                        cost,
                    });
                }
            }
            Command::GlyphRun(_)
            | Command::ClearRect { .. }
            | Command::ShaderQuad { .. }
            | Command::SceneTexture { .. }
            | Command::DrawRetained { .. }
            | Command::BackdropBlur { .. } => {}
        }
    }
    plan
}

/// Where a job's helper-generated run waits for the walk.
#[derive(Debug)]
struct Slot {
    state: SlotState,
    /// The job's own strips and alphas, alpha indices relative to this
    /// storage's start.
    storage: StripStorage,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            state: SlotState::Pending,
            storage: StripStorage::new(GenerationMode::Append),
        }
    }
}

/// Who has a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotState {
    /// Nobody yet: the first of a helper and the walk to lock it decides.
    Pending,
    /// A helper generated it; the walk appends it.
    Ready,
    /// The walk reached it first and generates it inline.
    Taken,
}

/// The retained half of the pool: one slot per job and one generator per
/// helper, kept across frames for their allocations alone.
#[derive(Debug, Default)]
pub(crate) struct Pool {
    slots: Vec<Mutex<Slot>>,
    helpers: Vec<StripGenerator>,
}

impl Pool {
    /// Run `walk` on the calling thread while `helpers` threads generate
    /// `plan`'s jobs behind it, every thread joined before this returns.
    ///
    /// `walk` is handed the [`Merge`] it claims each planned command's run
    /// through. `level`, `width` and `height` size the helpers' generators
    /// exactly as the walk's own is sized, which is what makes a helper's run
    /// the run the walk would have generated.
    pub(crate) fn run<R>(
        &mut self,
        plan: &Plan<'_>,
        helpers: usize,
        level: Level,
        (width, height): (u16, u16),
        walk: impl FnOnce(&Merge<'_, '_>) -> R,
    ) -> R {
        let jobs = plan.jobs.as_slice();
        if self.slots.len() < jobs.len() {
            self.slots.resize_with(jobs.len(), Mutex::default);
        }
        // Exclusive access before any helper exists, so no lock is taken.
        for slot in &mut self.slots[..jobs.len()] {
            slot.get_mut().unwrap_or_else(PoisonError::into_inner).state = SlotState::Pending;
        }
        while self.helpers.len() < helpers {
            self.helpers.push(StripGenerator::new(width, height, level));
        }

        let chunks = chunk(jobs, helpers);
        let slots = &self.slots[..jobs.len()];
        let claimed = AtomicUsize::new(0);
        let reached = AtomicUsize::new(0);
        let done = AtomicBool::new(false);
        #[cfg(test)]
        let finished = AtomicUsize::new(0);

        let helper = Helper {
            jobs,
            chunks: &chunks,
            slots,
            claimed: &claimed,
            reached: &reached,
            done: &done,
            size: (width, height),
        };
        let result = std::thread::scope(|scope| {
            for generator in self.helpers.iter_mut().take(helpers) {
                // A helper that cannot be spawned leaves its jobs to the walk,
                // which generates every slot it finds pending inline — said
                // once per process, so a thread-starved target's sequential
                // frames are attributable rather than silent.
                #[cfg(test)]
                let finished = &finished;
                if let Err(err) = std::thread::Builder::new().spawn_scoped(scope, move || {
                    helper.work(generator);
                    #[cfg(test)]
                    finished.fetch_add(1, Ordering::Release);
                }) {
                    static SAID: AtomicBool = AtomicBool::new(false);
                    if !SAID.swap(true, Ordering::Relaxed) {
                        log::warn!(
                            "frust-engine: could not spawn a strip helper thread ({err}); the walk \
                             generates its share inline"
                        );
                    }
                }
            }
            let merge = Merge {
                jobs,
                slots,
                next: std::cell::Cell::new(0),
                reached: &reached,
                #[cfg(test)]
                finished: &finished,
            };
            let result = walk(&merge);
            // Nothing the walk has passed is read again; whatever a helper is
            // still holding is finished and dropped as the scope joins.
            done.store(true, Ordering::Release);
            result
        });
        self.trim();
        result
    }

    /// Give back what one unusually large frame left behind: slots past
    /// [`MAX_RETAINED_SLOTS`] and, in the slots kept, capacity past
    /// [`RETAINED_SLOT_STRIPS`] strips and [`RETAINED_SLOT_ALPHAS`] alpha bytes.
    /// Every slot is emptied first — nothing reads a run after its walk, and
    /// `shrink_to` never cuts below a length — so a slot that held a huge run
    /// gives that run back too. The helpers' generators are kept as they are:
    /// there is one per helper, never more than the machine has threads.
    fn trim(&mut self) {
        self.slots.truncate(MAX_RETAINED_SLOTS);
        for slot in &mut self.slots {
            let storage = &mut slot
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner)
                .storage;
            storage.clear();
            storage.strips.shrink_to(RETAINED_SLOT_STRIPS);
            storage.alphas.shrink_to(RETAINED_SLOT_ALPHAS);
        }
    }

    /// How many slots the pool holds, and the largest strip and alpha
    /// capacity any of them keeps.
    #[cfg(test)]
    fn retained(&mut self) -> (usize, usize, usize) {
        let mut strips = 0;
        let mut alphas = 0;
        for slot in &mut self.slots {
            let storage = &slot
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner)
                .storage;
            strips = strips.max(storage.strips.capacity());
            alphas = alphas.max(storage.alphas.capacity());
        }
        (self.slots.len(), strips, alphas)
    }
}

/// Split `jobs` into contiguous chunks of roughly equal estimated cost.
fn chunk(jobs: &[Job<'_>], helpers: usize) -> Vec<Range<usize>> {
    let total: u64 = jobs.iter().map(|job| job.cost).sum();
    let pieces = (helpers as u64).max(1).saturating_mul(CHUNKS_PER_HELPER);
    let target = (total / pieces).max(1);
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut acc = 0_u64;
    for (index, job) in jobs.iter().enumerate() {
        acc = acc.saturating_add(job.cost);
        if acc >= target {
            chunks.push(start..index + 1);
            start = index + 1;
            acc = 0;
        }
    }
    if start < jobs.len() {
        chunks.push(start..jobs.len());
    }
    chunks
}

/// One helper thread's borrowed view of the frame's plan.
#[derive(Clone, Copy)]
struct Helper<'p, 'a> {
    jobs: &'p [Job<'a>],
    chunks: &'p [Range<usize>],
    slots: &'p [Mutex<Slot>],
    /// How many chunks helpers have claimed, counted from the back.
    claimed: &'p AtomicUsize,
    /// How many jobs the walk has reached; a helper never starts one below.
    reached: &'p AtomicUsize,
    done: &'p AtomicBool,
    /// The viewport the walk's generator is sized for.
    size: (u16, u16),
}

impl Helper<'_, '_> {
    /// Generate chunks from the back of the plan until the walk catches up,
    /// the chunks run out, or the walk finishes.
    fn work(self, generator: &mut StripGenerator) {
        generator.reset(self.size.0, self.size.1);
        loop {
            let claim = self.claimed.fetch_add(1, Ordering::Relaxed);
            let Some(chunk) = self
                .chunks
                .len()
                .checked_sub(claim.saturating_add(1))
                .and_then(|index| self.chunks.get(index))
            else {
                return;
            };
            // Back to front, so that meeting a job the walk has reached means
            // every job below it is the walk's too.
            for index in chunk.clone().rev() {
                if !self.generate(index, generator) {
                    return;
                }
            }
        }
    }

    /// Generate job `index` into its slot, answering whether to go on.
    fn generate(&self, index: usize, generator: &mut StripGenerator) -> bool {
        let (Some(job), Some(slot)) = (self.jobs.get(index), self.slots.get(index)) else {
            return false;
        };
        // The walk holding the lock means the walk has reached this job.
        let mut slot = match slot.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::WouldBlock | TryLockError::Poisoned(_)) => return false,
        };
        // Checked under the lock: the walk advances `reached` before it locks
        // a slot, so a job it has reached is never started here.
        if slot.state != SlotState::Pending
            || self.done.load(Ordering::Acquire)
            || index < self.reached.load(Ordering::Acquire)
        {
            return false;
        }
        slot.storage.clear();
        job.geometry.generate(generator, &mut slot.storage, None);
        slot.state = SlotState::Ready;
        true
    }
}

/// The walk's side of the pool: claims each planned command's run in painter
/// order.
pub(crate) struct Merge<'p, 'a> {
    jobs: &'p [Job<'a>],
    slots: &'p [Mutex<Slot>],
    /// The first job the walk has not yet passed.
    next: std::cell::Cell<usize>,
    reached: &'p AtomicUsize,
    /// How many helpers have returned from their work loop: what a test
    /// waits on instead of a slot, since a lock taken on a slot is, to the
    /// helper, the walk having reached that job.
    #[cfg(test)]
    finished: &'p AtomicUsize,
}

impl<'p> Merge<'p, '_> {
    /// How many helper threads have finished their share of the plan — every
    /// helper, once the walk is past the last job any of them could claim.
    #[cfg(test)]
    fn helpers_finished(&self) -> usize {
        self.finished.load(Ordering::Acquire)
    }

    /// The claim on the run planned for display-list command `command` drawn
    /// under `combined`, or `None` when none was planned or it was planned
    /// under a different transform.
    ///
    /// Asked for every command in order, which is what moves the walk's
    /// position past jobs it skipped and stops helpers from starting them.
    pub(crate) fn claim(&self, command: usize, combined: Affine) -> Option<Claim<'p>> {
        let mut next = self.next.get();
        while self.jobs.get(next).is_some_and(|job| job.command < command) {
            next += 1;
        }
        let job = self.jobs.get(next).filter(|job| job.command == command);
        let passed = next + usize::from(job.is_some());
        self.next.set(passed);
        self.reached.store(passed, Ordering::Release);

        let job = job?;
        let same = job
            .combined
            .as_coeffs()
            .iter()
            .zip(combined.as_coeffs().iter())
            .all(|(a, b)| a.to_bits() == b.to_bits());
        same.then(|| Claim {
            slot: &self.slots[next],
        })
    }
}

/// The walk's claim on one planned command's run.
pub(crate) struct Claim<'p> {
    slot: &'p Mutex<Slot>,
}

impl Claim<'_> {
    /// Append the helper's run to `storage` with its alpha indices rebased,
    /// answering whether there was one. `false` means no helper got to it —
    /// and none now will — so the caller generates it inline.
    ///
    /// Waits for a helper that is generating the run right now: the walk
    /// needs runs in painter order, and that one is already in flight.
    pub(crate) fn append_to(self, storage: &mut StripStorage) -> bool {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        match slot.state {
            SlotState::Ready => append_rebased(&slot.storage, storage),
            SlotState::Pending | SlotState::Taken => {
                slot.state = SlotState::Taken;
                false
            }
        }
    }
}

/// Append `run` — one generation's strips with alpha indices relative to its
/// own alpha buffer — to `storage`, rebasing every index onto `storage`'s
/// alpha buffer, answering whether it was appended.
///
/// The result is exactly what generating the run straight into `storage`
/// produces: the strip renderers read nothing of a storage but its alpha
/// buffer's length, which is what every index they write is offset by. A run
/// whose rebased indices would not fit a strip's 31-bit field is refused and
/// `storage` left untouched, so the caller's inline generation meets the
/// limit exactly where it always has.
pub(crate) fn append_rebased(run: &StripStorage, storage: &mut StripStorage) -> bool {
    let Ok(base) = u32::try_from(storage.alphas.len()) else {
        return false;
    };
    let fits = run.strips.iter().all(|strip| {
        strip
            .alpha_idx()
            .checked_add(base)
            .is_some_and(|index| index <= MAX_ALPHA_INDEX)
    });
    if !fits {
        return false;
    }
    storage.strips.extend(run.strips.iter().map(|strip| {
        let mut strip: Strip = *strip;
        strip.set_alpha_idx(strip.alpha_idx() + base);
        strip
    }));
    storage.alphas.extend_from_slice(&run.alphas);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    use kurbo::Circle;

    fn generator() -> StripGenerator {
        StripGenerator::new(256, 256, Level::try_detect().unwrap_or(Level::baseline()))
    }

    /// A run generated on its own and rebased onto a non-empty storage is the
    /// run generated straight into that storage.
    #[test]
    fn a_rebased_run_equals_one_generated_in_place() {
        let first = Circle::new((60.0, 60.0), 40.0).to_path(0.1);
        let second = Circle::new((130.0, 90.0), 55.5).to_path(0.1);
        let fill = |path| Geometry::Fill {
            path,
            transform: Affine::IDENTITY,
        };

        let mut direct = StripStorage::new(GenerationMode::Append);
        let mut walk = generator();
        fill(&first).generate(&mut walk, &mut direct, None);
        fill(&second).generate(&mut walk, &mut direct, None);

        let mut merged = StripStorage::new(GenerationMode::Append);
        fill(&first).generate(&mut walk, &mut merged, None);
        let mut run = StripStorage::new(GenerationMode::Append);
        fill(&second).generate(&mut generator(), &mut run, None);
        assert!(append_rebased(&run, &mut merged));

        assert_eq!(direct.strips, merged.strips);
        assert_eq!(direct.alphas, merged.alphas);
    }

    /// With the walk held back until the helper has finished, every planned
    /// command's run comes from the helper, and the merged storage is the one
    /// the walk would have generated on its own.
    #[test]
    fn a_helper_generates_every_run_the_walk_lets_it() {
        let mut scene = Scene::new();
        {
            let mut builder = frust_scene::SceneBuilder::new(&mut scene);
            for i in 0..40 {
                let center =
                    kurbo::Point::new(20.0 + f64::from(i) * 5.5, 30.0 + f64::from(i % 7) * 25.0);
                let path = Circle::new(center, 9.0 + f64::from(i % 5)).to_path(0.1);
                builder.fill_path(path.clone(), peniko::Brush::Solid(peniko::Color::BLACK));
                builder.stroke_path(path, 1.5, peniko::Brush::Solid(peniko::Color::WHITE));
            }
        }
        let viewport = Rect::new(0.0, 0.0, 256.0, 256.0);
        let plan = plan(&scene, Affine::IDENTITY, viewport);
        assert_eq!(plan.jobs.len(), 80);

        let level = Level::try_detect().unwrap_or(Level::baseline());
        let mut expected = StripStorage::new(GenerationMode::Append);
        let mut walk = generator();
        for job in &plan.jobs {
            job.geometry.generate(&mut walk, &mut expected, None);
        }

        let mut pool = Pool::default();
        let merged = pool.run(&plan, 1, level, (256, 256), |merge| {
            // Waited for without touching a slot: a lock taken here is, to
            // the helper, the walk having reached that job, and it stops.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while merge.helpers_finished() < 1 {
                assert!(std::time::Instant::now() < deadline, "the helper finished");
                std::thread::yield_now();
            }
            assert!(merge.slots.iter().all(|slot| {
                slot.lock().unwrap_or_else(PoisonError::into_inner).state == SlotState::Ready
            }));
            let mut merged = StripStorage::new(GenerationMode::Append);
            for job in &plan.jobs {
                let claim = merge.claim(job.command, Affine::IDENTITY).expect("planned");
                assert!(claim.append_to(&mut merged), "the helper generated it");
            }
            merged
        });
        assert_eq!(expected.strips, merged.strips);
        assert_eq!(expected.alphas, merged.alphas);

        // A transform the job was not planned under is never served.
        let merged = pool.run(&plan, 0, level, (256, 256), |merge| {
            let first = plan.jobs[0].command;
            merge.claim(first, Affine::translate((0.5, 0.0))).is_none()
        });
        assert!(merged);
    }

    #[test]
    fn a_run_past_the_alpha_index_field_is_refused() {
        let mut run = StripStorage::new(GenerationMode::Append);
        run.strips.push(Strip::new(0, 0, MAX_ALPHA_INDEX, false));
        let mut storage = StripStorage::new(GenerationMode::Append);
        storage.alphas.push(0);
        assert!(!append_rebased(&run, &mut storage));
        assert!(storage.strips.is_empty());
    }

    /// One oversized frame does not size the pool for good: slots past the
    /// bound go, and a kept slot gives back the run — not just the spare
    /// capacity — a huge command left in it.
    #[test]
    fn the_pool_is_trimmed_back_to_its_bounds_after_a_run() {
        let mut pool = Pool::default();
        pool.slots
            .resize_with(MAX_RETAINED_SLOTS + 500, Mutex::default);
        {
            // A run four times the bound, still held: what a slot looks like
            // after the walk has taken (or declined) a huge command's strips.
            let storage = &mut pool.slots[0]
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner)
                .storage;
            storage
                .strips
                .resize(RETAINED_SLOT_STRIPS * 4, Strip::new(0, 0, 0, false));
            storage.alphas.resize(RETAINED_SLOT_ALPHAS * 4, 0);
        }
        let (slots, strips, alphas) = pool.retained();
        assert_eq!(slots, MAX_RETAINED_SLOTS + 500);
        assert!(strips >= RETAINED_SLOT_STRIPS * 4 && alphas >= RETAINED_SLOT_ALPHAS * 4);

        pool.trim();

        let (slots, strips, alphas) = pool.retained();
        assert_eq!(slots, MAX_RETAINED_SLOTS);
        assert!(strips <= RETAINED_SLOT_STRIPS, "{strips} strips kept");
        assert!(alphas <= RETAINED_SLOT_ALPHAS, "{alphas} alpha bytes kept");

        // An empty run through the pool leaves it trimmed as well.
        let scene = Scene::new();
        let plan = plan(&scene, Affine::IDENTITY, Rect::new(0.0, 0.0, 8.0, 8.0));
        pool.run(&plan, 1, Level::baseline(), (8, 8), |_| ());
        assert_eq!(pool.retained().0, MAX_RETAINED_SLOTS);
    }

    #[test]
    fn chunks_cover_every_job_once_in_order() {
        let path = BezPath::new();
        let jobs: Vec<Job<'_>> = (0..37)
            .map(|index| Job {
                command: index,
                combined: Affine::IDENTITY,
                geometry: Geometry::Fill {
                    path: &path,
                    transform: Affine::IDENTITY,
                },
                cost: 1 + (index as u64 % 5) * 100,
            })
            .collect();
        for helpers in 1..4 {
            let chunks = chunk(&jobs, helpers);
            let flat: Vec<usize> = chunks.iter().flat_map(Clone::clone).collect();
            assert_eq!(flat, (0..jobs.len()).collect::<Vec<_>>());
            assert!(chunks.iter().all(|range| !range.is_empty()));
        }
        assert!(chunk(&[], 2).is_empty());
    }
}
