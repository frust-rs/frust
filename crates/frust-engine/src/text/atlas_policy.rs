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
use std::hash::{BuildHasherDefault, Hasher};

use glifo::{
    AtlasConfig, AtlasSlot, FontEmbolden, GlyphAtlas, GlyphCacheConfig, GlyphCacheKey, ImageCache,
    NormalizedCoord, PendingClearRect, RasterMetrics,
};
use kurbo::{Affine, Diagonal2};
use peniko::color::palette::css::BLACK;
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

/// Whether `run` is drawn with synthetic oblique, given the compiler's switch
/// (`SceneCompiler::set_synthetic_oblique`, off by default). A run it applies
/// to is drawn with a skewing glyph transform, and is routed to outlines every
/// frame (`glifo` caches no run whose transform skews, and an upright entry
/// must never be probed for it). Off, an obliqued face draws upright and is
/// cached like any upright run, as it did before the angle was carried.
#[must_use]
pub(crate) fn oblique_synthesis(run: &frust_scene::GlyphRun, enabled: bool) -> bool {
    enabled && run.font.skew_degrees() != 0.0
}

/// The glyph transform that leans `run`'s glyphs, when it has synthetic
/// oblique (see [`oblique_synthesis`]). The outline is flipped to layout space
/// (y down) before this is applied, so a rightward lean of the top is a
/// negative shear.
#[must_use]
pub(crate) fn oblique_transform(run: &frust_scene::GlyphRun, enabled: bool) -> Option<Affine> {
    oblique_synthesis(run, enabled)
        .then(|| Affine::skew(-f64::from(run.font.skew_degrees()).to_radians().tan(), 0.0))
}

/// The most variation axes a cached run's key holds inline. A face with more
/// axes than this is drawn as outlines ([`OutlineReason::VariationAxes`]):
/// the key stays `Copy` and allocation-free, and no real UI face comes close.
pub(crate) const MAX_KEYED_AXES: usize = 8;

/// The faux-bold stroke, per side, a run of `fraction` em is drawn with when
/// the stroke is a fraction of `basis` (see [`embolden_basis`]).
///
/// `fraction` is the whole added stroke width as a fraction of the em (see
/// `FontHandle::embolden`), so each side of the outline grows by half of it.
/// Zero when there is no synthesis, or when the basis is not a finite positive
/// number.
#[must_use]
pub(crate) fn embolden_per_side(fraction: f32, basis: f32) -> f32 {
    let amount = fraction * basis * 0.5;
    if amount.is_finite() && amount > 0.0 {
        amount
    } else {
        0.0
    }
}

/// The size `glifo` draws a run's outlines at: `size` with the run
/// transform's absorbed scale multiplied in, in `f32` exactly as `glifo`
/// multiplies it, or `size` itself when the transform is not absorbed and the
/// outline is drawn at its nominal size under the full transform.
#[must_use]
pub(crate) fn drawn_font_size(size: f32, transform: Affine) -> f32 {
    absorbed_scale(transform).map_or(size, |scale| size * scale as f32)
}

/// The length a run's faux-bold stroke is a fraction of: the unit the amount
/// handed to `glifo` is measured in.
///
/// `glifo` (`GlyphScaleProperties::new`, `glyph.rs`) caches an outline at one
/// of two sizes, and applies `FontEmbolden` to the outline *at that size*
/// before the draw scale `draw_size / cache_size` is applied:
///
/// - HINTED (`hinting_instance` is set, which `glifo` does when the run's
///   transform is a positive uniform scale without *vertical* skew,
///   [`hinted_absorbed_scale`] — a horizontal skew is still hinted): the
///   outline is cached at the device size `font_size * d`, with no further
///   scale, so the stroke is in device pixels. The basis is that size. A
///   hinted oblique run is still absorbed (the oblique skew is horizontal), so
///   oblique does not change the basis. Note this is a looser predicate than
///   [`absorbed_scale`], which gates the atlas and so rejects every skew.
/// - UNHINTED (every run of the mobile tier, `hint(false)`, and any hinted run
///   whose transform `glifo` cannot absorb): the outline is cached at the
///   face's `unitsPerEm` and scaled to the draw size afterwards, so the stroke
///   is in font units and the scale turns `fraction * upem` into
///   `fraction * draw_size` on screen. The basis is `upem`. Oblique or not
///   changes nothing at this level: a skew only changes the draw transform,
///   never the cache size, and whether the scale was absorbed
///   (`AbsorbScaleUnhinted`) or left in the transform (`Direct`) only changes
///   where `draw_size / upem` is applied.
///
/// `upem` is the face's units-per-em ([`FaceFacts::upem`](super::FaceFacts)),
/// read once per face. One function for the lowering, the route key and the
/// resident plan, so the embolden bits each derives are the same.
#[must_use]
pub(crate) fn embolden_basis(size: f32, transform: Affine, hinted: bool, upem: f32) -> f32 {
    match hinted.then(|| hinted_absorbed_scale(transform)).flatten() {
        // `glifo`'s `vertical_font_size`: `run.font_size * t_d`, in `f32`.
        Some(scale) => size * scale as f32,
        None => upem,
    }
}

/// `glifo`'s *hinted* absorption predicate
/// (`is_positive_uniform_scale_without_vertical_skew`, `util.rs`): a positive
/// uniform scale with no vertical skew. Unlike [`absorbed_scale`] it admits a
/// horizontal skew (`c`), because `glifo` hints vertically only. Returns the
/// vertical coefficient `d`, which `glifo` multiplies the font size by.
///
/// The run's own transform is enough to decide it: `glifo` multiplies a glyph
/// transform onto it, and the only glyph transform this tier sets is the
/// horizontal skew of synthetic oblique, which leaves `b` and `d` unchanged.
#[must_use]
pub(crate) fn hinted_absorbed_scale(transform: Affine) -> Option<f64> {
    let [a, b, c, d, _, _] = transform.as_coeffs();
    if !(a.is_finite() && b.is_finite() && c.is_finite() && d.is_finite()) {
        return None;
    }
    let uniform = (a - d).abs() <= NEARLY_ZERO && a > 0.0 && d > 0.0;
    (uniform && b.abs() <= NEARLY_ZERO).then_some(d)
}

/// What a run is drawn with beyond its font and size: the variation
/// coordinates and the faux-bold stroke. Both reach `glifo`'s own outline key,
/// so both are part of every key built here, and two runs differing in either
/// never share a cache record.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Synthesis {
    /// How many of `coords` are in use.
    axes: u8,
    /// The run's normalized coordinates, inline.
    coords: [NormalizedCoord; MAX_KEYED_AXES],
    /// The per-side embolden amount's `f32` bits (see [`embolden_per_side`]).
    embolden_bits: u32,
}

impl Synthesis {
    /// No coordinates and no embolden: the face's default instance.
    pub(crate) const NONE: Self = Self {
        axes: 0,
        coords: [0; MAX_KEYED_AXES],
        embolden_bits: 0,
    };

    /// The synthesis for `coords` and a per-side `embolden` amount, or `None`
    /// when there are more axes than a key holds.
    #[must_use]
    pub(crate) fn new(coords: &[NormalizedCoord], embolden: f32) -> Option<Self> {
        if coords.len() > MAX_KEYED_AXES {
            return None;
        }
        let mut inline = [0; MAX_KEYED_AXES];
        inline[..coords.len()].copy_from_slice(coords);
        Some(Self {
            axes: coords.len() as u8,
            coords: inline,
            embolden_bits: embolden.max(0.0).to_bits(),
        })
    }

    /// The coordinates in use.
    #[must_use]
    pub(crate) fn coords(&self) -> &[NormalizedCoord] {
        &self.coords[..usize::from(self.axes)]
    }

    /// The embolden `glifo` is handed and keys on.
    #[must_use]
    pub(crate) fn embolden(&self) -> FontEmbolden {
        embolden_for(f32::from_bits(self.embolden_bits))
    }

    fn same(&self, other: &Self) -> bool {
        self.embolden_bits == other.embolden_bits && self.coords() == other.coords()
    }
}

/// The symmetric stroke `glifo` expands an outline by: the same amount
/// horizontally and vertically, with its default miter joins. The default (no
/// expansion) when `per_side` is zero.
#[must_use]
pub(crate) fn embolden_for(per_side: f32) -> FontEmbolden {
    if per_side > 0.0 {
        FontEmbolden::new(Diagonal2::new(f64::from(per_side), f64::from(per_side)))
    } else {
        FontEmbolden::default()
    }
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
    /// The run is synthetically obliqued. `glifo` caches no run whose
    /// transform skews, and an upright entry of the same glyph must never be
    /// probed for it, so it is drawn as outlines every frame.
    SyntheticOblique,
    /// The face's variation instance has more axes than a key holds inline
    /// ([`MAX_KEYED_AXES`]).
    VariationAxes,
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
    /// The variation coordinates and faux-bold stroke the run is drawn with.
    synthesis: Synthesis,
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
    /// The embolden and coordinates are the run's own, the ones
    /// `super::lower_glyph_run` hands `glifo` for it, so a key names the glyph
    /// the engine draws: a bold and a regular run of one blob never share an
    /// entry.
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
            self.synthesis.embolden(),
            bytemuck::cast_slice(self.synthesis.coords()),
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
    /// The variation coordinates and faux-bold stroke the run is drawn with,
    /// or `None` when it has more axes than a key holds.
    pub(crate) synthesis: Option<Synthesis>,
    /// Whether the run is synthetically obliqued: a *routing* input, which
    /// sends it to outlines.
    pub(crate) oblique: bool,
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
    /// `seen` is scratch for the distinct-glyph count — caller-owned, emptied
    /// here and reused across the runs of a walk, so counting a
    /// ten-thousand-glyph run costs no allocation of its own (see
    /// [`DistinctGlyphs`]).
    #[must_use]
    pub(crate) fn for_run_with(
        run: &frust_scene::GlyphRun,
        transform: Affine,
        hinted: bool,
        color_font: bool,
        context_color: AlphaColor<Srgb>,
        seen: &mut impl DistinctGlyphs,
        basis: RunBasis,
    ) -> Self {
        let font = run.font.font();
        let distinct = seen.count_distinct(run.glyphs.iter().map(|glyph| glyph.id));
        let oblique = oblique_synthesis(run, basis.synthetic_oblique);
        let embolden = embolden_per_side(
            run.font.embolden(),
            embolden_basis(run.font_size, transform, hinted, basis.upem),
        );
        Self {
            font_id: font.data.id(),
            font_index: font.index,
            font_size: run.font_size,
            distinct_glyphs: u32::try_from(distinct).unwrap_or(u32::MAX),
            transform,
            hinted,
            color_font,
            context_color,
            synthesis: Synthesis::new(run.font.normalized_coords(), embolden),
            oblique,
        }
    }

    /// [`Self::for_run_with`] for a run with no faux bold and no synthetic
    /// oblique, which needs neither the face's units-per-em nor the switch.
    #[cfg(test)]
    pub(crate) fn for_run(
        run: &frust_scene::GlyphRun,
        transform: Affine,
        hinted: bool,
        color_font: bool,
        context_color: AlphaColor<Srgb>,
        seen: &mut impl DistinctGlyphs,
    ) -> Self {
        debug_assert!(run.font.embolden() == 0.0, "use for_run_with for faux bold");
        let basis = RunBasis {
            upem: 0.0,
            synthetic_oblique: false,
        };
        Self::for_run_with(
            run,
            transform,
            hinted,
            color_font,
            context_color,
            seen,
            basis,
        )
    }
}

/// What a [`RunKey`] needs beyond the run itself, both owned by the caller:
/// the face's units-per-em and the compiler's synthetic-oblique switch.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RunBasis {
    /// The face's units-per-em (see [`embolden_basis`]).
    pub(crate) upem: f32,
    /// `SceneCompiler::synthetic_oblique_enabled`.
    pub(crate) synthetic_oblique: bool,
}

/// Scratch a run's distinct glyph ids are counted in — what
/// [`RunKey::for_run`] charges a run's admission at.
///
/// Counted once per run per frame, so the scratch is reused rather than
/// allocated. The frame path counts in a `Vec`: sorting a run's ids and
/// dropping repeats costs no hashing at all, and a run is a line or a label —
/// tens of glyphs — where a sort beats rebuilding a hash set every time. A
/// `HashSet` counts the same number, for a caller that already holds one.
pub(crate) trait DistinctGlyphs {
    /// How many distinct values `ids` holds; whatever the scratch held before
    /// is discarded.
    fn count_distinct(&mut self, ids: impl Iterator<Item = u32>) -> usize;
}

impl DistinctGlyphs for Vec<u32> {
    fn count_distinct(&mut self, ids: impl Iterator<Item = u32>) -> usize {
        self.clear();
        self.extend(ids);
        self.sort_unstable();
        self.dedup();
        self.len()
    }
}

impl DistinctGlyphs for HashSet<u32> {
    fn count_distinct(&mut self, ids: impl Iterator<Item = u32>) -> usize {
        self.clear();
        self.extend(ids);
        self.len()
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
    /// How many of this frame's misses were the resident-run cache's own
    /// probes, which [`frame_misses`](Self::frame_misses) leaves out.
    probe_misses: u64,
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
    /// `glifo`'s last eviction serial, mirrored the same way as
    /// [`serial`](Self::serial): `maintain` reaps exactly when the serial it
    /// ticks to is [`EVICTION_FREQUENCY`] past this, so the policy knows which
    /// call will evict before making it — which is when the resident runs'
    /// slots have to be touched (see [`ResidentRuns`]).
    last_eviction_serial: u64,
    /// Glyph runs whose every glyph is resident, remembered with their slots so
    /// a later frame can draw them without handing the run to `glifo`.
    resident: ResidentRuns,
    /// This frame's counters for the `frust-perf enc` line.
    #[cfg(feature = "perf-trace")]
    trace: AtlasTrace,
}

/// What one frame's policy calls did, for the `frust-perf enc` line.
///
/// Reset by [`AtlasPolicy::begin_frame`], so each value is the frame's own.
/// `perf-trace`-only: a release build carries neither the type nor the
/// bookkeeping that feeds it.
#[cfg(feature = "perf-trace")]
#[derive(Debug, Default)]
pub(crate) struct AtlasTrace {
    /// Distinct run identities (font, collection index, raw size, hinting)
    /// that reached classification this frame.
    run_keys: HashSet<(u64, u32, u32, bool)>,
    /// Glyph cache misses the frame took, as [`AtlasPolicy::frame_misses`]
    /// counts them. Assigned once per frame by [`AtlasPolicy::end_frame`],
    /// after `glifo` has run the frame's lookups; `collect_glyph` does not feed
    /// it, since nothing on the production path reaches that half.
    pub(crate) glyph_misses: u32,
    /// Atlas rectangles the frame's eviction pass freed.
    pub(crate) glyph_evicted: u32,
}

#[cfg(feature = "perf-trace")]
impl AtlasTrace {
    /// Distinct run identities classified this frame.
    pub(crate) fn run_keys(&self) -> u32 {
        u32::try_from(self.run_keys.len()).unwrap_or(u32::MAX)
    }
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
            probe_misses: 0,
            unreplayed_since: None,
            serial: 0,
            phase: Phase::Idle,
            last_eviction_serial: 0,
            resident: ResidentRuns::default(),
            #[cfg(feature = "perf-trace")]
            trace: AtlasTrace::default(),
        }
    }

    /// The counters of the frame most recently compiled.
    #[cfg(feature = "perf-trace")]
    #[must_use]
    pub(crate) fn trace(&self) -> &AtlasTrace {
        &self.trace
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

    /// The entry map, read-only — for the counters a caller compares across a
    /// run.
    #[must_use]
    pub(crate) fn atlas(&self) -> &GlyphAtlas {
        &self.atlas
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
        // Every way an entry leaves the map on this policy's watch goes through
        // `end_frame`, which keeps the resident runs in step with it. The map is
        // handed out between frames too (see [`atlas_mut`](Self::atlas_mut)), so
        // a map that is somehow *smaller* than the one the last frame closed on
        // lost entries nobody accounted for — and a remembered slot may name a
        // rectangle that is no longer the glyph's. Forgetting every run is the
        // answer that cannot draw a wrong pixel.
        if self.atlas.len() < self.resident.expected_entries {
            self.resident.forget_all();
        }
        self.pending.clear();
        self.claimed.clear();
        // Read before anything this frame can miss, so `frame_misses` below is
        // this frame's own count rather than the process's.
        self.misses_at_frame_start = self.atlas.cache_misses();
        self.probe_misses = 0;
        #[cfg(feature = "perf-trace")]
        {
            self.trace.run_keys.clear();
            self.trace.glyph_misses = 0;
            self.trace.glyph_evicted = 0;
        }
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
    ///
    /// The resident-run cache's own probes are not among them: a probe is a
    /// lookup and nothing else — it records, queues and allocates nothing —
    /// and the run it missed for goes on to `glifo`, whose own lookup of the
    /// same glyph is the miss that counts. Counting both would report every
    /// glyph a resident run missed twice.
    #[must_use]
    fn frame_misses(&self) -> u64 {
        self.atlas
            .cache_misses()
            .saturating_sub(self.misses_at_frame_start)
            .saturating_sub(self.probe_misses)
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
        if run.oblique {
            return RunRoute::Outline(OutlineReason::SyntheticOblique);
        }
        let Some(synthesis) = run.synthesis else {
            return RunRoute::Outline(OutlineReason::VariationAxes);
        };
        let Some(device_size) = device_font_size(run.font_size, run.transform) else {
            return RunRoute::Outline(OutlineReason::TransformUncacheable);
        };
        let Some(size) = quantize_font_size(device_size) else {
            return RunRoute::Outline(OutlineReason::UnusableSize);
        };

        #[cfg(feature = "perf-trace")]
        self.trace.run_keys.insert((
            run.font_id,
            run.font_index,
            run.font_size.to_bits(),
            run.hinted,
        ));

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
            synthesis,
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

        #[cfg(feature = "perf-trace")]
        {
            self.trace.glyph_misses = u32::try_from(self.frame_misses()).unwrap_or(u32::MAX);
        }

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
            self.resident.expected_entries = self.atlas.len();
            self.phase = Phase::Idle;
            return;
        }

        // A resident run is drawn without `glifo` probing its glyphs, so their
        // entries' LRU ages stand still while it is on screen. The ages are only
        // ever *read* by the reaping call, so that is the one call ahead of
        // which they have to be true: the run's slots are touched here, once
        // per eviction pass, instead of once per glyph per frame (see
        // [`ResidentRuns`]).
        let evicting = self
            .serial
            .saturating_add(1)
            .saturating_sub(self.last_eviction_serial)
            >= EVICTION_FREQUENCY;
        if evicting {
            self.resident.touch(&mut self.atlas, self.serial);
        }
        let entries_before = self.atlas.len();
        self.atlas.maintain(images);
        // `maintain` ticks the LRU serial exactly once per call, and this is its
        // only caller for this map (see [`serial`](Self::serial)).
        self.serial = self.serial.saturating_add(1);
        if evicting {
            self.last_eviction_serial = self.serial;
        }
        self.discharge_evictions();
        let evicted: Vec<PendingClearRect> = self.atlas.drain_pending_clear_rects().collect();
        // Reaped without the touch having run first: the mirror above disagreed
        // with `glifo` about which call evicts. Nothing remembered can be
        // trusted to have survived, so nothing is.
        if !evicting && (!evicted.is_empty() || self.atlas.len() < entries_before) {
            self.resident.forget_all();
        }
        self.resident.expected_entries = self.atlas.len();
        #[cfg(feature = "perf-trace")]
        {
            self.trace.glyph_evicted = self
                .trace
                .glyph_evicted
                .saturating_add(u32::try_from(evicted.len()).unwrap_or(u32::MAX));
        }
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

    /// Resolve every glyph of a run out of the resident-run cache, probing
    /// `glifo`'s map only for a glyph the cache has not seen at this subpixel
    /// bucket since its slots were last known good.
    ///
    /// `placed` is the run's glyphs in order, each with the bucket its device
    /// origin falls into *this* frame (see [`subpixel_bucket`]); `slots`
    /// receives one slot per glyph, in the same order, when the answer is
    /// [`ResidentAnswer::Resolved`]. Answers only in the draw phase, for the
    /// same reason [`slot`](Self::slot) does.
    ///
    /// A probe that misses ends the attempt: the run goes to `glifo`, which
    /// keys, allocates and records the glyph exactly as it always has, and the
    /// record keeps what it learned so the next frame starts from there.
    pub(crate) fn resolve_resident_run(
        &mut self,
        identity: ResidentIdentity,
        placed: &[PlacedGlyph],
        slots: &mut Vec<AtlasSlot>,
    ) -> ResidentAnswer {
        slots.clear();
        if self.phase != Phase::Draw || placed.is_empty() {
            return ResidentAnswer::Bypassed;
        }
        let serial = self.serial;
        let black = self.resident.black;
        let hash = run_hash(&identity, placed);
        let Some(run) = self.resident.open(hash, identity, placed, self.frame) else {
            return ResidentAnswer::Bypassed;
        };
        if run.refused {
            return ResidentAnswer::Bypassed;
        }
        run.used = serial;
        run.frame = self.frame;

        for (glyph, record) in placed.iter().zip(run.glyphs.iter_mut()) {
            let bucket = usize::from(glyph.bucket);
            let Some(known) = record.slots.get(bucket).copied() else {
                // Out of range only for a bucket `subpixel_bucket` never
                // produces; such a run is `glifo`'s to draw.
                return ResidentAnswer::Bypassed;
            };
            let slot = match known {
                Some(slot) => slot,
                None => {
                    let key = outline_key(&identity, glyph.id, glyph.bucket, black);
                    let Some(slot) = self.atlas.get(&key) else {
                        self.probe_misses = self.probe_misses.saturating_add(1);
                        return ResidentAnswer::Missed(hash);
                    };
                    record.slots[bucket] = Some(slot);
                    slot
                }
            };
            record.used[bucket] = serial;
            slots.push(slot);
        }
        ResidentAnswer::Resolved
    }

    /// Stop offering the run `handle` names the resident path.
    ///
    /// For a run the resident path missed and `glifo` then drew without a
    /// single miss of its own — which means `glifo` resolved the run through
    /// keys this path does not build (a bitmap strike, a face it could not
    /// hint). Probing it again every frame would only repeat the miss, so the
    /// record is kept as a refusal until it ages out.
    pub(crate) fn refuse_resident_run(&mut self, handle: u64) {
        self.resident.refuse(handle);
    }

    /// How many glyph runs the resident-run cache currently remembers.
    #[must_use]
    pub(crate) fn resident_runs(&self) -> usize {
        self.resident.runs.len()
    }

    /// What the resident-run cache's records currently occupy, in bytes.
    #[must_use]
    pub(crate) fn resident_run_bytes(&self) -> usize {
        self.resident.bytes
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

// ---------------------------------------------------------------------------
// Resident runs
// ---------------------------------------------------------------------------

/// Horizontal subpixel buckets `glifo` quantizes a cached outline glyph's
/// position into.
///
/// Restated because `glifo` 0.3.0 keeps the constant crate-private. The bucket
/// a resident run resolves a glyph into has to be the one `glifo` would key it
/// under — a neighbouring bucket names a slot rasterized at a different
/// subpixel phase — so a test pins [`subpixel_bucket`] to
/// `GlyphCacheKey::new`'s own quantization.
const SUBPIXEL_BUCKETS: u8 = 4;

/// The subpixel bucket `glifo` keys an outline glyph under when its device
/// origin's horizontal fraction is `fractional_x`.
///
/// `glifo`'s private `quantize_subpixel`, restated: the fraction is wrapped
/// into `[0, 1)`, scaled by the bucket count and rounded, and a value rounding
/// up to a whole pixel clamps into the last bucket rather than wrapping to the
/// first.
#[must_use]
pub(crate) fn subpixel_bucket(fractional_x: f32) -> u8 {
    let normalized = fractional_x.fract();
    let normalized = if normalized < 0.0 {
        normalized + 1.0
    } else {
        normalized
    };
    // In `0.0..=4.0` (or NaN, which converts to zero) by construction.
    let bucket = (normalized * f32::from(SUBPIXEL_BUCKETS)).round() as u8;
    bucket.min(SUBPIXEL_BUCKETS - 1)
}

/// The most glyph runs the resident-run cache remembers at once.
///
/// A bound on the record count, beside [`MAX_RESIDENT_RUN_BYTES`]' bound on
/// what the records hold: a scrolling list of strings that never repeat mints
/// a record per new line, and neither number may grow with how long it
/// scrolls.
pub(crate) const MAX_RESIDENT_RUNS: usize = 1024;

/// The most bytes the resident-run cache's records hold at once — every
/// remembered run's glyph ids and per-bucket slots together.
///
/// Two mebibytes is a little over fifteen thousand glyph positions, several
/// screens of dense text; past it the least recently drawn runs are forgotten
/// first (see [`ResidentRuns`]).
pub(crate) const MAX_RESIDENT_RUN_BYTES: usize = 2 * 1024 * 1024;

/// The serial a bucket nothing has been drawn from is stamped with.
const NEVER: u64 = u64::MAX;

/// What a resident run is identified by besides its glyph ids: everything
/// `glifo` builds an outline glyph's key from except the glyph and its
/// subpixel bucket.
///
/// Neither the run's translation nor its brush colour is here. The
/// translation only moves the glyphs — their buckets are re-derived from it
/// every frame, and a bucket the record has not seen is probed like any new
/// glyph — and the colour is the tint of the record the run's quads share,
/// which `glifo` never keys an outline on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ResidentIdentity {
    /// The font blob's process-unique id.
    pub(crate) font_id: u64,
    /// Index within a font collection.
    pub(crate) font_index: u32,
    /// The size `glifo` keys the run's glyphs at: the display list's size with
    /// the run's absorbed scale multiplied in, exactly as `glifo` multiplies
    /// it.
    pub(crate) size: f32,
    /// Whether the run's outlines are hinted.
    pub(crate) hinted: bool,
    /// The variation coordinates and faux-bold stroke the run is drawn with.
    pub(crate) synthesis: Synthesis,
}

/// One glyph of a run about to be resolved: its id, and the subpixel bucket
/// its device origin falls into this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlacedGlyph {
    /// The glyph id.
    pub(crate) id: u32,
    /// Its bucket, from [`subpixel_bucket`].
    pub(crate) bucket: u8,
}

/// What [`AtlasPolicy::resolve_resident_run`] made of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResidentAnswer {
    /// Every glyph resolved, and its slot is in the caller's buffer.
    Resolved,
    /// A glyph was not resident. The run is `glifo`'s to draw; the handle names
    /// its record, for [`AtlasPolicy::refuse_resident_run`].
    Missed(u64),
    /// The run is not offered the resident path at all: refused earlier, too
    /// large to remember, or asked outside the draw phase.
    Bypassed,
}

/// One glyph position of a remembered run: the slot it resolved to in each
/// subpixel bucket it has been drawn in, and the LRU serial each was last
/// drawn at.
#[derive(Debug, Clone, Copy)]
struct ResidentGlyph {
    /// The slot per bucket, `None` until a probe found one.
    slots: [Option<AtlasSlot>; SUBPIXEL_BUCKETS as usize],
    /// The serial each bucket's slot was last drawn at, [`NEVER`] if not yet.
    used: [u64; SUBPIXEL_BUCKETS as usize],
}

impl ResidentGlyph {
    const EMPTY: Self = Self {
        slots: [None; SUBPIXEL_BUCKETS as usize],
        used: [NEVER; SUBPIXEL_BUCKETS as usize],
    };
}

/// One remembered glyph run.
#[derive(Debug)]
struct ResidentRun {
    /// What its glyphs are keyed under.
    identity: ResidentIdentity,
    /// Its glyph ids, in order — compared whole on every lookup, so two runs
    /// sharing a hash can never share a record.
    ids: Box<[u32]>,
    /// Per glyph position, the slots it resolved to.
    glyphs: Box<[ResidentGlyph]>,
    /// The LRU serial it was last asked about at.
    used: u64,
    /// The frame it was last asked about on — the order the bound forgets in.
    frame: u64,
    /// Whether `glifo` was seen drawing it through keys this path does not
    /// build (see [`AtlasPolicy::refuse_resident_run`]).
    refused: bool,
}

impl ResidentRun {
    fn new(identity: ResidentIdentity, placed: &[PlacedGlyph], frame: u64) -> Self {
        Self {
            identity,
            ids: placed.iter().map(|glyph| glyph.id).collect(),
            glyphs: vec![ResidentGlyph::EMPTY; placed.len()].into_boxed_slice(),
            used: NEVER,
            frame,
            refused: false,
        }
    }

    /// Whether this record is the run `identity` and `placed` describe.
    fn matches(&self, identity: &ResidentIdentity, placed: &[PlacedGlyph]) -> bool {
        self.identity.font_id == identity.font_id
            && self.identity.font_index == identity.font_index
            && self.identity.size.to_bits() == identity.size.to_bits()
            && self.identity.hinted == identity.hinted
            && self.identity.synthesis.same(&identity.synthesis)
            && self.ids.len() == placed.len()
            && self
                .ids
                .iter()
                .zip(placed)
                .all(|(id, glyph)| *id == glyph.id)
    }

    /// What the record occupies, in bytes.
    fn bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of_val(&*self.ids)
            + std::mem::size_of_val(&*self.glyphs)
    }
}

/// Multiplier of the run hash's mixing step (the Fx hash's).
const RUN_HASH_MULTIPLIER: u64 = 0x517c_c1b7_2722_0a95;

/// One mixing step of the run hash.
fn mix(hash: u64, word: u64) -> u64 {
    (hash.rotate_left(5) ^ word).wrapping_mul(RUN_HASH_MULTIPLIER)
}

/// The hash a run's record is filed under: one pass over its identity and its
/// glyph ids, a word per glyph — the once-per-run cost the record is looked up
/// for.
fn run_hash(identity: &ResidentIdentity, placed: &[PlacedGlyph]) -> u64 {
    let mut hash = mix(0, identity.font_id);
    hash = mix(hash, u64::from(identity.font_index));
    hash = mix(hash, u64::from(identity.size.to_bits()));
    hash = mix(hash, u64::from(identity.hinted));
    hash = mix(hash, u64::from(identity.synthesis.embolden_bits));
    for coord in identity.synthesis.coords() {
        hash = mix(hash, u64::from(*coord as u16));
    }
    hash = mix(hash, placed.len() as u64);
    for glyph in placed {
        hash = mix(hash, u64::from(glyph.id));
    }
    hash
}

/// The hasher the record map is keyed through: the key already *is* a hash.
#[derive(Debug, Default)]
struct RunHashHasher(u64);

impl Hasher for RunHashHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = mix(self.0, u64::from(*byte));
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = mix(self.0, value);
    }
}

/// The key `glifo` files an outline glyph of `identity`'s run under, for glyph
/// `glyph_id` in subpixel bucket `bucket`.
///
/// Built field for field as `glifo` builds it for the outline probe it makes
/// first for every glyph: the run's absorbed size and hinting, the bucket's
/// own fractional offset (which quantizes back to the bucket), black as the
/// context colour (an outline's ink is the tint, never the key), the run's
/// own embolden and variation coordinates, which the lowering hands `glifo`.
/// `black` is black packed, computed once by the caller.
fn outline_key(
    identity: &ResidentIdentity,
    glyph_id: u32,
    bucket: u8,
    black: u32,
) -> GlyphCacheKey {
    GlyphCacheKey::new(
        identity.font_id,
        identity.font_index,
        glyph_id,
        identity.size,
        identity.hinted,
        glifo::atlas::key::subpixel_offset(bucket),
        BLACK,
        black,
        identity.synthesis.embolden(),
        bytemuck::cast_slice(identity.synthesis.coords()),
    )
}

/// Glyph runs whose every glyph was resident, remembered with the slots they
/// resolved to so a later frame can draw them without `glifo`.
///
/// `glifo` resolves a cached glyph by building its key, hashing it, probing the
/// entry map and refreshing the entry's LRU serial — and drives the sink one
/// glyph at a time while doing it. For a run drawn unchanged frame after frame
/// that whole walk re-derives an answer it already gave. A record here is that
/// answer kept: the run's glyph ids, and per glyph and per subpixel bucket the
/// slot it resolved to, so a resident run costs one hash of its ids and one
/// array read per glyph.
///
/// # What keeps a remembered slot true
///
/// A slot stays the glyph's for as long as its entry is in `glifo`'s map, and
/// an entry leaves the map in exactly one way on this policy's watch: the
/// eviction pass inside `maintain`, which only [`AtlasPolicy::end_frame`]
/// calls. Insertion never moves an entry, so a frame that rasterizes new
/// glyphs leaves every remembered slot as it was — which is why the records
/// are not voided per insertion: a frame of animated text misses a glyph or two
/// on most frames, and a cache emptied by each of them would never be read.
///
/// # What keeps the LRU true
///
/// A record drawn without `glifo` probes nothing, so its entries' ages would
/// stand still and a run on screen would be reaped as though it were gone. But
/// an entry's age is only *read* by the eviction pass, and the policy knows
/// which `maintain` call runs one (see `last_eviction_serial`). So the ages
/// are made true once, there: ahead of that call every bucket drawn within the
/// last [`MAX_ENTRY_AGE`] serials is probed — one lookup per remembered slot,
/// once per eviction pass — and stamped exactly as fresh as a per-frame probe
/// would have left it, while a bucket drawn longer ago than that is forgotten,
/// since the pass may reap it. A run in use is therefore never reaped and a
/// stale one is reaped on the very pass it would have been without this cache.
///
/// # What keeps the sibling buckets
///
/// A glyph at one position is keyed once per subpixel bucket, and a label that
/// creeps by a fraction of a pixel stays in one bucket for longer than the age
/// horizon while the other three go unused, age out, and are rasterized again
/// the moment the label drifts back — a miss, and so a replay, on a frame that
/// draws nothing new. So the pass is also given the glyph: while a position was
/// drawn in *any* bucket within the window, every one of its buckets is probed,
/// resident or not, and so every resident sibling is kept, drawn or not. The
/// probe of a bucket that is not resident is what finds a sibling `glifo`
/// inserted outside this record. Each such probe that misses is a `glifo`
/// cumulative miss that lands after the frame's `frame_misses()` has been read,
/// so no frame is charged for it. The cost is bounded by what is already in
/// use — at most the four buckets of the glyphs on screen — and a glyph nothing
/// draws any more still ages out after the window, together with its siblings. Only a position the record remembers is covered; a run the record
/// does not hold (refused, over the bound, forgotten) is `glifo`'s to age.
///
/// A map that loses entries any other way — a reap the mirror did not predict,
/// a map smaller at the start of a frame than at the end of the last — forgets
/// every record, which costs one frame of probes and no pixel.
///
/// # Bounded
///
/// [`MAX_RESIDENT_RUNS`] records and [`MAX_RESIDENT_RUN_BYTES`] bytes. Past
/// either, the least recently drawn records are forgotten first, down to three
/// quarters of both, sparing every run drawn on the current frame; a run that
/// still does not fit is drawn by `glifo` and not remembered. A record also
/// leaves at the first eviction pass after it was last drawn more than
/// [`MAX_ENTRY_AGE`] serials ago.
#[derive(Debug)]
pub(crate) struct ResidentRuns {
    /// The records, by [`run_hash`].
    runs: HashMap<u64, ResidentRun, BuildHasherDefault<RunHashHasher>>,
    /// What the records occupy, in bytes.
    bytes: usize,
    /// `glifo`'s entry count as the last frame closed.
    expected_entries: usize,
    /// Scratch for the bound's least-recently-drawn order.
    order: Vec<(u64, u64)>,
    /// Black, packed as an outline key carries it — computed once rather than
    /// per run.
    black: u32,
}

impl Default for ResidentRuns {
    fn default() -> Self {
        Self {
            runs: HashMap::default(),
            bytes: 0,
            expected_entries: 0,
            order: Vec::new(),
            black: pack_context_color(BLACK),
        }
    }
}

impl ResidentRuns {
    /// The record for `placed` under `identity`, created if there is none and
    /// there is room; `None` when the run is not to be remembered.
    fn open(
        &mut self,
        hash: u64,
        identity: ResidentIdentity,
        placed: &[PlacedGlyph],
        frame: u64,
    ) -> Option<&mut ResidentRun> {
        let known = self
            .runs
            .get(&hash)
            .is_some_and(|run| run.matches(&identity, placed));
        if !known {
            // A run filed under a hash another run already holds replaces it:
            // the ids are compared whole, so the two can never share slots.
            if let Some(previous) = self.runs.remove(&hash) {
                self.bytes = self.bytes.saturating_sub(previous.bytes());
            }
            let run = ResidentRun::new(identity, placed, frame);
            let bytes = run.bytes();
            if !self.make_room(bytes, frame) {
                return None;
            }
            self.bytes = self.bytes.saturating_add(bytes);
            self.runs.insert(hash, run);
        }
        self.runs.get_mut(&hash)
    }

    /// Make room for a record of `bytes`, forgetting the least recently drawn
    /// records first; whether it now fits.
    ///
    /// Sheds down to three quarters of both bounds at once, so a list
    /// scrolling through strings it never repeats pays one sort per quarter of
    /// the cache rather than one per new line. Runs drawn on `frame` are
    /// spared: forgetting a run the same frame drew would only re-mint it on
    /// the next.
    fn make_room(&mut self, bytes: usize, frame: u64) -> bool {
        let fits = |runs: &Self| {
            runs.runs.len() < MAX_RESIDENT_RUNS
                && runs.bytes.saturating_add(bytes) <= MAX_RESIDENT_RUN_BYTES
        };
        // A run that would take a quarter of the budget alone is not one to
        // spend it on.
        if bytes > MAX_RESIDENT_RUN_BYTES / 4 {
            return false;
        }
        if fits(self) {
            return true;
        }

        let mut order = std::mem::take(&mut self.order);
        order.clear();
        order.extend(
            self.runs
                .iter()
                .filter(|(_, run)| run.frame != frame)
                .map(|(hash, run)| (run.frame, *hash)),
        );
        order.sort_unstable();
        let run_target = MAX_RESIDENT_RUNS / 4 * 3;
        let byte_target = (MAX_RESIDENT_RUN_BYTES / 4 * 3).saturating_sub(bytes);
        for (_, hash) in order.drain(..) {
            if self.runs.len() <= run_target && self.bytes <= byte_target {
                break;
            }
            if let Some(run) = self.runs.remove(&hash) {
                self.bytes = self.bytes.saturating_sub(run.bytes());
            }
        }
        self.order = order;
        fits(self)
    }

    /// Mark the record `hash` names refused.
    fn refuse(&mut self, hash: u64) {
        if let Some(run) = self.runs.get_mut(&hash) {
            run.refused = true;
        }
    }

    /// Forget every record.
    fn forget_all(&mut self) {
        self.runs.clear();
        self.bytes = 0;
    }

    /// Make `glifo`'s LRU ages true for every remembered slot ahead of the
    /// eviction pass that `maintain` is about to run at LRU serial `serial`
    /// (see this type's doc), and forget what that pass may reap.
    fn touch(&mut self, atlas: &mut GlyphAtlas, serial: u64) {
        // The serial `maintain` ticks to before it measures any age.
        let horizon = serial.saturating_add(1);
        let fresh = |used: u64| used != NEVER && horizon.saturating_sub(used) <= MAX_ENTRY_AGE;
        let black = self.black;
        let mut freed = 0_usize;
        self.runs.retain(|_, run| {
            if !fresh(run.used) {
                freed = freed.saturating_add(run.bytes());
                return false;
            }
            for (id, glyph) in run.ids.iter().zip(run.glyphs.iter_mut()) {
                // A position drawn in any bucket within the window keeps every
                // bucket of it that is resident alive, drawn or not (see this
                // type's doc); the siblings' own `used` stays as it was, so
                // they cannot keep each other alive once nothing draws it.
                let in_use = glyph.used.iter().any(|used| fresh(*used));
                for bucket in 0..SUBPIXEL_BUCKETS {
                    let index = usize::from(bucket);
                    if !in_use && glyph.slots[index].is_none() {
                        continue;
                    }
                    // A probe stamps the entry with the serial a per-frame
                    // probe at `serial` would have left on it.
                    let current = if in_use || fresh(glyph.used[index]) {
                        atlas.get(&outline_key(&run.identity, *id, bucket, black))
                    } else {
                        None
                    };
                    glyph.slots[index] = current;
                    if current.is_none() {
                        glyph.used[index] = NEVER;
                    }
                }
            }
            true
        });
        self.bytes = self.bytes.saturating_sub(freed);
    }
}

#[cfg(test)]
mod distinct_glyph_tests {
    use super::*;

    /// The sorting scratch the frame path counts in agrees with a set, run
    /// after run, whatever the previous run left in it.
    #[test]
    fn the_sorting_scratch_counts_what_a_set_counts() {
        let runs: [&[u32]; 5] = [
            &[],
            &[7],
            &[5, 6, 7, 7, 8],
            &[9, 9, 9, 9],
            &[3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5],
        ];
        let mut sorted: Vec<u32> = Vec::new();
        let mut set: HashSet<u32> = HashSet::new();
        for ids in runs.iter().chain(runs.iter().rev()) {
            let expected = ids.iter().collect::<HashSet<_>>().len();
            assert_eq!(
                sorted.count_distinct(ids.iter().copied()),
                expected,
                "{ids:?}"
            );
            assert_eq!(set.count_distinct(ids.iter().copied()), expected, "{ids:?}");
        }
    }
}

#[cfg(test)]
mod resident_run_tests {
    use super::*;

    /// The bucket the resident path resolves a glyph into is the bucket
    /// `glifo` keys it under, for fractions across several whole pixels in
    /// both directions — including the ones that round up to a whole pixel
    /// and clamp into the last bucket.
    #[test]
    fn the_restated_bucket_is_glifos_own() {
        let identity = ResidentIdentity {
            font_id: 1,
            font_index: 0,
            size: 16.0,
            hinted: false,
            synthesis: Synthesis::NONE,
        };
        let black = pack_context_color(BLACK);
        for step in -4096_i32..4096 {
            let origin = f64::from(step) / 1024.0 + 0.000_7;
            let fraction = origin.fract() as f32;
            let theirs = GlyphCacheKey::new(
                1,
                0,
                7,
                16.0,
                false,
                fraction,
                BLACK,
                black,
                FontEmbolden::default(),
                &[],
            );
            let bucket = subpixel_bucket(fraction);
            assert_eq!(bucket, theirs.subpixel_x, "fraction {fraction}");
            assert_eq!(
                outline_key(&identity, 7, bucket, black),
                theirs,
                "fraction {fraction}: the probe key is the key `glifo` builds"
            );
        }
    }

    /// A resident-run probe that misses is a lookup and nothing more: it is
    /// not one of the frame's misses, which stand for work `glifo` left
    /// recorded — the lookup `glifo` then makes for the same glyph is.
    #[test]
    fn a_resident_probe_miss_is_not_a_frame_miss() {
        use vello_common::multi_atlas::AllocationStrategy;

        let mut images = ImageCache::new_with_config(AtlasConfig {
            initial_atlas_count: 0,
            max_atlases: 1,
            atlas_size: (256, 256),
            auto_grow: true,
            allocation_strategy: AllocationStrategy::FirstFit,
        });
        let mut policy = AtlasPolicy::new(&images, false);
        policy.begin_frame();
        let _ = policy.build(&mut images, |_| None);
        let identity = ResidentIdentity {
            font_id: 9,
            font_index: 0,
            size: 16.0,
            hinted: false,
            synthesis: Synthesis::NONE,
        };
        let placed = [PlacedGlyph { id: 3, bucket: 1 }];
        let mut slots = Vec::new();
        assert!(matches!(
            policy.resolve_resident_run(identity, &placed, &mut slots),
            ResidentAnswer::Missed(_)
        ));
        assert_eq!(policy.frame_misses(), 0);

        let key = outline_key(&identity, 3, 1, policy.resident.black);
        assert!(policy.atlas_mut().get(&key).is_none());
        assert_eq!(policy.frame_misses(), 1, "`glifo`'s own lookup counts");
    }

    /// The bound holds in records and in bytes whatever is offered, and spares
    /// the runs drawn on the current frame.
    #[test]
    fn the_bound_sheds_the_least_recently_drawn_first() {
        let mut runs = ResidentRuns::default();
        let identity = ResidentIdentity {
            font_id: 1,
            font_index: 0,
            size: 16.0,
            hinted: false,
            synthesis: Synthesis::NONE,
        };
        for serial in 0..(MAX_RESIDENT_RUNS as u32 * 3) {
            let frame = u64::from(serial / 10);
            let placed: Vec<PlacedGlyph> = (0..3)
                .map(|digit| PlacedGlyph {
                    id: serial.wrapping_mul(7).wrapping_add(digit),
                    bucket: 0,
                })
                .collect();
            let hash = run_hash(&identity, &placed);
            assert!(runs.open(hash, identity, &placed, frame).is_some());
            assert!(runs.runs.len() <= MAX_RESIDENT_RUNS);
            assert!(runs.bytes <= MAX_RESIDENT_RUN_BYTES);
            let current = runs.runs.values().filter(|run| run.frame == frame).count();
            assert_eq!(
                current as u32,
                serial % 10 + 1,
                "this frame's runs are all kept"
            );
        }
        let total: usize = runs.runs.values().map(ResidentRun::bytes).sum();
        assert_eq!(runs.bytes, total, "the byte count is the records' own");

        // A run too large to be worth a quarter of the budget is never kept.
        let huge: Vec<PlacedGlyph> = (0..MAX_RESIDENT_RUN_BYTES as u32 / 64)
            .map(|id| PlacedGlyph { id, bucket: 0 })
            .collect();
        let hash = run_hash(&identity, &huge);
        assert!(runs.open(hash, identity, &huge, 10_000).is_none());
    }

    /// Runs differing in variation coordinates or embolden never share a
    /// resident record, a hash, or a glyph cache key's coordinates.
    #[test]
    fn synthesis_separates_resident_identities_and_glyph_keys() {
        let base = ResidentIdentity {
            font_id: 1,
            font_index: 0,
            size: 16.0,
            hinted: false,
            synthesis: Synthesis::NONE,
        };
        let bold_axis = ResidentIdentity {
            synthesis: Synthesis::new(&[8192], 0.0).unwrap(),
            ..base
        };
        let faux_bold = ResidentIdentity {
            synthesis: Synthesis::new(&[], 0.4).unwrap(),
            ..base
        };
        let placed = [PlacedGlyph { id: 5, bucket: 0 }];
        let base_hash = run_hash(&base, &placed);
        assert_ne!(base_hash, run_hash(&bold_axis, &placed));
        assert_ne!(base_hash, run_hash(&faux_bold, &placed));
        let record = ResidentRun::new(base, &placed, 0);
        assert!(record.matches(&base, &placed));
        assert!(!record.matches(&bold_axis, &placed));
        assert!(!record.matches(&faux_bold, &placed));
        let black = pack_context_color(BLACK);
        let key = |identity: &ResidentIdentity| outline_key(identity, 5, 0, black);
        assert!(key(&base).var_coords.is_empty());
        assert_eq!(key(&bold_axis).var_coords.len(), 1);
        assert_ne!(key(&base), key(&faux_bold));
    }

    /// Too many axes for the inline key is refused, not truncated.
    #[test]
    fn too_many_axes_are_not_keyed() {
        assert!(Synthesis::new(&[0; MAX_KEYED_AXES], 0.0).is_some());
        assert!(Synthesis::new(&[0; MAX_KEYED_AXES + 1], 0.0).is_none());
    }

    /// A hinted run's stroke is measured in drawn pixels, an unhinted one's
    /// in font units, oblique or not.
    #[test]
    fn the_embolden_basis_is_the_drawn_size_hinted_and_upem_unhinted() {
        let twice = Affine::scale(2.0);
        assert_eq!(embolden_basis(16.0, twice, true, 1000.0), 32.0);
        assert_eq!(embolden_basis(16.0, twice, false, 1000.0), 1000.0);
        assert_eq!(embolden_basis(16.0, twice, false, 2048.0), 2048.0);
        // A transform `glifo` cannot absorb cannot be hinted either.
        let skewed = Affine::skew(0.4, 0.4);
        assert_eq!(embolden_basis(16.0, skewed, true, 1000.0), 1000.0);
        // A horizontal-only shear is still hinted by `glifo`: device basis.
        let sheared = Affine::new([1.0, 0.0, 0.3, 1.0, 0.0, 0.0]);
        assert_eq!(embolden_basis(16.0, sheared, true, 1000.0), 16.0);
        let sheared_twice = Affine::new([2.0, 0.0, 0.3, 2.0, 5.0, 7.0]);
        assert_eq!(embolden_basis(16.0, sheared_twice, true, 1000.0), 32.0);
        assert_eq!(embolden_basis(16.0, sheared, false, 1000.0), 1000.0);
        // A rotation or a vertical skew is not: font units.
        let rotated = Affine::rotate(0.3);
        assert_eq!(embolden_basis(16.0, rotated, true, 1000.0), 1000.0);
        let vertical = Affine::new([1.0, 0.3, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(embolden_basis(16.0, vertical, true, 1000.0), 1000.0);
        assert_eq!(
            hinted_absorbed_scale(Affine::new([1.0, 0.0, f64::NAN, 1.0, 0.0, 0.0])),
            None
        );
    }
}

#[cfg(all(test, feature = "perf-trace"))]
mod trace_tests {
    use super::*;
    use glifo::RasterMetrics;
    use peniko::color::{AlphaColor, Srgb};
    use vello_common::multi_atlas::AllocationStrategy;

    const GLYPHS: [u32; 3] = [11, 12, 13];

    fn images() -> ImageCache {
        ImageCache::new_with_config(AtlasConfig {
            initial_atlas_count: 0,
            max_atlases: 4,
            atlas_size: (1024, 1024),
            auto_grow: true,
            allocation_strategy: AllocationStrategy::FirstFit,
        })
    }

    fn run_key() -> RunKey {
        RunKey {
            font_id: 0x7ace_0001,
            font_index: 0,
            font_size: 16.0,
            distinct_glyphs: GLYPHS.len() as u32,
            transform: Affine::IDENTITY,
            hinted: false,
            color_font: false,
            context_color: AlphaColor::<Srgb>::new([0.0, 0.0, 0.0, 1.0]),
        }
    }

    /// One frame through the production sequence, with `glifo`'s own lookup
    /// (a `get`, then an `insert` on a miss) standing in for the draw that
    /// really performs it. Returns the frame's `frame_misses()`.
    fn frame(policy: &mut AtlasPolicy, images: &mut ImageCache) -> u64 {
        policy.begin_frame();
        let route = policy.classify_run(&run_key());
        let route = policy.admit_run(route);
        let Some(run) = route.atlas().copied() else {
            // Surfaces as the caller's `first > 0` assertion failing.
            return 0;
        };
        policy.build(images, |_| None);
        for glyph in GLYPHS {
            let key = run.key(glyph, 0.0);
            if policy.atlas_mut().get(&key).is_none() {
                let metrics = RasterMetrics {
                    width: 8,
                    height: 10,
                    bearing_x: 0,
                    bearing_y: 10,
                };
                let inserted = policy.atlas_mut().insert(images, key, metrics).is_some();
                assert!(inserted, "the atlas has room");
            }
        }
        let misses = policy.frame_misses();
        policy.acknowledge_clears();
        policy.end_frame(images);
        policy.acknowledge_replay();
        misses
    }

    #[test]
    fn trace_glyph_misses_reports_the_production_frame_miss_count() {
        let mut images = images();
        let mut policy = AtlasPolicy::new(&images, false);

        let first = frame(&mut policy, &mut images);
        assert!(first > 0, "a cold run must miss");
        assert_eq!(u64::from(policy.trace().glyph_misses), first);

        let second = frame(&mut policy, &mut images);
        assert_eq!(second, 0, "the resident run must hit");
        assert_eq!(policy.trace().glyph_misses, 0);
    }
}
