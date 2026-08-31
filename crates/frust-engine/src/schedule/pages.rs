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
//!   [`schedule`](super) module header).
//! - **Coverage-sized, quantized, capped.** A page is sized to the layer's own
//!   tile-aligned bounds rather than to the viewport, floored at
//!   [`PageConfig::min_page_size`] and quantized to the substrate pool's key
//!   grid so a resizing layer reuses one texture instead of churning a new one
//!   per frame. It is capped at the smaller of [`PageConfig::max_page_size`]
//!   and what the adapter will allocate; a layer larger than that is refused
//!   with [`EngineError::IntermediateTextureTooLarge`] rather than clipped to
//!   the cap, which would silently drop the layer's outer pixels.
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
//! - **Padded, not just coverage-sized.** A filter pass's bilinear kernel taps
//!   can reach [`FILTER_ATLAS_PADDING`] texels past the region it writes (see
//!   that constant's own doc), and while the fragment stage bounds that
//!   overdraw at the layer's own extent, a page whose real allocation lands
//!   *exactly* on that extent — which the pool's quantization can and does
//!   produce, whenever a layer's own size already sits on the quantum grid —
//!   would put the overdraw's edge at the very last texel rather than inside
//!   a transparent margin. [`filter_page_size`] reserves that margin by
//!   growing the request by [`FILTER_ATLAS_PADDING`] on every side *before*
//!   flooring, quantizing (E13) and capping, so the page's real extent always
//!   has the margin regardless of where the quantum grid happens to land.
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

/// Which of the two ping-pong texture groups a page comes from.
///
/// The scheduler keeps at most one page live per group, so a parity *is* a page
/// identity for the shapes it serves; a schedule that would need two pages of
/// the same parity is escalated rather than given a second index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageParity {
    /// The group even-depth layers render into.
    Even,
    /// The group odd-depth layers render into.
    Odd,
}

impl PageParity {
    /// The group a layer at `depth` renders into.
    ///
    /// Depths are one-based (the outermost isolated layer is depth 1), matching
    /// the recorder's own numbering, so the outermost layer takes the odd group
    /// and the surface it composites onto is not a page at all.
    #[must_use]
    pub const fn from_depth(depth: usize) -> Self {
        if depth.is_multiple_of(2) {
            Self::Even
        } else {
            Self::Odd
        }
    }

    /// The other group.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Even => Self::Odd,
            Self::Odd => Self::Even,
        }
    }

    /// The group's index, `0` for even and `1` for odd.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Even => 0,
            Self::Odd => 1,
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
/// exceeds [`page_ceiling`]. Splitting such a layer into bands is later work,
/// and shrinking it to the ceiling would drop its outer pixels without saying
/// so, which is why the frame is refused (and skipped by the caller) instead.
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

/// The extent to acquire a filter layer's page at.
///
/// `bounds` is the layer's own tile-aligned device-space rectangle — already
/// grown by the filter's own visual spread (a blur's 3σ, a drop shadow's
/// offset plus its own blur), the same `bounds` [`page_size`] would take for
/// a regular layer. This function grows it by [`FILTER_ATLAS_PADDING`] on
/// every side before flooring, quantizing (E13) and capping, and checks the
/// padded request against two ceilings rather than one — see the module
/// header's *A third decision* section for why both differences exist.
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
