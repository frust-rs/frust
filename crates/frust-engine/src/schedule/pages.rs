//! Intermediate pages: which of the two ping-pong groups one comes from, and
//! how large it is asked for.
//!
//! A page is one pooled off-screen texture an isolated layer is rendered into
//! before its parent samples it. The scheduler never allocates one — it only
//! decides the group and the extent, and [`IntermediateTargets`] hands the
//! texture out at execute time. Everything here is therefore a pure function
//! over plain values, host-testable with no device in the loop.
//!
//! Two decisions live here.
//!
//! - **Group by parity.** Even-depth layers draw into the even group, odd-depth
//!   ones into the odd group, so a chain of nested layers ping-pongs between
//!   exactly two textures however deep it runs: the child's page is sampled by
//!   the parent's pass and freed the moment that pass ends, and the grandchild
//!   reuses it. Depth here is counted over *isolated* layers only, because an
//!   inlined layer never occupies a page and so never consumes a level (see the
//!   [`schedule`](super) module header). [`PageParity::Spill`] is the one group
//!   this rule does not name: it is the single bounded page a regular layer
//!   falls back on when both parity groups are held and no round could be cut
//!   to hand one back, and the [`schedule`](super) header's *The spill page*
//!   section is where the shapes that need it live.
//! - **Coverage-sized, quantized, capped.** A page is sized to the layer's own
//!   tile-aligned bounds rather than to the viewport, floored at
//!   [`PageConfig::min_page_size`] and quantized to the substrate pool's key
//!   grid so a resizing layer reuses one texture instead of churning a new one
//!   per frame. It is capped at the smaller of [`PageConfig::max_page_size`]
//!   and what the adapter will allocate; a layer larger than that is refused
//!   by [`page_size`] with [`EngineError::IntermediateTextureTooLarge`] rather
//!   than clipped to the cap, which would silently drop the layer's outer
//!   pixels — or split into [bands](page_bands), which is the answer that
//!   renders it.
//!
//! The default bounds mirror the reference renderer's own guidance for mobile:
//! keeping intermediate textures small matters more than saving render passes,
//! so the floor stays at 512 square rather than being raised to batch layers
//! together.
//!
//! ## A third decision: filter-layer sizing
//!
//! [`filter_page_size`] answers the same question [`page_size`] does — the
//! extent to acquire a page at — for a filter layer specifically, and differs
//! in two ways a regular layer's page never has to.
//!
//! - **Padded, not just coverage-sized.** A decimated filter pass overdraws a
//!   [`FILTER_ATLAS_PADDING`]-wide transparent border around the region it
//!   writes (see that constant's own doc), and a page whose real allocation
//!   landed *exactly* on the layer's extent — which the pool's quantization can
//!   and does produce, whenever a layer's own size already sits on the quantum
//!   grid — would have nowhere to put it. [`filter_page_size`] reserves the
//!   room by growing the request by [`FILTER_ATLAS_PADDING`] twice per axis
//!   *before* flooring, quantizing (E13) and capping, so the page's real extent
//!   always has it regardless of where the quantum grid happens to land.
//!
//!   Twice per axis, not "on every side": a layer is rendered into its page at
//!   the page's own origin, so the whole of the growth lands on the far side
//!   and the near side has no margin at all. That asymmetry is exactly why the
//!   kernels bound their taps against the source region themselves rather than
//!   trusting a margin to exist (`sample_region_bilinear` in
//!   `shaders/filters_blur.wgsl`); the room reserved here is what the far-side
//!   overdraw needs, not a guarantee about what a tap reads.
//! - **Two ceilings, not one.** [`page_size`] folds [`page_ceiling`]'s two
//!   inputs into one refusal ([`EngineError::IntermediateTextureTooLarge`]
//!   either way). A filter layer's padded request is checked against them
//!   separately instead, mirroring the reference sparse-strips renderer's own
//!   filter-layer sizing (`vello_hybrid`'s
//!   `LayersConfig::required_intermediate_texture_size`,
//!   `render/common.rs:139-191`): [`max_texture_size`] is what the adapter
//!   can allocate at all, refused as [`EngineError::IntermediateTextureTooLarge`]
//!   exactly as an over-ceiling regular layer is; [`PageConfig::max_page_size`]
//!   is this pool's own configured budget, which the adapter could serve but
//!   this engine has chosen not to ask for, refused as
//!   [`EngineError::IntermediateTextureLimitReached`] — the same two-error
//!   shape the reference's own `IntermediateTextureError::TooLarge`/
//!   `LimitReached` pair draws, adapted from its one-texture-for-the-whole-
//!   scene budget to this scheduler's per-layer pages.
//!
//! Both errors, like [`page_size`]'s own, are values rather than panics
//! (E17), and the padded width/height are computed in `u32` so a `bounds`
//! near the `u16` ceiling grows into headroom instead of wrapping.
//!
//! ## A fourth decision: a layer wider than any page
//!
//! A 5K desktop surface under one root opacity layer asks for an intermediate
//! wider than the ceiling above — 5120 texels against a 4096 default — and
//! refusing it freezes the surface for as long as the layer is recorded (see
//! the [`schedule`](super) module header on what a refused frame costs). The
//! reference sparse-strips renderer takes exactly that refusal: its
//! `LayersConfig::required_intermediate_texture_size` answers
//! `IntermediateTextureError::TooLarge` for a scene past the device limit
//! (`vello_hybrid`'s `render/common.rs:147-161`), and a root-level blend asks
//! it for the *whole scene's* size (`:181-183`), so a wide enough window has
//! no intermediate it can be served from at all.
//!
//! [`page_bands`] is the answer instead: the layer is cut into full-height
//! **column bands** no wider than the ceiling, each an ordinary page rendered
//! at its own origin and composited back at its own rectangle (E14). Three
//! properties make that a page decision rather than a new kind of target.
//!
//! - **Columns only.** A band spans the layer's whole height, so a layer
//!   *taller* than the ceiling is still refused. That is deliberate rather
//!   than pending: bands multiply passes over the layer's own draw list, and
//!   splitting one axis is what covers a display — which is wide before it is
//!   tall — at one pass per band instead of one per tile.
//! - **Evenly split, so the bands share one page.** The band count is what the
//!   ceiling forces (`width.div_ceil(ceiling)`), but the width is then divided
//!   *evenly* across that many bands rather than filling each to the ceiling
//!   and leaving a narrow tail. Every band therefore asks for one extent, which
//!   quantizes to one substrate-pool key (E13), so a banded layer costs the
//!   pool a single texture reused band after band instead of one wide entry
//!   plus an odd-sized tail — and its peak intermediate memory is the even
//!   band's, not the ceiling's.
//! - **Bounded.** [`MAX_PAGE_BANDS`] bands is where a split stops being cheaper
//!   than a refusal, and past it the layer is refused exactly as an
//!   over-ceiling one always was.
//!
//! What this module does not do is decide *when* a layer is banded: that call
//! belongs to the scheduler (`schedule::band_rounds`), reached only for a
//! *regular* layer that has not already been handed a page ahead of time (a
//! cut ancestor reserves one this way) and whose own accumulated ops hold no
//! composite of a nested isolated child — a band replays the layer's own
//! draws once per band, and replaying a child's composite would read a page
//! a later band has already reused. Every other over-ceiling shape, and a
//! layer taller than the ceiling on any axis, still refuses the frame exactly
//! as [`page_size`] always has.

use frust_gpu::TierCaps;
use vello_common::geometry::RectU16;

use crate::error::EngineError;
use crate::filters::blur::FILTER_ATLAS_PADDING;
use crate::gpu::targets::max_texture_size;

/// Smallest page the scheduler asks for, per axis.
///
/// A conservative floor: large enough that a small layer does not allocate a
/// texture of its own on every frame it resizes, small enough that a page is
/// never a memory decision on a mobile GPU.
pub const DEFAULT_MIN_PAGE_SIZE: u32 = 512;

/// Largest page the scheduler asks for, per axis, before the adapter's own
/// ceiling is applied.
pub const DEFAULT_MAX_PAGE_SIZE: u32 = 4096;

/// The most column bands one layer is split into by [`page_bands`].
///
/// Eight bands at the default ceiling span 32768 device pixels — half the
/// widest device grid the strip pipeline can address at all (`u16`
/// coordinates, E18) and several times any surface a shell configures. The
/// count is bounded because each band costs a render pass over the layer's own
/// draw list: an unbounded split would turn one pathological layer into an
/// unbounded number of passes, which is a worse answer than the refusal it
/// replaced.
pub const MAX_PAGE_BANDS: usize = 8;

/// Which texture group a page comes from: one of the two ping-pong groups, or
/// the single spill page beside them.
///
/// The scheduler keeps at most one page live per group, so a group *is* a page
/// identity for the shapes it serves; a schedule that would need two pages of
/// the same group is escalated rather than given a second index.
///
/// The name is the pair's: [`Even`](Self::Even) and [`Odd`](Self::Odd) are what
/// a layer's own depth parity names, and they carry every page of every chain
/// and every fan. [`Spill`](Self::Spill) is not a parity and is never derived
/// from a depth — it is the bounded third page the scheduler falls back on for
/// the one shape the pair cannot hold, kept in this enum rather than beside it
/// because what the renderer needs from all three is the same thing: an index
/// naming which live page a round writes, samples and hands back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageParity {
    /// The group even-depth layers render into.
    Even,
    /// The group odd-depth layers render into.
    Odd,
    /// The one spill page, taken only when both parity groups are held and no
    /// open round could be cut to hand one back.
    ///
    /// One page, never a group of its own: a second layer wanting it while it
    /// is held is refused, which is what keeps a frame's live intermediates at
    /// [`MAX_LIVE_PAGES`](super::MAX_LIVE_PAGES). It is released exactly as a
    /// parity page is — by the round whose composite sampled it — and only a
    /// regular layer is ever handed one (see [`schedule`](super)'s *The spill
    /// page*).
    Spill,
}

impl PageParity {
    /// The group a layer at `depth` renders into.
    ///
    /// Depths are one-based (the outermost isolated layer is depth 1), matching
    /// the recorder's own numbering, so the outermost layer takes the odd group
    /// and the surface it composites onto is not a page at all. Never answers
    /// [`Spill`](Self::Spill): the spill page is a fallback the scheduler
    /// reaches for explicitly, not a group any depth prefers.
    #[must_use]
    pub const fn from_depth(depth: usize) -> Self {
        if depth.is_multiple_of(2) {
            Self::Even
        } else {
            Self::Odd
        }
    }

    /// The other of the ping-pong pair.
    ///
    /// [`Spill`](Self::Spill) is not one of the pair and so has no other: it
    /// answers itself, which keeps this total without inventing a third
    /// ping-pong partner. Nothing asks: the one caller that ping-pongs between
    /// two groups of its own is a filter layer's pass sequence, and a filter
    /// layer is never handed the spill page.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Even => Self::Odd,
            Self::Odd => Self::Even,
            Self::Spill => Self::Spill,
        }
    }

    /// The group's index: `0` for even, `1` for odd, `2` for the spill page.
    ///
    /// This is what indexes the renderer's live-page slots, so the values are
    /// dense and stay inside [`MAX_LIVE_PAGES`](super::MAX_LIVE_PAGES).
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Even => 0,
            Self::Odd => 1,
            Self::Spill => 2,
        }
    }
}

/// The extent a page is acquired at, in texels.
///
/// Already floored, capped and quantized — this is what
/// [`IntermediateTargets::acquire`](crate::gpu::targets::IntermediateTargets::acquire)
/// is called with, not the layer's own bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageSize {
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
}

/// One full-height column of a layer, and the page it renders into.
///
/// A band is an ordinary page in every respect but its width: its contents are
/// rendered at the page's own origin (so every strip in it is offset by
/// `-(bounds.x0, bounds.y0)`, exactly as an unbanded layer's are) and it
/// composites back at [`bounds`](Self::bounds) in the parent's own
/// coordinates. A layer that fits one page is one band covering the whole of
/// it, so a caller has no second shape to handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageBand {
    /// The band's own tile-aligned device-space rectangle — a full-height
    /// column of the layer's bounds, and what the band composites at.
    pub bounds: RectU16,
    /// The extent this band's page is acquired at.
    ///
    /// One value for every band of a layer, the narrower last one included:
    /// the bands are split evenly and sized from the widest of them, so they
    /// share one substrate-pool key and reuse one texture (E13).
    pub size: PageSize,
}

/// The bounds page sizing works between.
///
/// Separate from the adapter's capabilities because both halves are policy: the
/// floor trades memory for fewer allocations, the ceiling trades scenes the
/// scheduler will serve for a bound on peak memory. The adapter's own limit is
/// applied on top of the ceiling and can only lower it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageConfig {
    /// Smallest page to ask for, per axis.
    pub min_page_size: u32,
    /// Largest page to ask for, per axis, before the adapter's ceiling.
    pub max_page_size: u32,
}

impl Default for PageConfig {
    fn default() -> Self {
        Self {
            min_page_size: DEFAULT_MIN_PAGE_SIZE,
            max_page_size: DEFAULT_MAX_PAGE_SIZE,
        }
    }
}

/// The largest page `config` will ask `caps`' adapter for, per axis.
///
/// The smaller of the configured ceiling and what the engine's intermediate
/// pool will allocate at all, so a page can never be sized past the extent the
/// pool would refuse.
#[must_use]
pub fn page_ceiling(config: &PageConfig, caps: &TierCaps) -> u32 {
    config.max_page_size.min(max_texture_size(caps))
}

/// The extent to acquire a page holding `bounds` at.
///
/// `bounds` is the layer's tile-aligned device-space rectangle; the layer is
/// rendered into the page at its origin, so only the extent matters here.
///
/// # Errors
///
/// [`EngineError::IntermediateTextureTooLarge`] when either axis of `bounds`
/// exceeds [`page_ceiling`]. Shrinking such a layer to the ceiling would drop
/// its outer pixels without saying so, which is why the frame is refused (and
/// skipped by the caller) instead; [`page_bands`] is the answer that renders
/// an over-wide layer rather than refusing it, and this function is the
/// single-page sizing it falls back on for a band.
pub fn page_size(
    bounds: RectU16,
    config: &PageConfig,
    caps: &TierCaps,
) -> Result<PageSize, EngineError> {
    let ceiling = page_ceiling(config, caps);
    let width = u32::from(bounds.width());
    let height = u32::from(bounds.height());

    if width > ceiling || height > ceiling {
        return Err(EngineError::IntermediateTextureTooLarge);
    }

    // The floor is clamped to the ceiling first: a configuration whose floor
    // sits above what the adapter allows must not turn every layer into an
    // over-ceiling request.
    let floor = config.min_page_size.min(ceiling);
    let (width, height) =
        frust_gpu::pool::quantize_extent(width.max(floor), height.max(floor), ceiling);

    Ok(PageSize { width, height })
}

/// The column bands `bounds` renders as, and the page extent each is acquired
/// at.
///
/// One band holding the whole layer whenever it fits a single page — which is
/// every layer a phone or a laptop surface records — and otherwise the
/// narrowest even split of its width that stays inside [`page_ceiling`]. The
/// bands tile `bounds` exactly: they abut, none overlaps, and their union is
/// the layer's own rectangle, so compositing them in order paints precisely
/// what one page would have (E14). See the module header's *A fourth decision*
/// section for why the split is by column, why it is even rather than greedy,
/// and what still has to happen for a banded layer to reach a device.
///
/// # Errors
///
/// [`EngineError::IntermediateTextureTooLarge`] when the layer is *taller*
/// than [`page_ceiling`] — bands are columns, so height has no split to be
/// served by — and when its width would need more than [`MAX_PAGE_BANDS`]
/// bands. Both are the refusal [`page_size`] gives an over-ceiling layer, kept
/// for the cases banding does not reach rather than replaced by a partial
/// answer, and neither is a panic (E17).
pub fn page_bands(
    bounds: RectU16,
    config: &PageConfig,
    caps: &TierCaps,
) -> Result<Vec<PageBand>, EngineError> {
    let ceiling = page_ceiling(config, caps);
    let width = u32::from(bounds.width());

    if u32::from(bounds.height()) > ceiling {
        return Err(EngineError::IntermediateTextureTooLarge);
    }
    if width <= ceiling {
        return Ok(vec![PageBand {
            bounds,
            size: page_size(bounds, config, caps)?,
        }]);
    }

    // `width > ceiling >= 0` here, so the ceiling is at least one and the
    // division below is well defined however a caller configured this pool.
    let count = width.div_ceil(ceiling.max(1));
    if count > u32::try_from(MAX_PAGE_BANDS).unwrap_or(u32::MAX) {
        return Err(EngineError::IntermediateTextureTooLarge);
    }

    // Even rather than greedy: `count` bands of this width cover the layer and
    // none of them exceeds the ceiling, and sizing every page from the widest
    // one gives the whole split a single pool key.
    let band_width = width.div_ceil(count);
    let size = page_size(
        RectU16::new(
            0,
            0,
            u16::try_from(band_width).unwrap_or(u16::MAX),
            bounds.height(),
        ),
        config,
        caps,
    )?;

    let end = u32::from(bounds.x1);
    let mut bands = Vec::with_capacity(count as usize);
    let mut x0 = u32::from(bounds.x0);
    while x0 < end {
        let x1 = end.min(x0.saturating_add(band_width));
        bands.push(PageBand {
            bounds: RectU16::new(
                u16::try_from(x0).unwrap_or(u16::MAX),
                bounds.y0,
                u16::try_from(x1).unwrap_or(u16::MAX),
                bounds.y1,
            ),
            size,
        });
        x0 = x1;
    }

    Ok(bands)
}

/// The extent to acquire a filter layer's page at.
///
/// `bounds` is the layer's own tile-aligned device-space rectangle — already
/// grown by the filter's own visual spread (a blur's 3σ, a drop shadow's
/// offset plus its own blur), the same `bounds` [`page_size`] would take for
/// a regular layer. This function grows it by [`FILTER_ATLAS_PADDING`] twice
/// per axis before flooring, quantizing (E13) and capping, and checks the
/// padded request against two ceilings rather than one — see the module
/// header's *A third decision* section for why both differences exist, and for
/// why the growth is not a margin "on every side".
///
/// # Errors
///
/// [`EngineError::IntermediateTextureTooLarge`] when the padded extent
/// exceeds [`max_texture_size`] on either axis — this adapter will never
/// allocate a page that large, the same refusal an over-ceiling regular layer
/// gets from [`page_size`]. [`EngineError::IntermediateTextureLimitReached`]
/// when the padded extent stays inside that hard ceiling but still exceeds
/// [`PageConfig::max_page_size`] — the adapter could serve it, but this
/// pool's own configured budget does not. Neither is a panic (E17).
pub fn filter_page_size(
    bounds: RectU16,
    config: &PageConfig,
    caps: &TierCaps,
) -> Result<PageSize, EngineError> {
    let device_ceiling = max_texture_size(caps);
    let budget_ceiling = config.max_page_size;

    // `u32` throughout: `bounds`' axes are `u16`, so even a `bounds` at the
    // `u16` ceiling grows into `u32` headroom under the padding below rather
    // than wrapping.
    let padding = u32::from(FILTER_ATLAS_PADDING).saturating_mul(2);
    let width = u32::from(bounds.width()).saturating_add(padding);
    let height = u32::from(bounds.height()).saturating_add(padding);

    if width > device_ceiling || height > device_ceiling {
        return Err(EngineError::IntermediateTextureTooLarge);
    }
    if width > budget_ceiling || height > budget_ceiling {
        return Err(EngineError::IntermediateTextureLimitReached);
    }

    // The floor is clamped to the tighter of the two ceilings first, exactly
    // as `page_size` clamps it to `page_ceiling`: a configuration whose floor
    // sits above what either ceiling allows must not turn every filter layer
    // into a refused request.
    let ceiling = budget_ceiling.min(device_ceiling);
    let floor = config.min_page_size.min(ceiling);
    let (width, height) =
        frust_gpu::pool::quantize_extent(width.max(floor), height.max(floor), ceiling);

    Ok(PageSize { width, height })
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_gpu::DownlevelProfile;

    fn caps() -> TierCaps {
        TierCaps::fake(DownlevelProfile::Full)
    }

    #[test]
    fn parity_alternates_down_a_chain_and_starts_odd_at_the_outermost_layer() {
        assert_eq!(PageParity::from_depth(1), PageParity::Odd);
        assert_eq!(PageParity::from_depth(2), PageParity::Even);
        assert_eq!(PageParity::from_depth(3), PageParity::Odd);
        assert_eq!(PageParity::from_depth(4), PageParity::Even);

        assert_eq!(PageParity::Even.opposite(), PageParity::Odd);
        assert_eq!(PageParity::Odd.opposite(), PageParity::Even);
        assert_eq!(PageParity::Even.index(), 0);
        assert_eq!(PageParity::Odd.index(), 1);
    }

    #[test]
    fn the_spill_page_is_no_depths_group_and_indexes_past_the_ping_pong_pair() {
        // A depth never names it — it is reached by falling back, not by
        // preferring — and its index is the third live-page slot, which is
        // what keeps the renderer's own array indexable by this value alone.
        for depth in 0..=16 {
            assert_ne!(PageParity::from_depth(depth), PageParity::Spill);
        }
        assert_eq!(PageParity::Spill.index(), 2);
        assert!(PageParity::Spill.index() < crate::schedule::MAX_LIVE_PAGES);
        assert_eq!(
            PageParity::Spill.opposite(),
            PageParity::Spill,
            "the spill page is not one of the ping-pong pair, so it has no other"
        );
    }

    #[test]
    fn a_small_layer_is_floored_at_the_minimum_page_size() {
        let size = page_size(RectU16::new(0, 0, 12, 8), &PageConfig::default(), &caps())
            .expect("a 12x8 layer is far inside the ceiling");
        assert_eq!(
            size,
            PageSize {
                width: DEFAULT_MIN_PAGE_SIZE,
                height: DEFAULT_MIN_PAGE_SIZE
            }
        );
    }

    #[test]
    fn a_larger_layer_is_quantized_up_to_the_pool_grid() {
        let size = page_size(
            RectU16::new(0, 0, 900, 700),
            &PageConfig::default(),
            &caps(),
        )
        .expect("a 900x700 layer is inside the ceiling");
        // The substrate pool keys on a 256-texel grid, so both axes round up to
        // it rather than to the request.
        assert_eq!(
            size,
            PageSize {
                width: 1024,
                height: 768
            }
        );
    }

    #[test]
    fn a_layer_past_the_ceiling_is_refused_rather_than_shrunk() {
        let config = PageConfig {
            min_page_size: DEFAULT_MIN_PAGE_SIZE,
            max_page_size: 1024,
        };
        let ceiling = page_ceiling(&config, &caps());
        assert_eq!(ceiling, 1024);

        assert!(matches!(
            page_size(RectU16::new(0, 0, 1025, 16), &config, &caps()),
            Err(EngineError::IntermediateTextureTooLarge)
        ));
        assert!(matches!(
            page_size(RectU16::new(0, 0, 16, 1025), &config, &caps()),
            Err(EngineError::IntermediateTextureTooLarge)
        ));
        assert!(page_size(RectU16::new(0, 0, 1024, 1024), &config, &caps()).is_ok());
    }

    #[test]
    fn a_floor_above_the_ceiling_still_produces_a_page_inside_it() {
        let config = PageConfig {
            min_page_size: 4096,
            max_page_size: 512,
        };
        let size = page_size(RectU16::new(0, 0, 8, 8), &config, &caps())
            .expect("the floor is clamped to the ceiling, not applied over it");
        assert_eq!(
            size,
            PageSize {
                width: 512,
                height: 512
            }
        );
    }

    #[test]
    fn the_adapter_limit_can_only_lower_the_configured_ceiling() {
        let mut caps = caps();
        caps.max_texture_dimension_2d = 2048;
        let config = PageConfig::default();
        assert_eq!(page_ceiling(&config, &caps), 2048);

        caps.max_texture_dimension_2d = 16384;
        assert_eq!(page_ceiling(&config, &caps), DEFAULT_MAX_PAGE_SIZE);
    }

    /// The bands cover `bounds` exactly: they start at its left edge, abut
    /// with no gap and no overlap, end at its right edge, and every one of
    /// them spans its full height.
    fn assert_tiles(bands: &[PageBand], bounds: RectU16) {
        let mut x = bounds.x0;
        for band in bands {
            assert_eq!(band.bounds.x0, x, "bands abut with no gap and no overlap");
            assert!(band.bounds.x1 > band.bounds.x0, "no band is degenerate");
            assert_eq!(band.bounds.y0, bounds.y0, "a band spans the full height");
            assert_eq!(band.bounds.y1, bounds.y1, "a band spans the full height");
            x = band.bounds.x1;
        }
        assert_eq!(x, bounds.x1, "the bands end exactly at the layer's edge");
    }

    #[test]
    fn a_layer_that_fits_one_page_is_a_single_band_holding_all_of_it() {
        let bounds = RectU16::new(0, 0, 900, 700);
        let config = PageConfig::default();
        let bands = page_bands(bounds, &config, &caps()).expect("900x700 fits one page");

        assert_eq!(bands.len(), 1);
        assert_eq!(bands[0].bounds, bounds);
        assert_eq!(
            bands[0].size,
            page_size(bounds, &config, &caps()).expect("the same layer sizes as one page"),
            "an unbanded layer's band is sized exactly as `page_size` sizes it"
        );
        assert_tiles(&bands, bounds);
    }

    #[test]
    fn a_5k_layer_splits_into_two_equal_bands_sharing_one_page_extent() {
        // The desktop case: a 5120x2880 surface under one root opacity layer,
        // 1024 texels past the default 4096 ceiling.
        let bounds = RectU16::new(0, 0, 5120, 2880);
        let bands =
            page_bands(bounds, &PageConfig::default(), &caps()).expect("a 5K layer is banded");

        assert_eq!(bands.len(), 2, "5120 needs two bands under a 4096 ceiling");
        assert_tiles(&bands, bounds);
        assert_eq!(bands[0].bounds, RectU16::new(0, 0, 2560, 2880));
        assert_eq!(bands[1].bounds, RectU16::new(2560, 0, 5120, 2880));

        // Split evenly rather than greedily, so both bands quantize to one
        // pool key — 2560 is already on the 256 grid, 2880 rounds up to 3072.
        assert_eq!(
            bands[0].size,
            PageSize {
                width: 2560,
                height: 3072
            }
        );
        assert_eq!(
            bands[0].size, bands[1].size,
            "every band of a layer asks the pool for one extent"
        );
    }

    #[test]
    fn an_uneven_width_gives_a_narrower_last_band_at_the_same_page_extent() {
        // Three bands under a 1024 ceiling, and 2500 does not divide by three:
        // the first two take 834 and the last one 832.
        let config = PageConfig {
            min_page_size: 256,
            max_page_size: 1024,
        };
        let bounds = RectU16::new(0, 0, 2500, 600);
        let bands = page_bands(bounds, &config, &caps()).expect("2500 needs three bands");

        assert_eq!(bands.len(), 3);
        assert_tiles(&bands, bounds);
        assert_eq!(bands[0].bounds.width(), 834);
        assert_eq!(bands[1].bounds.width(), 834);
        assert_eq!(bands[2].bounds.width(), 832, "the last band takes the rest");
        assert!(
            bands
                .iter()
                .all(|band| band.size == bands[0].size && band.size.width <= config.max_page_size),
            "the short band is still sized from the widest one, and none exceeds the ceiling"
        );
    }

    #[test]
    fn bands_are_offset_by_the_layers_own_origin_rather_than_starting_at_zero() {
        let config = PageConfig {
            min_page_size: 256,
            max_page_size: 512,
        };
        let bounds = RectU16::new(100, 40, 1100, 300);
        let bands = page_bands(bounds, &config, &caps()).expect("a 1000-wide layer needs two");

        assert_eq!(bands.len(), 2);
        assert_tiles(&bands, bounds);
        assert_eq!(bands[0].bounds, RectU16::new(100, 40, 600, 300));
        assert_eq!(bands[1].bounds, RectU16::new(600, 40, 1100, 300));
    }

    #[test]
    fn a_layer_taller_than_the_ceiling_is_still_refused_because_bands_are_columns() {
        let config = PageConfig {
            min_page_size: 256,
            max_page_size: 1024,
        };

        assert!(page_bands(RectU16::new(0, 0, 512, 1024), &config, &caps()).is_ok());
        assert!(matches!(
            page_bands(RectU16::new(0, 0, 512, 1025), &config, &caps()),
            Err(EngineError::IntermediateTextureTooLarge)
        ));
        // Width past the ceiling is served; height past it is not, and a layer
        // over on both axes takes the height refusal.
        assert!(matches!(
            page_bands(RectU16::new(0, 0, 4096, 1025), &config, &caps()),
            Err(EngineError::IntermediateTextureTooLarge)
        ));
    }

    #[test]
    fn a_width_needing_more_than_the_band_bound_is_refused_rather_than_split_further() {
        let config = PageConfig {
            min_page_size: 256,
            max_page_size: 1024,
        };
        let ceiling = page_ceiling(&config, &caps());
        let at_bound = ceiling * MAX_PAGE_BANDS as u32;

        let bands = page_bands(RectU16::new(0, 0, at_bound as u16, 64), &config, &caps())
            .expect("exactly the bound is served");
        assert_eq!(bands.len(), MAX_PAGE_BANDS);

        assert!(matches!(
            page_bands(
                RectU16::new(0, 0, (at_bound + 1) as u16, 64),
                &config,
                &caps()
            ),
            Err(EngineError::IntermediateTextureTooLarge)
        ));
    }

    #[test]
    fn every_band_of_a_layer_is_a_page_the_pool_would_accept() {
        // Whatever the split, no band may ask for an extent `page_size` itself
        // would refuse — that is what makes a band an ordinary page.
        let config = PageConfig::default();
        let caps = caps();
        let ceiling = page_ceiling(&config, &caps);

        for width in [4097_u32, 5120, 6000, 8192, 12288, 32768] {
            let bounds = RectU16::new(0, 0, width as u16, 2880);
            let bands = page_bands(bounds, &config, &caps).expect("inside the band bound");
            assert_tiles(&bands, bounds);
            for band in &bands {
                assert!(band.bounds.width() as u32 <= ceiling);
                assert_eq!(
                    band.size,
                    page_size(
                        RectU16::new(0, 0, bands[0].bounds.width(), bounds.height()),
                        &config,
                        &caps
                    )
                    .expect("a band is inside the ceiling by construction")
                );
            }
        }
    }

    #[test]
    fn a_small_filter_layer_is_padded_then_still_floored_at_the_minimum_page_size() {
        // 12x8 plus padding on every side (24x20) is still far inside the
        // floor, exactly as the unpadded case is for `page_size`.
        let size = filter_page_size(RectU16::new(0, 0, 12, 8), &PageConfig::default(), &caps())
            .expect("a padded 24x20 request is far inside the ceiling");
        assert_eq!(
            size,
            PageSize {
                width: DEFAULT_MIN_PAGE_SIZE,
                height: DEFAULT_MIN_PAGE_SIZE
            }
        );
    }

    #[test]
    fn filter_padding_can_push_a_borderline_layer_into_the_next_quantum() {
        // Unpadded, 1013 rounds up to 1024 and 700 to 768 (`page_size`'s own
        // grid); the extra 12 texels of padding on each axis is what carries
        // the request past 1024 into the next 256-wide bucket.
        let size = filter_page_size(
            RectU16::new(0, 0, 1013, 700),
            &PageConfig::default(),
            &caps(),
        )
        .expect("1025x712 padded is inside the default ceiling");
        assert_eq!(
            size,
            PageSize {
                width: 1280,
                height: 768
            }
        );
    }

    #[test]
    fn a_filter_layer_whose_padding_carries_it_past_the_device_ceiling_is_too_large() {
        // A budget at least as large as the device ceiling isolates the
        // device-capability refusal from the pool-budget one below.
        let config = PageConfig {
            min_page_size: DEFAULT_MIN_PAGE_SIZE,
            max_page_size: 8192,
        };
        let caps = caps();
        let device_ceiling = max_texture_size(&caps);
        assert_eq!(device_ceiling, 8192);

        // Exactly at the ceiling once padded: still served.
        let at_ceiling = device_ceiling - u32::from(FILTER_ATLAS_PADDING) * 2;
        assert!(
            filter_page_size(RectU16::new(0, 0, at_ceiling as u16, 16), &config, &caps).is_ok()
        );

        // One texel past it once padded — the padding is what pushes this
        // request over, not the bounds alone.
        let over_ceiling = at_ceiling + 1;
        assert!(matches!(
            filter_page_size(RectU16::new(0, 0, over_ceiling as u16, 16), &config, &caps),
            Err(EngineError::IntermediateTextureTooLarge)
        ));
        assert!(matches!(
            filter_page_size(RectU16::new(0, 0, 16, over_ceiling as u16), &config, &caps),
            Err(EngineError::IntermediateTextureTooLarge)
        ));
    }

    #[test]
    fn a_filter_layer_inside_the_device_ceiling_but_past_the_pool_budget_is_limit_reached() {
        // The default budget (4096) sits well under the default caps' device
        // ceiling (8192), so a request that overflows only the former is
        // distinguishable from one that overflows the latter.
        let config = PageConfig::default();
        let caps = caps();
        assert!(config.max_page_size < max_texture_size(&caps));

        let over_budget = config.max_page_size - u32::from(FILTER_ATLAS_PADDING) * 2 + 1;
        assert!(matches!(
            filter_page_size(RectU16::new(0, 0, over_budget as u16, 16), &config, &caps),
            Err(EngineError::IntermediateTextureLimitReached)
        ));
        assert!(matches!(
            filter_page_size(RectU16::new(0, 0, 16, over_budget as u16), &config, &caps),
            Err(EngineError::IntermediateTextureLimitReached)
        ));
    }

    #[test]
    fn filter_page_size_never_overflows_padding_a_bounds_near_the_u16_ceiling() {
        // `bounds` is `u16`-addressed, so its axes can sit right at 65535;
        // the padded width/height are computed in `u32`, so this refuses as
        // an ordinary over-ceiling request rather than wrapping or panicking.
        let size = filter_page_size(
            RectU16::new(0, 0, u16::MAX, u16::MAX),
            &PageConfig::default(),
            &caps(),
        );
        assert!(matches!(
            size,
            Err(EngineError::IntermediateTextureTooLarge)
        ));
    }
}
