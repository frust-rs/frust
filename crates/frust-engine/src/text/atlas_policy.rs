//! Which glyphs earn a texture-atlas slot, and which stay outlines.
//!
//! `glifo` ships the atlas machinery — a packer, an LRU, a key — but not a
//! policy, and says so: its own builder calls atlas caching "highly
//! experimental and not recommended for external use", and its
//! [`GlyphCacheKey`] keys the font size by the exact `f32` bit pattern
//! (`size_bits`, no quantization). Handed straight through, an animated size
//! mints a fresh entry every frame, rasterizes it once, uses it once, and pays
//! for it again 64 frames later when the LRU finally reaps it. This module is
//! the policy that closes that gap: it decides *which* glyphs are worth
//! caching, keys them so a size that moves cannot shred the cache, and hands
//! the frame one ordered atlas pass to service before it draws anything.
//!
//! Deliberately pure. It owns a [`GlyphAtlas`] — an entry map and an LRU, plain
//! bookkeeping with no device in the loop — and never touches `wgpu`, so every
//! decision below is host-testable exactly as `crate::cache::images`'s
//! residency decisions are. The pixels themselves are somebody else's problem:
//! this module says *what* to rasterize and *where* it goes, and the caller
//! rasterizes.
//!
//! # The packer is shared, not owned
//!
//! What it deliberately does *not* own is the [`ImageCache`] its slots come out
//! of. That cache is handed in at [`AtlasPolicy::new`] (for its geometry) and at
//! [`build`](AtlasPolicy::build) / [`end_frame`](AtlasPolicy::end_frame) (to
//! allocate and to evict through), and the one that reaches it is the image
//! residency's — the process's single atlas allocator.
//!
//! One allocator is a correctness requirement, not a saving. An `ImageId` is a
//! slot index into one cache, and the strip shader samples exactly one atlas
//! texture array; a second cache over the same page geometry would hand the
//! same small integers to unrelated occupants and pack them into overlapping
//! rectangles of the same layers, so glyphs and images would address each
//! other's texels by construction. `vello_hybrid` shares one `image_cache`
//! between its glyph atlas and its image path for the same reason, and its
//! glyph-side type owns no cache either.
//!
//! The borrow runs one way only: this policy allocates through the cache and
//! deallocates strictly what `glifo`'s own eviction holds in the entry map
//! below, never a handle the residency allocated.
//!
//! # A frame is two phases, not one
//!
//! A glyph cannot be drawn out of the atlas in the same pass that fills the
//! atlas: sampling a region a later draw in the same pass is still writing is a
//! read-write hazard, and the sparse-strip reference renderers avoid it by
//! separating the two. So a frame here runs:
//!
//! 1. [`begin_frame`](AtlasPolicy::begin_frame), then a walk of the scene that
//!    [`classify_run`](AtlasPolicy::classify_run)s each run and
//!    [`collect_glyph`](AtlasPolicy::collect_glyph)s each of its glyphs. Nothing
//!    is drawn and nothing is rasterized; the walk only accumulates the distinct
//!    keys this frame will need.
//! 2. [`build`](AtlasPolicy::build), which allocates a slot for every collected
//!    miss at once and returns the frame's whole [`AtlasPass`] — the regions to
//!    clear first, then the glyphs to rasterize into their new slots. The caller
//!    services that pass *before* the scene pass. Atlas uploads are the one
//!    sanctioned exception to `frust-gpu`'s single-submit borrowing contract:
//!    they may submit an encoder of their own ahead of the scene pass, which is
//!    exactly the ordering this split needs.
//!
//! Only then does the scene pass draw, resolving each glyph through
//! [`slot`](AtlasPolicy::slot). Collecting first is also what makes the
//! allocation batch coherent: a run drawn twice in a frame, or the same glyph at
//! the same subpixel bucket in two runs, is one key and therefore one
//! rasterization.
//!
//! # Clears come before uploads, one frame later — and are re-offered
//!
//! `glifo` evicts inside [`GlyphAtlas::maintain`] and queues a
//! [`PendingClearRect`] per evicted slot; a freed rectangle that is not zeroed
//! shows the dead glyph's pixels through the next glyph composited (`SrcOver`)
//! onto it. Eviction runs at the *end* of a frame, so those rects are carried
//! into the next frame's [`AtlasPass::clears`] and serviced ahead of that
//! frame's uploads — the same "evictions first, uploads second" contract
//! `crate::cache::images` states for the image atlas, and for the same reason:
//! a rectangle freed on one frame can be re-allocated on the next, and clearing
//! after uploading would erase the glyph that just moved in.
//!
//! [`build`](AtlasPolicy::build) *copies* those rects into the pass rather than
//! draining them, exactly as `crate::cache::images`' `ImageResidency::plan`
//! copies the image plan: a frame can be compiled and then refused before
//! anything reaches a queue, and a clear consumed by such a frame would leave
//! the rectangle holding a dead glyph's pixels forever. They stay pending, and
//! keep being offered on every later frame, until
//! [`acknowledge_clears`](AtlasPolicy::acknowledge_clears) says the writes were
//! really issued.
//!
//! # The size a glyph is actually keyed at is a *device* size
//!
//! `glifo` absorbs a run's uniform scale into the font size before it keys
//! anything: a run asking for 16 px under a 2x transform is prepared, keyed and
//! rasterized at 32 px, and the transform it draws through is left at unit
//! scale. So the display list's own `font_size` is not the quantity a cache
//! entry is minted against, and a guard watching it watches the wrong number —
//! a container scaling from 1x to 3x over a second holds `font_size` perfectly
//! still while minting a fresh 60-key-per-second sweep underneath.
//!
//! [`device_font_size`] restates that absorption so the policy observes what
//! `glifo` keys. It also answers `None` for a transform `glifo` will *not*
//! absorb — anything but a positive uniform scale without skew — and such a run
//! is routed to outlines outright ([`OutlineReason::TransformUncacheable`]).
//! That refusal is not an optimization: `glifo` probes the cache with a key
//! built from the unabsorbed size *before* it discovers the transform is
//! uncacheable, so a rotated or skewed run can hit an entry rasterized under a
//! different transform and draw that bitmap unrotated. Keeping the run off the
//! atlas route is what keeps that hit from being possible.
//!
//! # Glyphs cannot spend the whole allocator
//!
//! The packer is shared with the image residency, so an unbounded glyph
//! population is an image *outage*: every rectangle a glyph holds is one an
//! image cannot have, and a full atlas answers `ImageSkip::NoAtlasSpace`
//! rather than making room. `glifo`'s LRU bounds residency only over its own 64-frame
//! horizon, which a fast enough churn outruns. So the policy rations the
//! quantity the two classes actually compete for — *texels* — and stops
//! offering the atlas route once glyph residency would spend more than
//! [`glyph_texel_budget`] of them ([`OutlineReason::ResidencyFull`]). Text keeps
//! drawing; it draws as outlines until the LRU gives space back.
//!
//! The bound has to be *additive*, and that is the whole design constraint. A
//! rule comparing the population's size to a per-size-class allowance is not:
//! ration a 44 px run against "how many 44 px glyphs fit" and a screen of body
//! text locks a title out of the atlas permanently, while two size classes
//! that each stay under their own allowance together spend the share twice
//! over. So there is one running total, [`AtlasPolicy::resident_texels`] —
//! every resident entry charged at what a glyph of *its* size occupies
//! ([`glyph_texels_at`]) — and one question asked of it: does this run's own
//! demand still fit under the budget. One class's refusal then depends on the
//! others only through the texels they really hold, which is the coupling that
//! is true.
//!
//! Charged rather than measured, because `glifo` 0.3.0 will not say. Its
//! `GlyphAtlas::all_keys` and `stats` — the two entries that could price the
//! resident population from the outside, once per frame — are both behind
//! `#[cfg(all(debug_assertions, feature = "std"))]` and simply do not exist in
//! a release build. What is observable is the entry *count*, so the total is
//! maintained incrementally instead: entries appearing between two run
//! boundaries are charged at the size of the run that was admitted
//! ([`AtlasPolicy::charge_admissions`]), and an eviction pass discharges the
//! total in proportion to the entries it reaped
//! ([`AtlasPolicy::discharge_evictions`]) — which holds the *mean* charge per
//! entry across a reap, and takes the total to exactly zero when the map
//! empties, so the estimate cannot drift upward for good.
//!
//! [`glyph_texels_at`] is deliberately an over-estimate: it prices a glyph at
//! its em square plus `glifo`'s padding, where a Latin lowercase letter is
//! nearer six tenths of that wide. Over-estimating is the safe direction — the
//! images' half of the array is protected by construction, and glyph residency
//! simply stops a little short of the half it is entitled to. Under-estimating
//! would hand the images' share away silently, which is why the model is the
//! square (a CJK or full-width glyph, the widest thing routinely cached) rather
//! than the average Latin extent.
//!
//! And the bound is tested twice: once per run in the collect phase, and again
//! as each route is *consumed* ([`AtlasPolicy::admit_run`]). The second test is
//! what makes it a bound within a frame rather than only across frames —
//! `glifo` inserts nothing until a run is drawn, so every run of one frame is
//! classified from the population that frame opened with, and one screen of new
//! text would otherwise overshoot the share by its whole self.
//!
//! A run is asked about at its own *demand* rather than one entry's, for the
//! same reason. `glifo` owns the per-glyph loop once a run is admitted — it
//! keys, allocates and inserts every glyph of the run itself — so the policy
//! gets no say between the first glyph and the last, and a single run of ten
//! thousand distinct glyphs would spend the whole share inside one admission
//! that tested as fitting. [`RunKey::distinct_glyphs`] is what bounds it: the
//! count of distinct glyph ids the run carries, which is what its entries can
//! at worst grow to, charged up front. Distinct ids rather than draws, because
//! a paragraph repeating a letter two hundred times is two hundred draws and
//! one entry, and charging it its draws would refuse steady-state text that is
//! already entirely resident.
//!
//! # A page this tier cannot replay must never have been cached
//!
//! `glifo` records a cached COLR glyph as a clip bracket around a colour-layer
//! stream, into the recorder *shared by every glyph on that page*, and
//! [`GlyphAtlas::replay_pending_atlas_commands`] clears a recorder's commands
//! whether or not the caller could replay them. A tier whose replay lowers only
//! solid fills therefore loses the whole page — including the ordinary Latin
//! glyphs sharing it — while their entries stay in the map pointing at texels
//! nothing ever wrote, which is a *transparent* glyph on every later frame.
//!
//! `glifo` 0.3.0 offers no way to withdraw those entries after the fact —
//! [`GlyphAtlas`] has no per-entry removal, and its whole-map `clear` drops the
//! entries without giving their `ImageId`s back to the shared packer, which
//! leaks the allocator permanently. So the defect is closed on the near side
//! instead: a face carrying a `COLR` table is never offered the atlas route at
//! all ([`OutlineReason::ColorFont`]), and its glyphs draw through
//! `crate::text::color`'s layer recombination as they did before the atlas
//! existed. The route has to be refused per *run* rather than per glyph because
//! `glifo` derives colour-glyph caching from the presence of a cacher alone —
//! there is no way to enable it for a run's outlines and not for its COLR
//! glyphs.
//!
//! # What is not cached
//!
//! Everything in [`OutlineReason`] routes to outline strips instead, each for
//! its own reason. The one that matters most is
//! [`OutlineReason::SizeAnimating`]: a run whose device font size has moved in
//! the last [`SETTLE_FRAMES`] frames is drawn as outlines outright, because each
//! of its frames would otherwise be a fresh key, a fresh rasterization and a
//! fresh slot that nothing ever reuses. Not caching it is *cheaper* than
//! caching it, not merely safer.
//!
//! And [`OutlineReason::Disabled`] is a real fallback rather than a stub:
//! `FRUST_ENGINE_NO_ATLAS` routes every glyph in the process to outlines, which
//! is the path the engine draws text on today — correct pixels, slower, and
//! available to anyone bisecting a text-rendering defect without rebuilding.

#![allow(
    dead_code,
    reason = "the run-routing half is consumed by `super::glyph_atlas_policy`'s \
              caller; the per-glyph half — `collect_glyph`, `slot`, \
              `GlyphUpload` and `AtlasPass::uploads` — is host-tested but \
              unreachable while `glifo` owns the allocation, since it keys and \
              rasterizes every cached glyph itself (see `atlas_mut`); and the \
              readings of the budget — `entry_ceiling_at`, `max_entries`, \
              `texel_budget`, `resident_texels` — are reporting rather than \
              routing, so only the cases read them"
)]

use std::collections::{HashMap, HashSet};

use glifo::{
    AtlasConfig, AtlasSlot, FontEmbolden, GlyphAtlas, GlyphCacheConfig, GlyphCacheKey, ImageCache,
    PendingClearRect, RasterMetrics,
};
use kurbo::Affine;
use peniko::color::{AlphaColor, Srgb};

/// Grid a font size is snapped to before it reaches a cache key, in pixels.
///
/// A quarter pixel, so the worst-case size error is an eighth of a pixel — well
/// under the quarter-pixel horizontal bucket `glifo` already quantizes subpixel
/// *position* into, and therefore not the term that dominates a glyph's error.
/// What it buys is a bound: an animation sweeping 12 px to 48 px can visit at
/// most 145 distinct keys per glyph instead of one per frame forever.
///
/// A power of two on purpose. Dividing and multiplying by `0.25` are both exact
/// in binary floating point, so a size already on the grid — every integer
/// size, which is nearly all of them — quantizes to *itself*, bit for bit, and
/// the common case is not perturbed at all.
pub(crate) const SIZE_QUANTUM: f32 = 0.25;

/// Frames a font's sizes must hold still before its runs are cached again.
///
/// Eight frames is 133 ms at 60 Hz: long enough that a size still being
/// interpolated cannot slip through between two sampled frames, short enough
/// that a size which has genuinely settled — an animation that ended, a text
/// scale the user just released — starts paying atlas dividends inside the
/// following frame budget rather than at the next screen.
///
/// It is a *demotion* window, not a warm-up: a font drawn at a size nothing
/// preceded (a fresh screen, a newly loaded face) is cached on its very first
/// frame. Only an observed *change* opens the window, so the ordinary case —
/// static text — uploads on frame one and hits on frame two.
pub(crate) const SETTLE_FRAMES: u64 = 8;

/// Frames an unused entry survives before `glifo`'s LRU reaps it.
///
/// `glifo`'s own default, restated here rather than inherited so the three
/// numbers this policy runs the cache on are legible in one place.
pub(crate) const MAX_ENTRY_AGE: u64 = 64;

/// Frames between eviction passes.
///
/// Equal to [`MAX_ENTRY_AGE`], so the sweep runs no more often than an entry can
/// age out and an entry is reaped within one sweep of becoming reapable.
pub(crate) const EVICTION_FREQUENCY: u64 = 64;

/// Largest font size, in pixels, that is worth an atlas slot at all.
///
/// A glyph's bitmap grows with the square of its size, so one 256-px glyph
/// costs what sixteen 64-px ones do; past this point the outline path is both
/// cheaper and unbounded. Also `glifo`'s own default.
pub(crate) const MAX_CACHED_FONT_SIZE: f32 = 128.0;

/// Below what magnitude a transform coefficient counts as zero.
///
/// `glifo`'s own `SCALAR_NEARLY_ZERO_F64` — a twelfth of a binary order, i.e.
/// `1/4096` — restated because the trait carrying it (`AffineExt`) is
/// `pub(crate)` there and never exported. The number has to be the same one:
/// this module refuses the atlas route for exactly the transforms `glifo`
/// declines to absorb, and a stricter or looser threshold would put a band of
/// transforms on one side here and the other side there.
const NEARLY_ZERO: f64 = 1.0 / 4096.0;

/// The share of the atlas array's texels glyph residency may hold at once.
///
/// A half. The array is shared with the image residency, and an occupant that
/// may take all of it can starve the other one outright — so each class is left
/// room the other cannot spend. Half rather than a tuned split because there is
/// no per-application answer: a text-heavy screen and an image-heavy one are
/// both ordinary, and a bound exists to stop a *runaway*, not to ration a
/// steady state (a steady state never reaches it — see
/// [`glyph_texel_budget`]).
const GLYPH_ATLAS_SHARE: u64 = 2;

/// Texels one glyph slot occupies at [`TYPICAL_GLYPH_SIZE`].
///
/// 18x18: the em square at 16 px plus `glifo`'s one texel of padding on each
/// side. It is the price list's anchor, and the whole population is charged
/// against it by [`glyph_texels_at`].
///
/// Modelled as a *square* rather than as the extent a Latin letter actually
/// rasterizes to (nearer 10x16 at this size), because the estimate has to be an
/// over-estimate in the class it will really meet: a CJK or full-width glyph
/// fills its em box, and pricing every entry at the widest thing routinely
/// cached is what keeps glyph residency from quietly spending the images' half
/// of the array. Over-estimating costs glyphs a little of the share they were
/// entitled to; under-estimating costs the images theirs.
///
/// It does not need to be exact: the packer refuses an allocation it genuinely
/// has no room for regardless, and this budget exists to stop the population
/// growing to that point in the first place.
const TYPICAL_GLYPH_TEXELS: u64 = 18 * 18;

/// The device font size [`TYPICAL_GLYPH_TEXELS`] is stated at, in pixels.
///
/// The other half of that estimate, and the reason it can be scaled rather than
/// only asserted: 18x18 is what a *16 px* glyph is worth, so a glyph drawn at
/// some other device size is worth that figure scaled by the square of the
/// ratio — a glyph's bitmap grows with the square of its size, which is the
/// same relation [`MAX_CACHED_FONT_SIZE`] is argued from.
///
/// Without it the accounting counts every entry as a typical one, and an entry
/// count stops being a proxy for texels at all: at [`MAX_CACHED_FONT_SIZE`] a
/// slot is sixty-four times a typical one (eight times the size, squared), so a
/// few hundred large glyphs fill the whole shared array while the entry count is
/// still an order of magnitude below any bound — text starving images out of
/// the allocator with the guard that exists to prevent exactly that never
/// firing. See [`glyph_texels_at`].
const TYPICAL_GLYPH_SIZE: f32 = 16.0;

/// The fewest typical glyphs the budget is ever sized for.
///
/// A deliberately tiny atlas — a test constraining the packer, a downlevel
/// adapter clamped to a small texture — must still cache *something*, or the
/// budget would turn the atlas off on exactly the devices it was budgeted for.
/// The floor is deliberately allowed to exceed the array's own texel count on
/// such a device: the packer still refuses what it has no room for, and the
/// alternative is a cache that is off rather than bounded.
const MIN_GLYPH_ENTRIES: usize = 256;

/// The most typical glyphs the budget is ever sized for.
///
/// The entry map's own bookkeeping is not free, and a population past this is
/// past what any one surface's text can be reusing; a larger array should widen
/// the *images*' share, not keep growing a glyph population nothing looks up.
///
/// It bounds the entry *count* as well as the texels, without a second rule:
/// [`glyph_texels_at`] never charges less than [`TYPICAL_GLYPH_TEXELS`], so a
/// budget of `n` typical glyphs' texels cannot hold more than `n` entries
/// whatever sizes they are drawn at.
const MAX_GLYPH_ENTRIES: usize = 65_536;

/// How many texels glyph residency may hold at once in `pages`' geometry.
///
/// [`GLYPH_ATLAS_SHARE`] of the array's whole texel count, clamped to what
/// `MIN_GLYPH_ENTRIES..=MAX_GLYPH_ENTRIES` typical glyphs are worth. Derived
/// from the geometry rather than fixed, because the mobile and desktop budgets
/// differ by eight times their area and a constant sized for one would either
/// strand the other's atlas or fail to bound it.
///
/// This is the one bound the policy enforces. It is additive by construction —
/// a total against a total — which is what an entry count compared to a
/// per-size allowance is not (see this module's doc).
#[must_use]
pub(crate) fn glyph_texel_budget(pages: AtlasConfig) -> u64 {
    let (width, height) = pages.atlas_size;
    let layers = u64::try_from(pages.max_atlases).unwrap_or(u64::MAX).max(1);
    let texels = u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(layers);

    (texels / GLYPH_ATLAS_SHARE).clamp(
        MIN_GLYPH_ENTRIES as u64 * TYPICAL_GLYPH_TEXELS,
        MAX_GLYPH_ENTRIES as u64 * TYPICAL_GLYPH_TEXELS,
    )
}

/// How many *typical* glyphs `pages`' geometry admits at once.
///
/// [`glyph_texel_budget`] read as a population rather than as an area, for
/// reporting and for the cases that reason in entries. Nothing routes on it —
/// the route is decided against the texel budget itself — so it cannot drift
/// from what is enforced: it is that same number divided by the price of one
/// typical glyph, and the clamp it lands inside is the budget's own.
#[must_use]
pub(crate) fn glyph_entry_ceiling(pages: AtlasConfig) -> usize {
    let entries = glyph_texel_budget(pages) / TYPICAL_GLYPH_TEXELS;
    usize::try_from(entries)
        .unwrap_or(MAX_GLYPH_ENTRIES)
        .clamp(MIN_GLYPH_ENTRIES, MAX_GLYPH_ENTRIES)
}

/// What one resident glyph of device size `size` is charged against the budget.
///
/// [`TYPICAL_GLYPH_TEXELS`] scaled by the square of the size ratio: a glyph's
/// bitmap grows with the square of its size, so a glyph at
/// [`MAX_CACHED_FONT_SIZE`] costs sixty-four typical ones. Pricing each entry
/// at its own size is what makes the running total additive across size
/// classes — a hundred small entries and one large one cost exactly what they
/// occupy, rather than counting against a ceiling stated in somebody else's
/// units.
///
/// A size at or below [`TYPICAL_GLYPH_SIZE`] is charged the typical price
/// rather than less. The entry map's own bookkeeping is what bounds the small
/// end (see [`MAX_GLYPH_ENTRIES`]), and pricing 6 px text at a ninth of a
/// typical glyph would let the map grow nine times as large for the same
/// texels.
#[must_use]
pub(crate) fn glyph_texels_at(size: f32) -> u64 {
    debug_assert!(
        size.is_finite(),
        "a non-finite size never reaches a charge: `quantize_font_size` refuses \
         it and `absorbed_scale` refuses the transforms that could produce it"
    );
    // `f32::max` answers the operand that is not NaN, so a NaN that reached a
    // release build is priced as typical rather than poisoning the total.
    let scale = f64::from(size.max(TYPICAL_GLYPH_SIZE)) / f64::from(TYPICAL_GLYPH_SIZE);
    let texels = TYPICAL_GLYPH_TEXELS as f64 * scale * scale;
    // Finite by construction: `scale` is at least one, and `size` is a
    // quantized font size at or under `MAX_CACHED_FONT_SIZE` by the time any
    // caller charges it.
    texels as u64
}

/// How many entries of device size `size` fit in the texels `entries` typical
/// glyphs occupy.
///
/// The budget restated as a homogeneous population at one size — what a screen
/// drawn entirely at `size` could hold. It is a *reading* of the bound, not the
/// bound: nothing routes on it, because a real population is mixed and the rule
/// that decides a route is the additive total (see [`glyph_texels_at`]). An
/// earlier revision did route on it, comparing the whole population's count
/// against one size class's allowance, and that is precisely the non-additive
/// shape this file no longer has.
///
/// A size at or below [`TYPICAL_GLYPH_SIZE`] reads `entries` unchanged, on the
/// same terms [`glyph_texels_at`] charges it.
///
/// Never zero: a reading of "no glyph of this size fits at all" is the cache
/// switched off rather than bounded, so one entry is the floor here.
/// [`MIN_GLYPH_ENTRIES`] is a different floor and lives on the budget itself —
/// it says how small a *budget* may get, not how small this restatement of an
/// arbitrary `entries` may read.
#[must_use]
pub(crate) fn entry_ceiling_at(entries: usize, size: f32) -> usize {
    let cost = glyph_texels_at(size);
    let texels = u64::try_from(entries)
        .unwrap_or(u64::MAX)
        .saturating_mul(TYPICAL_GLYPH_TEXELS);
    usize::try_from(texels / cost.max(1))
        .unwrap_or(MAX_GLYPH_ENTRIES)
        .max(1)
}

/// The scale `glifo` will absorb out of `transform` into the font size, or
/// `None` when it will absorb none and so cache nothing drawn through it.
///
/// `glifo` prepares a run by *absorbing* a positive uniform scale out of the
/// transform and into the font size — the outline is fetched larger and drawn
/// through a unit transform — and it is that absorbed size, unquantized, that
/// reaches the cache key.
///
/// `None` covers every transform it leaves unabsorbed: a rotation, a skew, a
/// mirror, a non-uniform or non-positive scale, or a non-finite coefficient.
/// Those are runs it draws through the full transform and then declines to
/// cache — but only *after* probing the cache with a key built from the raw
/// font size, which can hit an entry some other frame rasterized under a unit
/// transform and paint it with the rotation and scale simply dropped. A caller
/// that refuses the atlas route on `None` never lets that probe happen.
///
/// Restates `glifo`'s `is_positive_uniform_scale_without_skew` rather than
/// calling it: the trait carrying it is private there. It is deliberately the
/// stricter of `glifo`'s two absorption predicates — the hinted path admits a
/// horizontal skew, which `glifo` then refuses at insertion anyway, again only
/// after the probe.
#[must_use]
pub(crate) fn absorbed_scale(transform: Affine) -> Option<f64> {
    let [a, b, c, d, _, _] = transform.as_coeffs();
    if !(a.is_finite() && b.is_finite() && c.is_finite() && d.is_finite()) {
        return None;
    }
    let uniform = (a - d).abs() <= NEARLY_ZERO && a > 0.0 && d > 0.0;
    let skewed = b.abs() > NEARLY_ZERO || c.abs() > NEARLY_ZERO;
    // The vertical coefficient, which is the one `glifo` multiplies the font
    // size by (`run.font_size * t_d`). Equal to the horizontal one to within
    // the tolerance above whenever that check passed.
    (uniform && !skewed).then_some(d)
}

/// The font size `glifo` will key a run of `size` drawn under `transform` at.
///
/// The number a size guard has to watch: a run at 16 px under a 2x transform is
/// a 32 px cache entry, and a transform sweeping 1x to 3x is a size animation
/// however still `size` itself holds.
///
/// `None` is a statement about the *transform* only — see [`absorbed_scale`].
/// A `size` that is not one a glyph can be rasterized at comes back as the
/// non-finite or non-positive number it is, and is
/// [`quantize_font_size`]'s to refuse; keeping the two answers apart is what
/// lets a caller say *which* of them refused a run.
#[must_use]
pub(crate) fn device_font_size(size: f32, transform: Affine) -> Option<f32> {
    absorbed_scale(transform).map(|scale| (f64::from(size) * scale) as f32)
}

/// The cache-behaviour half of the policy, as `glifo` consumes it.
#[must_use]
pub(crate) fn glyph_cache_config() -> GlyphCacheConfig {
    GlyphCacheConfig {
        max_entry_age: MAX_ENTRY_AGE,
        eviction_frequency: EVICTION_FREQUENCY,
        max_cached_font_size: MAX_CACHED_FONT_SIZE,
    }
}

/// `size` snapped to the [`SIZE_QUANTUM`] grid, or `None` when it is not a size
/// a glyph can be rasterized at.
///
/// `None` covers the whole unusable class in one answer — infinite, NaN, zero,
/// negative, and anything that rounds away to nothing — so a caller has one
/// branch to take rather than four, and a degenerate run reaches the outline
/// path instead of a key built on a non-finite `size_bits`.
#[must_use]
pub(crate) fn quantize_font_size(size: f32) -> Option<f32> {
    if !size.is_finite() || size <= 0.0 {
        return None;
    }
    let quantized = (size / SIZE_QUANTUM).round() * SIZE_QUANTUM;
    (quantized > 0.0 && quantized.is_finite()).then_some(quantized)
}

/// Why a glyph is drawn as outline strips rather than sampled from the atlas.
///
/// Every variant is a *route*, never a failure: the glyph is still drawn, by
/// the path the engine has always drawn it on. They are distinguished because
/// the remedies differ — an animating size fixes itself, an oversized one is an
/// application decision, and a disabled atlas is the operator's own doing.
///
/// One route has no variant here, because it is not decided per run: a glyph
/// the atlas had no room for is counted by [`AtlasPass::refused`] and resolves
/// to no slot, so the scene pass draws it as outlines on the same terms as an
/// uncollected one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutlineReason {
    /// `FRUST_ENGINE_NO_ATLAS` is set; the atlas is out of the frame entirely.
    Disabled,
    /// The run's device font size changed within the last [`SETTLE_FRAMES`]
    /// frames.
    SizeAnimating,
    /// The run's device size is past [`MAX_CACHED_FONT_SIZE`].
    SizeTooLarge,
    /// The run's size is not one a glyph can be rasterized at (see
    /// [`quantize_font_size`]).
    UnusableSize,
    /// The run's transform is not one `glifo` absorbs a scale out of, so no
    /// glyph of it can be cached and none may be *looked up* either (see
    /// [`device_font_size`]).
    TransformUncacheable,
    /// The run's face carries a `COLR` table, so `glifo` would cache its colour
    /// glyphs as a clip-bracketed command stream this tier cannot replay — and
    /// would take the page's ordinary glyphs down with it (see this module's
    /// doc).
    ColorFont,
    /// Admitting this run would take glyph residency past
    /// [`glyph_texel_budget`], so its entries would be taken out of the images'
    /// share of the shared allocator.
    ResidencyFull,
    /// Asked outside the frame's collect phase, so there was nothing to collect
    /// into (see this module's doc).
    NotCollecting,
}

/// A run the policy will cache, reduced to everything its glyph keys need.
///
/// Holds the *quantized* size, never the requested one: the quantization
/// happens once per run in [`AtlasPolicy::classify_run`], and carrying the
/// result forward is what makes it impossible for a key to be built from the
/// raw size by accident.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AtlasRun {
    /// The font blob's process-unique id.
    font_id: u64,
    /// Index within a font collection.
    font_index: u32,
    /// The run's font size, snapped to [`SIZE_QUANTUM`].
    size: f32,
    /// The most entries this run can add, carried through from
    /// [`RunKey::distinct_glyphs`] so [`AtlasPolicy::admit_run`] can ask the
    /// same question the collect phase asked. A *routing* quantity, not part of
    /// the identity: [`AtlasRun::key`] never reads it.
    distinct_glyphs: u32,
    /// Whether the run's outlines are hinted.
    hinted: bool,
    /// The context colour a COLR glyph resolves against.
    context_color: AlphaColor<Srgb>,
    /// `context_color` premultiplied and packed, which is the form
    /// [`GlyphCacheKey`] hashes and compares.
    context_color_packed: u32,
}

impl AtlasRun {
    /// This run's quantized font size.
    #[must_use]
    pub(crate) fn size(&self) -> f32 {
        self.size
    }

    /// The most entries drawing this run can add to the atlas.
    #[must_use]
    pub(crate) fn distinct_glyphs(&self) -> u32 {
        self.distinct_glyphs
    }

    /// The cache key for `glyph_id` drawn at horizontal fraction
    /// `fractional_x`.
    ///
    /// `fractional_x` reaches [`GlyphCacheKey::new`] raw: `glifo` owns the
    /// subpixel bucketing, including the count of buckets, which it does not
    /// expose. Quantizing it here as well would either duplicate that constant
    /// or disagree with it.
    ///
    /// No synthetic embolden and no variation coordinates, matching what
    /// `super::lower_glyph_run` states explicitly when it builds the run:
    /// the classic tier drops parley's synthesis, so a key claiming otherwise
    /// would name a glyph the engine never draws.
    #[must_use]
    pub(crate) fn key(&self, glyph_id: u32, fractional_x: f32) -> GlyphCacheKey {
        GlyphCacheKey::new(
            self.font_id,
            self.font_index,
            glyph_id,
            self.size,
            self.hinted,
            fractional_x,
            self.context_color,
            self.context_color_packed,
            FontEmbolden::default(),
            &[],
        )
    }
}

/// What one run is drawn through this frame.
#[derive(Debug, Clone, Copy)]
pub(crate) enum RunRoute {
    /// Cached: every glyph of the run is collected against this identity.
    Atlas(AtlasRun),
    /// Drawn as outline strips, for the stated reason.
    Outline(OutlineReason),
}

impl RunRoute {
    /// The run identity when this route is cached.
    #[must_use]
    pub(crate) fn atlas(&self) -> Option<&AtlasRun> {
        match self {
            Self::Atlas(run) => Some(run),
            Self::Outline(_) => None,
        }
    }

    /// Why the run is drawn as outlines, when it is.
    #[must_use]
    pub(crate) fn outline_reason(&self) -> Option<OutlineReason> {
        match self {
            Self::Atlas(_) => None,
            Self::Outline(reason) => Some(*reason),
        }
    }
}

/// What one glyph of a cached run costs this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlyphRoute {
    /// Already resident from an earlier frame: nothing to rasterize, and the
    /// entry's LRU age has been refreshed by the lookup.
    Cached,
    /// Newly claimed for this frame's [`AtlasPass`]. Resolvable through
    /// [`AtlasPolicy::slot`] once [`AtlasPolicy::build`] has run — or `None`
    /// there, if the atlas turned out to have no room, in which case the glyph
    /// falls back to outlines like any other.
    Pending,
    /// Drawn as outline strips, for the stated reason.
    Outline(OutlineReason),
}

/// One glyph the frame's atlas pass must rasterize into the slot it was given.
#[derive(Debug, Clone)]
pub(crate) struct GlyphUpload {
    /// The key the glyph was allocated for; the rasterizer reads the font,
    /// glyph id, size, hinting and subpixel bucket back off it.
    pub(crate) key: GlyphCacheKey,
    /// Where in the atlas array the rasterized bitmap belongs.
    pub(crate) slot: AtlasSlot,
}

/// Everything a frame's single atlas pass has to do, in the order it has to do
/// it.
///
/// Clears first, uploads second, always: see this module's doc.
#[derive(Debug, Default)]
pub(crate) struct AtlasPass {
    /// Regions freed by the previous frame's eviction, to be zeroed before
    /// anything is written.
    pub(crate) clears: Vec<PendingClearRect>,
    /// Glyphs allocated this frame, in the order they were collected.
    pub(crate) uploads: Vec<GlyphUpload>,
    /// Collected glyphs the atlas had no room for. They were routed
    /// [`GlyphRoute::Pending`] during the walk and resolve to no slot, so the
    /// scene pass draws them as outlines.
    pub(crate) refused: u32,
}

impl AtlasPass {
    /// Whether this pass has nothing to do, and so can be skipped whole.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.clears.is_empty() && self.uploads.is_empty()
    }
}

/// The run identity a caller hands [`AtlasPolicy::classify_run`], before
/// quantization.
///
/// Three of its fields are *routing* inputs rather than parts of the identity —
/// [`transform`](Self::transform), [`color_font`](Self::color_font) and
/// [`distinct_glyphs`](Self::distinct_glyphs). They live here because the route
/// is decided in one call from one value, and none of them reaches a cache key:
/// an [`AtlasRun`] carries only what [`AtlasRun::key`] hashes, plus the demand
/// [`AtlasPolicy::admit_run`] has to re-ask with.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RunKey {
    /// The font blob's process-unique id (`peniko::Blob::id`).
    pub(crate) font_id: u64,
    /// Index within a font collection.
    pub(crate) font_index: u32,
    /// The run's font size as the display list stated it.
    pub(crate) font_size: f32,
    /// How many *distinct* glyph ids the run carries — the most entries
    /// admitting it can add, and so what its admission is charged against the
    /// budget (see this module's doc).
    ///
    /// Distinct ids rather than glyphs drawn: a paragraph repeating a letter
    /// two hundred times is two hundred draws and one entry, and charging it
    /// its draws would refuse steady-state text that is already resident. It is
    /// still an over-estimate — a glyph landing in two subpixel buckets is two
    /// entries, and one already resident is none — but it is an over-estimate
    /// of the right quantity, and the only one available before `glifo` has
    /// keyed anything.
    ///
    /// A *routing* input, like [`transform`](Self::transform) and
    /// [`color_font`](Self::color_font): it reaches no cache key.
    pub(crate) distinct_glyphs: u32,
    /// The run's own transform composed with the frame root — the full
    /// device-space mapping `glifo` absorbs a scale out of (see
    /// [`device_font_size`]).
    pub(crate) transform: Affine,
    /// Whether the run's outlines are hinted.
    pub(crate) hinted: bool,
    /// Whether the run's face carries a `COLR` table, and so could produce a
    /// colour glyph this tier's replay cannot lower.
    pub(crate) color_font: bool,
    /// The context colour a COLR glyph resolves against.
    pub(crate) context_color: AlphaColor<Srgb>,
}

impl RunKey {
    /// The identity a display-list run is cached under.
    ///
    /// The font identity is the blob's own process-unique id and its collection
    /// index — the same pair `glifo` keys its caches on, read off the same
    /// `peniko::FontData` the run already carries. There is no font registry in
    /// between and no identity of the engine's own invention, which is what
    /// makes a key built here and a key built inside `glifo` the same key.
    ///
    /// `hinted` is passed rather than assumed: it is the same device-class
    /// choice `super::lower_glyph_run` hands `glifo` — on for a desktop-class
    /// adapter, off for a mobile one — and a hinted glyph and an unhinted one
    /// are different bitmaps, so neither may reuse the other's entry. Passing
    /// it also keeps this key honest about the half of the hinting policy
    /// `glifo` decides for itself: a run whose transform it refuses to hint is
    /// keyed as the caller asked, which is conservative in the safe direction
    /// (a redundant entry) rather than the unsafe one.
    ///
    /// `transform` is the run's *device* transform — its own composed with the
    /// frame root, the same value the lowering hands `glifo` — not the display
    /// list's `run.transform` alone. Passing the composed one is the whole point:
    /// the scale the guard has to watch is usually the root's, not the run's.
    ///
    /// `color_font` is passed rather than derived because answering it means
    /// reading the face's table directory, which is `super`'s job (see
    /// `crate::text::font_has_color_glyphs`) and would tie this module to a
    /// font parser it otherwise has no need of.
    ///
    /// `seen` is scratch for the distinct-glyph count — a caller-owned set,
    /// cleared here and reused across the runs of a walk, so counting a
    /// ten-thousand-glyph run costs no allocation of its own.
    #[must_use]
    pub(crate) fn for_run(
        run: &frust_scene::GlyphRun,
        transform: Affine,
        hinted: bool,
        color_font: bool,
        context_color: AlphaColor<Srgb>,
        seen: &mut HashSet<u32>,
    ) -> Self {
        let font = run.font.font();
        seen.clear();
        seen.extend(run.glyphs.iter().map(|glyph| glyph.id));
        Self {
            font_id: font.data.id(),
            font_index: font.index,
            font_size: run.font_size,
            distinct_glyphs: u32::try_from(seen.len()).unwrap_or(u32::MAX),
            transform,
            hinted,
            color_font,
            context_color,
        }
    }
}

/// Which font a size observation belongs to.
type FontKey = (u64, u32);

/// One font's recently drawn sizes, and whether they are holding still.
#[derive(Debug, Default)]
struct FontSizes {
    /// The last frame on which this font counts as animating; a change pushes
    /// it [`SETTLE_FRAMES`] frames out.
    animating_until: u64,
    /// Last frame each recently drawn quantized size was seen, keyed by the
    /// size's `f32` bit pattern. Pruned every frame, so its size is bounded by
    /// how many distinct sizes the last [`SETTLE_FRAMES`] frames actually used.
    sizes: HashMap<u32, u64>,
}

/// Which phase of the two-phase frame the policy is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Between frames.
    Idle,
    /// Walking the scene, accumulating keys.
    Collect,
    /// Slots allocated; the scene pass may resolve them.
    Draw,
}

/// The engine's glyph-atlas policy: one per renderer, retained across frames.
///
/// Owns the entry map it decides for — a [`GlyphAtlas`] — so "which glyphs are
/// resident" and "which glyphs *should* be resident" can never drift apart into
/// two structures that disagree. The packer those entries live in is shared
/// rather than owned; see this module's doc.
#[derive(Debug)]
pub(crate) struct AtlasPolicy {
    /// `glifo`'s entry map and LRU.
    atlas: GlyphAtlas,
    /// The shared packer's page geometry, read off it once at construction and
    /// kept for reporting. Fixed for a cache's lifetime, so the copy cannot
    /// drift from the allocator it was taken from.
    pages: AtlasConfig,
    /// How many texels glyph residency may hold before the atlas route stops
    /// being offered — [`glyph_texel_budget`] over [`Self::pages`]. The bound
    /// everything routes on.
    texel_budget: u64,
    /// The same budget read as a population of typical glyphs
    /// ([`glyph_entry_ceiling`]), for reporting only.
    max_entries: usize,
    /// What the resident population is estimated to occupy, in texels: every
    /// entry charged at [`glyph_texels_at`] of the size it was admitted under.
    ///
    /// Maintained incrementally, because `glifo` 0.3.0 will not price the
    /// population from outside — see this module's doc. Charged by
    /// [`charge_admissions`](Self::charge_admissions), discharged by
    /// [`discharge_evictions`](Self::discharge_evictions), and exactly zero
    /// whenever the entry map is empty.
    resident_texels: u64,
    /// How many entries [`resident_texels`](Self::resident_texels) has been
    /// charged for, so an entry-count change can be read as a delta.
    charged_entries: usize,
    /// The device size the next admission delta is charged at: the size of the
    /// last run [`admit_run`](Self::admit_run) let through, since `glifo`
    /// inserts a run's glyphs as it draws them and every entry appearing before
    /// the next run's boundary belongs to that run.
    admitting_size: f32,
    /// Whether `FRUST_ENGINE_NO_ATLAS` took the atlas out of the frame.
    disabled: bool,
    /// Frames begun, which is what the size tracker ages against. Distinct from
    /// `glifo`'s own LRU serial, which only advances inside `maintain`.
    frame: u64,
    /// Per-font size history driving [`OutlineReason::SizeAnimating`].
    fonts: HashMap<FontKey, FontSizes>,
    /// This frame's collected misses, in collection order.
    pending: Vec<GlyphCacheKey>,
    /// The same set, for deduplicating a glyph collected twice in one frame.
    claimed: HashSet<GlyphCacheKey>,
    /// Clear rects drained after an earlier frame's eviction, awaiting a pass
    /// that actually writes them.
    ///
    /// Offered by every [`build`](Self::build) and cleared only by
    /// [`acknowledge_clears`](Self::acknowledge_clears), so a compiled frame
    /// that is refused before it reaches a queue does not consume them (see
    /// this module's doc).
    clears: Vec<PendingClearRect>,
    /// How many of [`clears`](Self::clears) the last [`build`](Self::build)
    /// offered.
    ///
    /// The queue is only ever appended to, so the offered rects are a prefix of
    /// it and [`acknowledge_clears`](Self::acknowledge_clears) can drop exactly
    /// them. Acknowledging the whole queue instead would silently swallow the
    /// rectangles the *same* frame's eviction queued after the offer was made —
    /// which no caller has written, because they did not exist when it wrote.
    offered_clears: usize,
    /// `glifo`'s cumulative miss count as this frame opened, so the frame's own
    /// misses can be read as a difference.
    misses_at_frame_start: u64,
    /// The LRU serial at which the oldest still-unreplayed page recording was
    /// made, or `None` when nothing is outstanding.
    ///
    /// Two questions in one field, because they have one answer. It is
    /// [`replay_pending`](Self::replay_pending) — set by
    /// [`end_frame`](Self::end_frame) on any frame that missed, cleared by
    /// [`acknowledge_replay`](Self::acknowledge_replay) — and it is what
    /// [`eviction_deferred`](Self::eviction_deferred) measures the deferral
    /// window from.
    ///
    /// The caller drives its replay pass from the `is_some` half: driving it
    /// from *surviving draws* instead misses a run whose coverage was entirely
    /// culled, which still inserted entries and still dirtied a page.
    unreplayed_since: Option<u64>,
    /// `glifo`'s LRU serial, mirrored.
    ///
    /// `GlyphAtlas` does not expose it, and `maintain` ticks it exactly once per
    /// call, so counting the calls this policy makes reproduces it — the policy
    /// is the only thing that calls `maintain` on the map it owns. Needed
    /// because the eviction deferral is a statement about *ages*, and an age is
    /// a difference of serials rather than of frames: a deferred frame does not
    /// tick.
    serial: u64,
    /// Where in the two-phase frame this policy is.
    phase: Phase,
}

impl AtlasPolicy {
    /// A policy packing into `images`, disabled outright when `disabled`.
    ///
    /// `images` is the shared allocator every later call must be handed — see
    /// `super::glyph_atlas_policy`, which takes it from the image residency.
    /// The page geometry is read off it here rather than passed separately,
    /// which is what makes it impossible for the policy to report one geometry
    /// while allocating into another.
    ///
    /// Nothing is allocated here, and nothing is even borrowed onward: the
    /// cache is read and released. With `initial_atlas_count` at zero the first
    /// page is created by the first glyph or image that needs one, so an
    /// application drawing neither pays for no atlas.
    #[must_use]
    pub(crate) fn new(images: &ImageCache, disabled: bool) -> Self {
        let pages = *images.atlas_manager().config();
        Self {
            atlas: GlyphAtlas::with_config(glyph_cache_config()),
            pages,
            texel_budget: glyph_texel_budget(pages),
            max_entries: glyph_entry_ceiling(pages),
            resident_texels: 0,
            charged_entries: 0,
            admitting_size: TYPICAL_GLYPH_SIZE,
            disabled,
            frame: 0,
            fonts: HashMap::new(),
            pending: Vec::new(),
            claimed: HashSet::new(),
            clears: Vec::new(),
            offered_clears: 0,
            misses_at_frame_start: 0,
            unreplayed_since: None,
            serial: 0,
            phase: Phase::Idle,
        }
    }

    /// Whether any glyph can be cached at all.
    #[must_use]
    pub(crate) fn is_enabled(&self) -> bool {
        !self.disabled
    }

    /// The page geometry this policy packs into.
    ///
    /// The shared allocator's own, taken at construction. How many of those
    /// pages exist is *not* answerable here and deliberately so: pages are
    /// created by whichever class needs one, so the count belongs to the cache
    /// (`ImageCache::atlas_count`) rather than to either occupant of it.
    #[must_use]
    pub(crate) fn pages(&self) -> AtlasConfig {
        self.pages
    }

    /// Frames begun since construction.
    #[must_use]
    pub(crate) fn frame(&self) -> u64 {
        self.frame
    }

    /// The entry map itself, for the caller that hands it to `glifo`.
    ///
    /// `glifo` does not take a *decision* from an integrator — it takes the
    /// cache: [`AtlasCacher::Enabled`](glifo::AtlasCacher::Enabled) borrows a
    /// [`GlyphAtlas`] and the shared allocator, and from there `glifo` builds
    /// every key, resolves every hit, allocates every miss and records the
    /// fills that rasterize it. So a run this policy routes to the atlas
    /// reaches `glifo` by handing over this map, and the frame's dirty pages
    /// are drained back off it afterwards.
    ///
    /// Handed out rather than mirrored, for the reason this type's own doc
    /// gives: "which glyphs are resident" has to stay one structure. What the
    /// policy still decides on its own is *which runs are offered* — the kill
    /// switch, the size quantization, the animation guard and the size
    /// ceiling — and that decision is made before this borrow is taken.
    pub(crate) fn atlas_mut(&mut self) -> &mut GlyphAtlas {
        &mut self.atlas
    }

    /// How many glyphs are currently resident.
    #[must_use]
    pub(crate) fn entry_count(&self) -> usize {
        self.atlas.len()
    }

    /// How many (font, size) pairs the animation tracker is holding.
    ///
    /// The tracker is the other structure a size sweep could grow without
    /// bound, so it is reported alongside [`entry_count`](Self::entry_count)
    /// rather than trusted to prune itself unobserved.
    #[must_use]
    pub(crate) fn tracked_sizes(&self) -> usize {
        self.fonts.values().map(|font| font.sizes.len()).sum()
    }

    /// How many *typical* glyphs the budget is worth, for reporting.
    ///
    /// Not a bound anything is tested against — that is
    /// [`texel_budget`](Self::texel_budget). A population of larger glyphs
    /// reaches the budget with fewer entries than this, and one of smaller ones
    /// cannot exceed it (see [`MAX_GLYPH_ENTRIES`]).
    #[must_use]
    pub(crate) fn max_entries(&self) -> usize {
        self.max_entries
    }

    /// The texels glyph residency may hold at once — the bound routes are
    /// decided against.
    #[must_use]
    pub(crate) fn texel_budget(&self) -> u64 {
        self.texel_budget
    }

    /// What the resident population is estimated to occupy, in texels.
    ///
    /// The running total the budget is spent against; see this type's field doc
    /// for how it is maintained and why it is charged rather than measured.
    #[must_use]
    pub(crate) fn resident_texels(&self) -> u64 {
        self.resident_texels
    }

    /// Whether page commands are still waiting to be replayed into the atlas.
    ///
    /// The gate a caller drives its render-to-atlas pass from. Deliberately not
    /// "did this frame draw an atlas glyph": `glifo` dirties a page at
    /// *insertion*, so a run whose every draw was culled away still recorded
    /// fills that have to reach the array, and a caller gating on surviving
    /// draws leaves them recorded until something else happens to replay them
    /// — by which time the slots they name may belong to other glyphs.
    #[must_use]
    pub(crate) fn replay_pending(&self) -> bool {
        self.unreplayed_since.is_some()
    }

    /// Record that the recorded page commands reached the atlas array.
    ///
    /// The counterpart of [`replay_pending`](Self::replay_pending), and the
    /// same shape as the image residency's `acknowledge_plan`: call it only
    /// once the replay really ran, because it also closes the eviction
    /// deferral's window (see
    /// [`eviction_deferred`](Self::eviction_deferred)).
    pub(crate) fn acknowledge_replay(&mut self) {
        self.unreplayed_since = None;
    }

    /// Whether any evicted rectangle is still waiting to be zeroed.
    #[must_use]
    pub(crate) fn has_pending_clears(&self) -> bool {
        !self.clears.is_empty()
    }

    /// Record that the rects the last [`build`](Self::build) offered were
    /// written to the array, dropping exactly those.
    ///
    /// Call it only once the writes were really issued: a frame that consumed
    /// them and was then refused would leave every one of those rectangles
    /// holding the pixels of the glyph that was evicted out of it.
    ///
    /// Only the offered prefix is dropped. The same frame's own
    /// [`end_frame`](Self::end_frame) may have queued more rects behind them —
    /// this frame's eviction, which belongs to the *next* pass — and those have
    /// been written by nobody.
    pub(crate) fn acknowledge_clears(&mut self) {
        let offered = self.offered_clears.min(self.clears.len());
        self.clears.drain(..offered);
        self.offered_clears = 0;
    }

    /// Open the frame's collect phase.
    pub(crate) fn begin_frame(&mut self) {
        self.frame = self.frame.saturating_add(1);
        self.pending.clear();
        self.claimed.clear();
        // Read before anything this frame can miss, so `frame_misses` below is
        // this frame's own count rather than the process's.
        self.misses_at_frame_start = self.atlas.cache_misses();
        self.phase = Phase::Collect;
    }

    /// How many glyphs have missed the cache since this frame opened.
    ///
    /// The proxy for "this frame left work in `glifo`'s pending queues": every
    /// path that records a page command, queues a bitmap upload or allocates a
    /// slot goes through a miss first. An over-approximation by construction —
    /// a miss whose transform `glifo` then declines to cache records nothing —
    /// and deliberately so, since the cost of the extra answer is a replay call
    /// that finds nothing to do, while the cost of a missed one is stale ink.
    #[must_use]
    fn frame_misses(&self) -> u64 {
        self.atlas
            .cache_misses()
            .saturating_sub(self.misses_at_frame_start)
    }

    /// Decide how `run` is drawn, and record its size against the font's
    /// history.
    ///
    /// The size observation happens whatever the answer — including for a run
    /// refused as [`OutlineReason::SizeTooLarge`] — because a sweep that passes
    /// through the cacheable range is still a size change while it is above it,
    /// and forgetting that would let an animation slowing down above 128 px be
    /// treated as settled the moment it dropped back under.
    ///
    /// The size observed is the *device* one — `font_size` with the transform's
    /// absorbed scale in it, which is what `glifo` keys (see
    /// [`device_font_size`]). A transform it will not absorb is refused before
    /// any observation is made: the raw size such a run would be keyed at is a
    /// different quantity from the one this tracker follows, and mixing the two
    /// into one history would read a scale change as a settled size.
    pub(crate) fn classify_run(&mut self, run: &RunKey) -> RunRoute {
        if self.disabled {
            return RunRoute::Outline(OutlineReason::Disabled);
        }
        if self.phase != Phase::Collect {
            return RunRoute::Outline(OutlineReason::NotCollecting);
        }
        // Ahead of every other answer, and ahead of the size observation: a
        // colour face is refused whatever it is drawn at, so recording its
        // sizes would only be tracking a font that can never take the route.
        if run.color_font {
            return RunRoute::Outline(OutlineReason::ColorFont);
        }
        let Some(device_size) = device_font_size(run.font_size, run.transform) else {
            return RunRoute::Outline(OutlineReason::TransformUncacheable);
        };
        let Some(size) = quantize_font_size(device_size) else {
            return RunRoute::Outline(OutlineReason::UnusableSize);
        };

        let animating = self.observe_size((run.font_id, run.font_index), size);
        // Compared against the device size rather than the quantized one, on
        // the same terms `glifo` compares its own ceiling against the absorbed
        // size: a run just over the line must not be admitted by rounding down
        // to it.
        if device_size > MAX_CACHED_FONT_SIZE {
            return RunRoute::Outline(OutlineReason::SizeTooLarge);
        }
        if animating {
            return RunRoute::Outline(OutlineReason::SizeAnimating);
        }
        // Last, because it is the only answer that depends on what other runs
        // have already been admitted rather than on this one alone — and the
        // only one this walk cannot answer conclusively, since nothing is
        // inserted until the run is drawn. [`admit_run`](Self::admit_run) asks
        // it again against the population as it really stands.
        if self.would_overspend(size, run.distinct_glyphs) {
            return RunRoute::Outline(OutlineReason::ResidencyFull);
        }

        RunRoute::Atlas(AtlasRun {
            font_id: run.font_id,
            font_index: run.font_index,
            size,
            distinct_glyphs: run.distinct_glyphs,
            hinted: run.hinted,
            context_color: run.context_color,
            context_color_packed: pack_context_color(run.context_color),
        })
    }

    /// Re-test a route decided in the collect phase against residency as it
    /// stands *now*, narrowing it to outlines if the budget has been spent
    /// since.
    ///
    /// The collect walk classifies every run of a frame before any of them is
    /// drawn, and `glifo` inserts nothing until a run is drawn — so every run
    /// of one frame is answered from the population the frame *opened* with. A
    /// frame beginning one run below the budget would therefore admit all of
    /// its runs, and a frame is not a bounded amount of text: one screen of new
    /// glyphs can overshoot the share by as much as it likes, which is the
    /// outage the bound exists to prevent.
    ///
    /// Called as each run's route is consumed by the draw walk, where the
    /// population is the real one — every earlier run of the same frame has
    /// already inserted whatever it inserted, and this call charges those
    /// insertions before it answers. The run's own demand is its distinct glyph
    /// count, not its draws, so a paragraph repeating a letter is charged the
    /// one entry it will really claim (see [`RunKey::distinct_glyphs`]).
    ///
    /// Narrowing only. `Atlas` never comes back out of this, because a route
    /// widened after the fact would name a slot the collect phase never
    /// reserved; outlines are correct pixels on the path the engine has always
    /// used.
    ///
    /// Takes `&mut self` for the charging, which is also what makes it the
    /// single point where the last admitted run's size is latched: `glifo`
    /// inserts a run's glyphs as it draws them, so every entry appearing
    /// between two of these calls belongs to the run the previous one let
    /// through.
    #[must_use]
    pub(crate) fn admit_run(&mut self, route: RunRoute) -> RunRoute {
        self.charge_admissions();
        match route {
            RunRoute::Atlas(run) if self.would_overspend(run.size(), run.distinct_glyphs()) => {
                RunRoute::Outline(OutlineReason::ResidencyFull)
            }
            RunRoute::Atlas(run) => {
                self.admitting_size = run.size();
                RunRoute::Atlas(run)
            }
            route => route,
        }
    }

    /// Whether admitting `entries` further glyphs of device size `size` would
    /// take glyph residency past its share of the array.
    ///
    /// The one place the bound is spelled, asked by both
    /// [`classify_run`](Self::classify_run) and [`admit_run`](Self::admit_run)
    /// so the collect answer and the draw answer cannot be two different rules.
    ///
    /// A total against a total: what is already charged plus what this run can
    /// add, against [`texel_budget`](Self::texel_budget). That is what makes it
    /// additive — every size class spends the same pool in the same units, so
    /// one class's refusal turns on the texels the others really hold and never
    /// on their *count*, and no combination of classes can each stay under an
    /// allowance of its own while together spending the share twice.
    #[must_use]
    fn would_overspend(&self, size: f32, entries: u32) -> bool {
        // At least one entry: a run with no glyphs still asks for the route,
        // and charging it nothing would let an unbounded number of them
        // through at a full budget.
        let demand = glyph_texels_at(size).saturating_mul(u64::from(entries.max(1)));
        self.resident_texels.saturating_add(demand) > self.texel_budget
    }

    /// Charge whatever `glifo` has inserted since the last time the accounting
    /// looked.
    ///
    /// The admission half of the running total. `GlyphAtlas` reports its entry
    /// *count* and nothing else in a release build (see this module's doc), so
    /// the delta since the last charge is what there is to work with, priced at
    /// the size of the run that was admitted to produce it.
    fn charge_admissions(&mut self) {
        let live = self.atlas.len();
        let Some(added) = live.checked_sub(self.charged_entries) else {
            // Fewer entries than were charged: an eviction the accounting has
            // not been told about. `discharge_evictions` is what reconciles it,
            // in the same proportional terms, rather than this one guessing.
            self.discharge_evictions();
            return;
        };
        if added == 0 {
            return;
        }
        let cost = glyph_texels_at(self.admitting_size);
        let added = u64::try_from(added).unwrap_or(u64::MAX);
        self.resident_texels = self
            .resident_texels
            .saturating_add(cost.saturating_mul(added));
        self.charged_entries = live;
    }

    /// Give back what an eviction pass reaped, in proportion to the entries it
    /// took.
    ///
    /// The discharge half, and the reason the estimate cannot ratchet upward.
    /// `glifo`'s eviction reports the padded *rectangle* of each entry it
    /// freed, which is a true texel count but not one in this accounting's
    /// units — the charges were the model's price, not the rasterizer's extent,
    /// and subtracting one from the other would leave a residue per entry that
    /// never comes back. Scaling by the surviving fraction keeps the *mean*
    /// charge per entry unchanged across a reap, which is exact for a
    /// homogeneous population, unbiased for a mixed one, and — the property
    /// that matters — takes the total to precisely zero when the last entry
    /// goes.
    fn discharge_evictions(&mut self) {
        let live = self.atlas.len();
        if live >= self.charged_entries {
            return;
        }
        self.resident_texels = if self.charged_entries == 0 {
            0
        } else {
            let scaled =
                u128::from(self.resident_texels) * live as u128 / self.charged_entries as u128;
            u64::try_from(scaled).unwrap_or(u64::MAX)
        };
        self.charged_entries = live;
    }

    /// Record one glyph of a cached run, and say what it costs.
    ///
    /// A hit refreshes the entry's LRU age, which is what keeps a glyph drawn
    /// on every frame from ever being reaped. A miss is claimed once per frame
    /// however many times the glyph appears, so a paragraph repeating a letter
    /// two hundred times rasterizes it once.
    pub(crate) fn collect_glyph(
        &mut self,
        run: &AtlasRun,
        glyph_id: u32,
        fractional_x: f32,
    ) -> GlyphRoute {
        if self.phase != Phase::Collect {
            return GlyphRoute::Outline(OutlineReason::NotCollecting);
        }

        let key = run.key(glyph_id, fractional_x);
        if self.atlas.get(&key).is_some() {
            return GlyphRoute::Cached;
        }
        if self.claimed.insert(key.clone()) {
            self.pending.push(key);
        }
        GlyphRoute::Pending
    }

    /// Close the collect phase: allocate every collected miss and hand back the
    /// frame's whole atlas pass.
    ///
    /// `images` is the shared allocator — the same one [`new`](Self::new) was
    /// built from. Every slot this frame claims is taken out of it, so a glyph
    /// competes for space with the resident images rather than with a private
    /// copy of the same budget, and a full atlas refuses both classes alike.
    ///
    /// `raster` measures one glyph — it is called once per newly collected key
    /// and answers the bitmap extent and bearings the packer needs, or `None`
    /// for a glyph with nothing to rasterize (a space, an outline that produced
    /// no coverage). Measuring is separated from *rendering* deliberately: the
    /// caller renders into the returned slots afterwards, in one pass, which is
    /// the whole point of collecting first.
    pub(crate) fn build<F>(&mut self, images: &mut ImageCache, mut raster: F) -> AtlasPass
    where
        F: FnMut(&GlyphCacheKey) -> Option<RasterMetrics>,
    {
        // Cloned, not taken: the pass this returns may belong to a frame the
        // caller refuses before anything reaches a queue, and a clear consumed
        // by such a frame is a rectangle that keeps a dead glyph's pixels for
        // good. They go on being offered until `acknowledge_clears`.
        self.offered_clears = self.clears.len();
        let mut pass = AtlasPass {
            clears: self.clears.clone(),
            uploads: Vec::new(),
            refused: 0,
        };

        // Taken out of `self` so the loop can hold the atlas mutably while
        // draining; the emptied allocation goes back at the end for reuse.
        let mut pending = std::mem::take(&mut self.pending);
        for key in pending.drain(..) {
            let Some(metrics) = raster(&key) else {
                pass.refused = pass.refused.saturating_add(1);
                continue;
            };
            match self.atlas.insert_entry(images, key.clone(), metrics) {
                Some(slot) => {
                    // Charged here rather than left to `charge_admissions`'
                    // delta: this path knows each key's own size, so it prices
                    // a mixed batch entry by entry instead of at whichever run
                    // was last admitted.
                    let size = f32::from_bits(key.size_bits);
                    self.resident_texels =
                        self.resident_texels.saturating_add(glyph_texels_at(size));
                    self.charged_entries = self.charged_entries.saturating_add(1);
                    pass.uploads.push(GlyphUpload { key, slot });
                }
                None => pass.refused = pass.refused.saturating_add(1),
            }
        }
        self.pending = pending;
        self.claimed.clear();
        self.phase = Phase::Draw;

        pass
    }

    /// Where `key`'s glyph lives, or `None` when it is not resident and the
    /// caller must draw it as outlines.
    ///
    /// Answers only during the draw phase: before [`build`](Self::build) has
    /// run, a key collected this frame has no slot yet, and answering `None`
    /// there would be indistinguishable from "no room", which is a wrong
    /// fallback rather than a slow one.
    pub(crate) fn slot(&mut self, key: &GlyphCacheKey) -> Option<AtlasSlot> {
        if self.phase != Phase::Draw {
            return None;
        }
        self.atlas.get(key)
    }

    /// Close the frame: age the cache, take the eviction's clear rects, and
    /// prune the size tracker.
    ///
    /// The clear rects are held rather than returned because they belong to the
    /// *next* frame's pass — see this module's doc. Draining them here, right
    /// after `maintain`, is what makes that carry-over unconditional: they
    /// cannot be left in `glifo`'s queue to be rediscovered several frames late,
    /// by which point the freed rectangle may already hold a different glyph.
    ///
    /// `images` is the shared allocator again: eviction gives glyph rectangles
    /// back to the same packer the images draw from, so text that leaves the
    /// screen returns its space to whichever class asks for it next. Only
    /// handles from the entry map below are freed — a resident image's handle
    /// is not `glifo`'s to reach.
    ///
    /// The ageing pass is *deferred* while an unreplayed page command could
    /// name a rectangle the pass would free — see
    /// [`eviction_deferred`](Self::eviction_deferred) for exactly when that is.
    /// Eviction hands an entry's rectangle back to the packer and queues it for
    /// clearing, so a recorded command still naming it would be replayed into
    /// whatever was let that rectangle next — old ink in another glyph's slot.
    /// Deferring costs a delayed reap; not deferring costs a wrong pixel.
    pub(crate) fn end_frame(&mut self, images: &mut ImageCache) {
        // The last run of the frame inserted after its own `admit_run`, so the
        // total is one run behind until this settles it.
        self.charge_admissions();

        if self.frame_misses() > 0 && self.unreplayed_since.is_none() {
            // This frame's own recording opens the window: the replay pass runs
            // *after* compiling — `end_frame` closes the compile and the caller
            // replays only once the frame reaches the encoder (see
            // `EngineRenderer::encode`) — so what was just recorded is
            // unreplayed at this instant by construction.
            self.unreplayed_since = Some(self.serial);
        }

        if self.eviction_deferred() {
            self.prune_sizes();
            self.phase = Phase::Idle;
            return;
        }

        self.atlas.maintain(images);
        // `maintain` ticks the LRU serial exactly once per call, and this is its
        // only caller for this map (see [`serial`](Self::serial)).
        self.serial = self.serial.saturating_add(1);
        self.discharge_evictions();
        let evicted: Vec<PendingClearRect> = self.atlas.drain_pending_clear_rects().collect();
        self.clears.extend(evicted);
        self.prune_sizes();
        self.phase = Phase::Idle;
    }

    /// Whether `glifo`'s ageing pass has to wait for a replay.
    ///
    /// Deferring on *any* outstanding replay is the safe answer and was the
    /// first one, but it is far wider than the hazard: under sustained churn
    /// every frame misses, so every frame defers, and the LRU never ages at all
    /// — a cache that only ever grows until the budget closes the route.
    ///
    /// The hazard is narrower than that, and its bound is arithmetic. `glifo`
    /// records page commands in exactly one place — `GlyphAtlas::insert`, which
    /// creates the entry in the same call, stamped with the current LRU serial
    /// — so every unreplayed command belongs to an entry whose serial is at
    /// least the serial the recording was made at. Eviction reaps an entry only
    /// once `serial - entry.serial` exceeds [`MAX_ENTRY_AGE`], and a hit only
    /// pushes an entry's serial *up*. So while the oldest unreplayed recording
    /// is no more than [`MAX_ENTRY_AGE`] serials old, no entry a recording names
    /// is old enough to be reaped, and ageing is safe to run.
    ///
    /// Since `glifo`'s `maintain` advances the serial *before* evaluating ages
    /// for eviction, the check must account for the post-tick serial: defer
    /// whenever `(serial + 1) - since` would exceed [`MAX_ENTRY_AGE`], i.e.,
    /// defer when `serial - since >= MAX_ENTRY_AGE`, proceeding only while
    /// `serial - since < MAX_ENTRY_AGE`.
    ///
    /// Past that window the deferral bites exactly as before, and holds: a
    /// deferred frame does not call `maintain`, so the serial does not advance
    /// and the window cannot widen its way back out. Only
    /// [`acknowledge_replay`](Self::acknowledge_replay) reopens it.
    #[must_use]
    fn eviction_deferred(&self) -> bool {
        self.unreplayed_since
            .is_some_and(|since| self.serial.saturating_sub(since) >= MAX_ENTRY_AGE)
    }

    /// Record `size` against `font` and answer whether that font is currently
    /// animating.
    ///
    /// A size the font is not already tracked at, while it *is* tracked at some
    /// other one, is the change signal: the font is being drawn at a size it was
    /// not being drawn at recently. A font tracked at nothing at all is a first
    /// appearance, not a change, which is why static text caches on frame one.
    fn observe_size(&mut self, font: FontKey, size: f32) -> bool {
        let frame = self.frame;
        let bits = size.to_bits();
        let entry = self.fonts.entry(font).or_default();

        if !entry.sizes.contains_key(&bits) && !entry.sizes.is_empty() {
            entry.animating_until = frame.saturating_add(SETTLE_FRAMES);
        }
        entry.sizes.insert(bits, frame);

        frame <= entry.animating_until
    }

    /// Drop size records older than the settle window, and fonts left with
    /// nothing to say.
    ///
    /// Without this the tracker is the leak the entry cache is not: a sweep
    /// through 145 quantized sizes would keep all 145 forever, and a long
    /// session would keep every size any screen ever used. The window is
    /// [`SETTLE_FRAMES`] because that is exactly how far back "changed
    /// recently" reaches — a size older than that can no longer affect any
    /// answer.
    fn prune_sizes(&mut self) {
        let frame = self.frame;
        self.fonts.retain(|_, font| {
            font.sizes
                .retain(|_, last_seen| frame.saturating_sub(*last_seen) <= SETTLE_FRAMES);
            !font.sizes.is_empty() || frame <= font.animating_until
        });
    }
}

/// `color` premultiplied and packed into the `u32` [`GlyphCacheKey`] hashes and
/// compares.
///
/// `glifo` packs it the same way inside its own key construction but does not
/// export the helper, so the packing is restated here rather than approximated:
/// a key whose packed colour disagreed with `glifo`'s would compare unequal to
/// the entry `glifo` itself stored and miss on every lookup.
#[must_use]
fn pack_context_color(color: AlphaColor<Srgb>) -> u32 {
    color.premultiply().to_rgba8().to_u32()
}
