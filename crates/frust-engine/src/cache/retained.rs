//! Retained pages: bodies the renderer rasterized once and keeps across
//! frames, so a later frame can composite them instead of painting them again.
//!
//! A `PushRetained` bracket's body is an ordinary layer of the frame — the
//! compiler records it as one whatever its alpha (see
//! [`crate::compile::layers`]) — and this store decides, per frame, whether
//! that layer renders into a pooled page like any other (the inline lowering)
//! or into a page the store owns and keeps. A `DrawRetained` names a page by
//! key; the store resolves it to a composite of that page, or refuses the
//! frame when it cannot.
//!
//! ## What a page holds, and what it can be reused for
//!
//! A page holds the body exactly as the inline lowering rasterizes it: on the
//! device grid, under the bracket's own transform with its presentation scale
//! already applied, at the layer's own tile-aligned bounds, and **without** the
//! bracket's alpha — the alpha is the composite's, so a page fades at no cost.
//! It is drawn through the same 1:1 [`PaintClass::LayerComposite`] quad a
//! pooled page is, never resampled, which is what makes a retained frame
//! pixel-identical to the inline one and also bounds what a page can serve: a
//! `DrawRetained` whose [`RetainedPlacement`] has the page's linear part
//! (scale, rotation, skew) and whose translation differs by any amount is
//! composited at the page's bounds moved by that translation rounded to whole
//! pixels; anything else — another scale, another extent — is not resident at
//! that placement, which is a refusal (below). The resident-key ledger reports
//! each page's extent and scale *as rasterized*, presentation scale included,
//! so a widget asking for the extent it is about to draw at only ever asks for
//! a page that can serve it.
//!
//! [`PaintClass::LayerComposite`]: crate::gpu::strips::PaintClass::LayerComposite
//!
//! ## Outside the pool, outside the page budget
//!
//! A page is this store's own allocation (`gpu::targets::retained_page_descriptor`),
//! never a pooled entry: the pool ages and re-lets textures frame by frame,
//! and a retained page's whole point is to survive across frames. For the same
//! reason it is **not** one of the scheduler's live pages: a round rendering a
//! body into its retained page, and the composite that later samples it, take
//! nothing from the two ping-pong groups or the spill page
//! ([`MAX_LIVE_PAGES`](crate::schedule::MAX_LIVE_PAGES)) — the body's own nested
//! translucent layers still do, exactly as they would inside any layer.
//!
//! ## Budget, eviction and refusals
//!
//! The store is held to a byte budget ([`RetainedBudget`]) tiered by adapter
//! family the way the image atlas is (`cache::images::choose_tier`): a phone
//! keeps a few full-screen pages, a desktop a few more. A body that needs a
//! page evicts the least recently used pages until it fits, never one the frame
//! being planned draws or records. A body is refused — rendered inline this
//! frame, its key absent from the ledger — when the compiler marked it
//! unretainable (clipped on the way in, a hole punch or an external texture
//! inside it, an unbalanced bracket), when it drew nothing, drew outside its
//! own rectangle, is larger than [`MAX_INTERMEDIATE_DIMENSION`] or the
//! adapter's ceiling on either axis, needs more than the whole budget, or names
//! a key the same frame already records or draws.
//!
//! A `DrawRetained` the store cannot serve is a renderer error the display
//! list's own contract anticipates: the frame is refused with
//! [`EngineError::RetainedKeyMissing`] before anything is recorded, and the
//! store is cleared, so the ledger published after it is empty and the next
//! frame paints every body again.
//!
//! ## Plan, then commit
//!
//! [`RetainedPages::plan`] decides everything from plain values and changes
//! nothing of the store but its frame clock (and, on a refusal, its contents) —
//! of the frame, only the bounds of the recorded layers enclosing a served
//! `DrawRetained`, grown to hold its composite, since the empty layer standing
//! for it put nothing of it into theirs:
//! a frame can still be refused after it — by the scheduler, or a capacity
//! check — and a refused frame must leave every page and every ledger entry as
//! it was. [`RetainedPages::commit`] applies a plan once nothing can fail any
//! more: evictions first, then each admitted body's page — reused when the
//! key's current page is large enough, allocated otherwise — and only for a
//! body the schedule really rendered (a body inside a dropped layer renders
//! nothing, and a page claiming its key would composite stale texels).
//!
//! Generic over the texture and view types like the intermediate pool, so plan,
//! eviction and ledger behaviour is exercised against a counting fake
//! allocator with no GPU in the loop (`tests/retained.rs`).

use frust_gpu::{TextureAllocator, TierCaps};
use vello_common::geometry::RectU16;
use vello_common::record::RecordedLayer;
use vello_common::tile::Tile;

use crate::cache::images::{AtlasTier, choose_tier, device_memory_bytes};
use crate::compile::{RetainedBracket, RetainedKind, RetainedPlacement};
use crate::config;
use crate::error::EngineError;
use crate::gpu::targets::{MAX_INTERMEDIATE_DIMENSION, max_texture_size, retained_page_descriptor};

/// Bytes one retained-page texel costs — the intermediate format is
/// `Rgba8Unorm`.
pub const RETAINED_BYTES_PER_TEXEL: u64 = 4;

/// How many bytes of retained pages a store may hold at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetainedBudget {
    /// The ceiling, in bytes of texture.
    pub bytes: u64,
}

impl RetainedBudget {
    /// A phone's budget: 32 MiB, three 1080x2400 full-screen pages — what a
    /// navigator transition (the outgoing and the incoming page) needs with
    /// one to spare.
    pub const MOBILE: Self = Self { bytes: 32 << 20 };

    /// A phone with at least 4 GiB of memory: 64 MiB.
    pub const MOBILE_LARGE: Self = Self { bytes: 64 << 20 };

    /// A desktop-class adapter: 256 MiB, several 4K-window pages.
    pub const DESKTOP: Self = Self { bytes: 256 << 20 };

    /// The budget `tier` stands for — the image atlas's own tier read
    /// (`cache::images::choose_tier`), so one adapter answers the
    /// mobile-or-desktop question once for both caches.
    #[must_use]
    pub fn for_tier(tier: AtlasTier) -> Self {
        match tier {
            AtlasTier::Mobile => Self::MOBILE,
            AtlasTier::MobileLarge => Self::MOBILE_LARGE,
            AtlasTier::Desktop => Self::DESKTOP,
        }
    }

    /// The budget for `caps`' adapter on this device, or
    /// `FRUST_ENGINE_RETAINED_BUDGET_MB` when it is set.
    #[must_use]
    pub fn for_caps(caps: &TierCaps) -> Self {
        match config::retained_budget_mb() {
            Some(mib) => Self {
                bytes: mib.saturating_mul(1 << 20),
            },
            None => Self::for_tier(choose_tier(caps, device_memory_bytes()).tier),
        }
    }
}

/// Why a body was rendered inline rather than into a retained page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedRefusal {
    /// The compiler marked the body unretainable (see
    /// [`RetainedKind::Record`]).
    NotRetainable,
    /// The body drew nothing at all.
    Empty,
    /// The body drew outside its own rectangle.
    Overflow,
    /// The body's bounds exceed the largest page on either axis.
    TooLarge,
    /// The body's page needs more than the whole budget, or more than every
    /// evictable page frees.
    OverBudget,
    /// The frame already records or draws the same key.
    Duplicate,
}

/// What the store decided for one retained bracket of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedDecision {
    /// A body rendered inline — the ordinary layer its bracket lowers to.
    Inline(RetainedRefusal),
    /// A body rendered into the key's retained page, which holds `bounds`
    /// at its own origin.
    Record {
        /// The body's tile-aligned device bounds.
        bounds: RectU16,
    },
    /// A `DrawRetained` composited from the key's resident page.
    Draw {
        /// Where the composite lands, already trimmed to the command's window.
        dest: RectU16,
        /// The page texel under `dest`'s top-left corner.
        origin: (u16, u16),
    },
    /// A `DrawRetained` whose composite would land wholly outside its window:
    /// the page is resident and served, and draws nothing this frame.
    Hidden,
}

/// One frame's decisions, parallel to the frame's
/// [`CompiledFrame::retained`](crate::compile::CompiledFrame::retained), and
/// the keys committing it evicts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetainedPlan {
    /// One decision per retained bracket, in recording order.
    pub decisions: Vec<RetainedDecision>,
    /// The keys whose pages committing this plan drops, least recently used
    /// first.
    pub evictions: Vec<u64>,
}

/// One resident page as the resident-key ledger reports it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetainedEntry {
    /// The raw key the page was recorded under.
    pub key: u64,
    /// The extent the bracket's rectangle covered when the page was rendered,
    /// in device pixels.
    pub extent_px: (u32, u32),
    /// The scale the page was rasterized at — device scale and presentation
    /// scale together (see [`RetainedPlacement::scale`]).
    pub scale: f32,
}

/// One page the store holds.
#[derive(Debug)]
struct RetainedPage<T, V> {
    key: u64,
    /// The body's tile-aligned device bounds when it was rendered; the page
    /// holds them at its own origin.
    bounds: RectU16,
    /// Where the body's local space landed when it was rendered.
    placement: RetainedPlacement,
    /// The ledger's extent.
    extent_px: (u32, u32),
    /// The texture's own extent, at least `bounds`' on both axes.
    size: (u32, u32),
    /// The store frame the page was last recorded or drawn on.
    last_used: u64,
    /// Kept alive beside its view; never read.
    _texture: T,
    view: V,
}

impl<T, V> RetainedPage<T, V> {
    fn bytes(&self) -> u64 {
        page_bytes(self.size)
    }

    fn entry(&self) -> RetainedEntry {
        RetainedEntry {
            key: self.key,
            extent_px: self.extent_px,
            scale: self.placement.scale(),
        }
    }
}

/// The renderer's retained pages: a budgeted, least-recently-used store keyed
/// by the raw retained key (see the module doc).
#[derive(Debug)]
pub struct RetainedPages<T = wgpu::Texture, V = wgpu::TextureView> {
    pages: Vec<RetainedPage<T, V>>,
    budget: RetainedBudget,
    /// The largest page extent per axis — the smaller of the adapter's ceiling
    /// and [`MAX_INTERMEDIATE_DIMENSION`].
    max_dimension: u32,
    /// Advanced once per [`Self::plan`]; what `last_used` is stamped with.
    frame: u64,
    /// Advanced whenever what the ledger reports changes.
    generation: u64,
    /// Per retained bracket of the last committed frame, the index of the page
    /// it renders into or composites from.
    bound: Vec<Option<usize>>,
    /// Whether a `DrawRetained` refusal cleared the store since the last
    /// [`Self::take_refusal`].
    refused: bool,
}

impl<T, V> RetainedPages<T, V> {
    /// An empty store held to `budget`, with pages capped at `max_dimension`
    /// per axis (and never above [`MAX_INTERMEDIATE_DIMENSION`]).
    #[must_use]
    pub fn new(budget: RetainedBudget, max_dimension: u32) -> Self {
        Self {
            pages: Vec::new(),
            budget,
            max_dimension: max_dimension.min(MAX_INTERMEDIATE_DIMENSION),
            frame: 0,
            generation: 0,
            bound: Vec::new(),
            refused: false,
        }
    }

    /// An empty store budgeted and capped for `caps`' adapter.
    #[must_use]
    pub fn for_caps(caps: &TierCaps) -> Self {
        Self::new(RetainedBudget::for_caps(caps), max_texture_size(caps))
    }

    /// The budget the store is held to.
    #[must_use]
    pub fn budget(&self) -> RetainedBudget {
        self.budget
    }

    /// Re-budget the store, evicting least recently used pages until what it
    /// holds fits.
    pub fn set_budget(&mut self, budget: RetainedBudget) {
        self.budget = budget;
        let mut order: Vec<(u64, u64)> = self
            .pages
            .iter()
            .map(|page| (page.last_used, page.key))
            .collect();
        order.sort_unstable();
        for (_, key) in order {
            if self.used_bytes() <= budget.bytes {
                break;
            }
            self.remove(key);
        }
    }

    /// The largest page extent the store allocates, per axis.
    #[must_use]
    pub fn max_dimension(&self) -> u32 {
        self.max_dimension
    }

    /// How many pages are resident.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pages.len()
    }

    /// Whether no page is resident.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// The bytes of texture the resident pages hold.
    #[must_use]
    pub fn used_bytes(&self) -> u64 {
        self.pages.iter().map(RetainedPage::bytes).sum()
    }

    /// Whether `key` has a resident page.
    #[must_use]
    pub fn contains(&self, key: u64) -> bool {
        self.position(key).is_some()
    }

    /// The store's frame clock: how many frames have been planned.
    #[must_use]
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// A counter advanced whenever the set [`Self::entries`] reports changes —
    /// a page added, evicted, re-rendered at another extent or scale, or the
    /// store cleared. A caller publishing the ledger only when this moved
    /// publishes every change and nothing else.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The resident pages, as the ledger reports them, in no particular order.
    pub fn entries(&self) -> impl Iterator<Item = RetainedEntry> + '_ {
        self.pages.iter().map(RetainedPage::entry)
    }

    /// Whether a `DrawRetained` refusal cleared the store since the last call,
    /// clearing the flag — what lets a host ask for one more frame to be
    /// painted against the emptied ledger.
    pub fn take_refusal(&mut self) -> bool {
        std::mem::take(&mut self.refused)
    }

    /// Drop every page.
    ///
    /// What a surface recreate, a resize and a `DrawRetained` refusal all do:
    /// the ledger the widget tree paints against goes empty with it, and every
    /// body is painted (and, where admitted, recorded) again.
    pub fn clear(&mut self) {
        if !self.pages.is_empty() {
            self.generation = self.generation.wrapping_add(1);
        }
        self.pages.clear();
        self.bound.clear();
    }

    /// The view and texture extent of the page retained bracket `bracket` of
    /// the last committed frame renders into or composites from.
    #[must_use]
    pub fn page_for(&self, bracket: u32) -> Option<(&V, (u32, u32))> {
        let index = (*self.bound.get(bracket as usize)?)?;
        let page = self.pages.get(index)?;
        Some((&page.view, page.size))
    }

    /// Decide one frame: which bodies render into a retained page, which
    /// `DrawRetained`s composite from one and where, and which pages make room
    /// (see the module doc). Advances the frame clock; changes nothing else
    /// unless it refuses.
    ///
    /// `layers` is the frame's recorded layers, for each body's drawn bounds;
    /// every layer enclosing a served draw has its bounds grown to hold the
    /// draw's composite (rounded out to tiles) before any body is sized.
    ///
    /// # Errors
    ///
    /// [`EngineError::RetainedKeyMissing`] for the first `DrawRetained` whose
    /// key is not resident at its placement — and the store is cleared.
    pub fn plan(
        &mut self,
        brackets: &[RetainedBracket],
        layers: &mut [RecordedLayer],
    ) -> Result<RetainedPlan, EngineError> {
        self.frame = self.frame.saturating_add(1);
        let mut decisions: Vec<Option<RetainedDecision>> = vec![None; brackets.len()];

        // Every draw first: one that cannot be served refuses the frame, and
        // nothing the records below would decide matters then.
        for (slot, bracket) in decisions.iter_mut().zip(brackets) {
            if let RetainedKind::Draw { window, masked, .. } = bracket.kind {
                match self.resolve_draw(bracket, window, masked) {
                    Some(decision) => *slot = Some(decision),
                    None => {
                        self.clear();
                        self.refused = true;
                        return Err(EngineError::RetainedKeyMissing { key: bracket.key });
                    }
                }
            }
        }

        // A served draw nested inside a recorded layer composites into that
        // layer's page, which was sized from the draws the layer holds — and a
        // `DrawRetained`'s own layer holds none. Every enclosing layer's bounds
        // grow to take the composite in, before anything is sized from them.
        grow_enclosing_layers(brackets, &decisions, layers);

        // Every key the frame names is off limits to eviction: a drawn page is
        // sampled this frame, and a recorded one is either reused or replaced.
        let named: Vec<u64> = brackets.iter().map(|bracket| bracket.key).collect();
        let drawn: Vec<u64> = brackets
            .iter()
            .filter(|bracket| bracket.is_draw())
            .map(|bracket| bracket.key)
            .collect();
        let mut candidates: Vec<(u64, u64, u64)> = self
            .pages
            .iter()
            .filter(|page| !named.contains(&page.key))
            .map(|page| (page.last_used, page.key, page.bytes()))
            .collect();
        candidates.sort_unstable();
        let mut evicted = 0_usize;
        let mut projected = self.used_bytes();
        // A key the frame draws is never re-recorded in the same frame: the
        // draw samples the page the ledger promised, not one this frame
        // rewrites.
        let mut recorded: Vec<u64> = drawn;

        for (index, bracket) in brackets.iter().enumerate() {
            let RetainedKind::Record { rect, retainable } = bracket.kind else {
                continue;
            };
            let decision = self.admit(
                bracket,
                rect,
                retainable,
                layers,
                &recorded,
                &mut projected,
                &candidates,
                &mut evicted,
            );
            if matches!(decision, RetainedDecision::Record { .. }) {
                recorded.push(bracket.key);
            }
            if let Some(slot) = decisions.get_mut(index) {
                *slot = Some(decision);
            }
        }

        Ok(RetainedPlan {
            decisions: decisions
                .into_iter()
                .map(|decision| {
                    decision.unwrap_or(RetainedDecision::Inline(RetainedRefusal::NotRetainable))
                })
                .collect(),
            evictions: candidates
                .iter()
                .take(evicted)
                .map(|&(_, key, _)| key)
                .collect(),
        })
    }

    /// Apply `plan`, decided for `brackets`, once nothing about the frame can
    /// fail any more: evict, then give every admitted body the schedule really
    /// rendered (`rendered`, indexed like `brackets`) its page, then bind each
    /// bracket to the page it uses this frame.
    pub fn commit<A>(
        &mut self,
        allocator: &A,
        brackets: &[RetainedBracket],
        plan: &RetainedPlan,
        rendered: &[bool],
    ) where
        A: TextureAllocator<Texture = T, View = V>,
    {
        let mut changed = false;
        for key in &plan.evictions {
            changed |= self.remove(*key);
        }

        let frame = self.frame;
        for (index, (bracket, decision)) in brackets.iter().zip(&plan.decisions).enumerate() {
            match *decision {
                RetainedDecision::Record { bounds } => {
                    if !rendered.get(index).copied().unwrap_or(false) {
                        continue;
                    }
                    let size = (u32::from(bounds.width()), u32::from(bounds.height()));
                    match self.position(bracket.key) {
                        Some(at) if fits(self.pages[at].size, size) => {
                            let page = &mut self.pages[at];
                            let before = page.entry();
                            page.bounds = bounds;
                            page.placement = bracket.placement;
                            page.extent_px = bracket.extent_px;
                            page.last_used = frame;
                            changed |= page.entry() != before;
                        }
                        existing => {
                            if let Some(at) = existing {
                                self.pages.swap_remove(at);
                            }
                            let (texture, view) = allocator
                                .allocate_texture(&retained_page_descriptor(size.0, size.1));
                            self.pages.push(RetainedPage {
                                key: bracket.key,
                                bounds,
                                placement: bracket.placement,
                                extent_px: bracket.extent_px,
                                size,
                                last_used: frame,
                                _texture: texture,
                                view,
                            });
                            changed = true;
                        }
                    }
                }
                RetainedDecision::Draw { .. } | RetainedDecision::Hidden => {
                    if let Some(at) = self.position(bracket.key) {
                        self.pages[at].last_used = frame;
                    }
                }
                RetainedDecision::Inline(_) => {}
            }
        }

        self.bound.clear();
        for (index, (bracket, decision)) in brackets.iter().zip(&plan.decisions).enumerate() {
            let uses_page = match decision {
                RetainedDecision::Record { .. } => rendered.get(index).copied().unwrap_or(false),
                RetainedDecision::Draw { .. } => true,
                RetainedDecision::Hidden | RetainedDecision::Inline(_) => false,
            };
            self.bound
                .push(uses_page.then(|| self.position(bracket.key)).flatten());
        }

        if changed {
            self.generation = self.generation.wrapping_add(1);
        }
    }

    /// Where a `DrawRetained` composites from, or `None` when the key is not
    /// resident at its placement.
    fn resolve_draw(
        &self,
        bracket: &RetainedBracket,
        window: RectU16,
        masked: bool,
    ) -> Option<RetainedDecision> {
        if masked {
            return None;
        }
        let page = self.pages.get(self.position(bracket.key)?)?;
        if page.extent_px != bracket.extent_px {
            return None;
        }
        let (dx, dy) = page.placement.offset_to(&bracket.placement)?;

        // The page's bounds moved by the draw's offset, in a signed space wide
        // enough that a move off the grid's edge is representable.
        let moved = (
            i64::from(page.bounds.x0) + i64::from(dx),
            i64::from(page.bounds.y0) + i64::from(dy),
            i64::from(page.bounds.x1) + i64::from(dx),
            i64::from(page.bounds.y1) + i64::from(dy),
        );
        let x0 = moved.0.max(i64::from(window.x0));
        let y0 = moved.1.max(i64::from(window.y0));
        let x1 = moved.2.min(i64::from(window.x1));
        let y1 = moved.3.min(i64::from(window.y1));
        if x0 >= x1 || y0 >= y1 {
            return Some(RetainedDecision::Hidden);
        }
        let grid = |value: i64| u16::try_from(value).ok();
        Some(RetainedDecision::Draw {
            dest: RectU16::new(grid(x0)?, grid(y0)?, grid(x1)?, grid(y1)?),
            origin: (grid(x0 - moved.0)?, grid(y0 - moved.1)?),
        })
    }

    /// Admit one body, or say why not. `projected` is the store's size once
    /// everything admitted so far is committed; `candidates` the evictable
    /// pages, least recently used first, of which the first `evicted` are
    /// already planned to go.
    #[expect(
        clippy::too_many_arguments,
        reason = "the frame-wide running state of `plan`'s admission loop, \
                  threaded rather than bundled so each piece stays a plain local there"
    )]
    fn admit(
        &self,
        bracket: &RetainedBracket,
        rect: RectU16,
        retainable: bool,
        layers: &[RecordedLayer],
        recorded: &[u64],
        projected: &mut u64,
        candidates: &[(u64, u64, u64)],
        evicted: &mut usize,
    ) -> RetainedDecision {
        let refuse = RetainedDecision::Inline;
        if !retainable {
            return refuse(RetainedRefusal::NotRetainable);
        }
        if recorded.contains(&bracket.key) {
            return refuse(RetainedRefusal::Duplicate);
        }
        let Some(layer) = layers.get(bracket.layer as usize) else {
            return refuse(RetainedRefusal::NotRetainable);
        };
        let bounds = layer.bbox;
        if bounds.is_empty() {
            return refuse(RetainedRefusal::Empty);
        }
        // A recorded layer's bounds are `vello_common`'s strip bounds, which
        // run one tile past a fill ending exactly on a tile edge; one tile of
        // slack keeps that from reading as an overflow, and is all the slack
        // there is — a body reaching further out than that drew outside its
        // rectangle, where a scissor or the viewport may have cut it.
        let slack = rect_grown_by_a_tile(rect);
        if !(bounds.x0 >= slack.x0
            && bounds.y0 >= slack.y0
            && bounds.x1 <= slack.x1
            && bounds.y1 <= slack.y1)
        {
            return refuse(RetainedRefusal::Overflow);
        }
        let size = (u32::from(bounds.width()), u32::from(bounds.height()));
        if size.0 > self.max_dimension || size.1 > self.max_dimension {
            return refuse(RetainedRefusal::TooLarge);
        }
        let need = page_bytes(size);
        if need > self.budget.bytes {
            return refuse(RetainedRefusal::OverBudget);
        }

        // What committing this body adds: nothing when the key's page is
        // reused, its whole page when one is allocated — less the page it
        // replaces.
        let current = self.position(bracket.key).and_then(|at| self.pages.get(at));
        let after = match current {
            Some(page) if fits(page.size, size) => *projected,
            Some(page) => projected.saturating_sub(page.bytes()).saturating_add(need),
            None => projected.saturating_add(need),
        };

        // Evict, least recently used first, only as far as this body needs —
        // and only if that is far enough.
        let mut freed_to = after;
        let mut cursor = *evicted;
        while freed_to > self.budget.bytes {
            let Some(&(_, _, bytes)) = candidates.get(cursor) else {
                return refuse(RetainedRefusal::OverBudget);
            };
            freed_to = freed_to.saturating_sub(bytes);
            cursor += 1;
        }
        *evicted = cursor;
        *projected = freed_to;
        RetainedDecision::Record { bounds }
    }

    fn position(&self, key: u64) -> Option<usize> {
        self.pages.iter().position(|page| page.key == key)
    }

    /// Drop `key`'s page, answering whether there was one.
    fn remove(&mut self, key: u64) -> bool {
        match self.position(key) {
            Some(at) => {
                self.pages.swap_remove(at);
                self.generation = self.generation.wrapping_add(1);
                true
            }
            None => false,
        }
    }
}

/// Grow every recorded layer enclosing a served `DrawRetained` to cover the
/// composite it now holds, rounded out to whole tiles.
///
/// A layer's bounds are the union of what was drawn into it, folded in as each
/// draw was recorded; a `DrawRetained`'s own layer is empty, so nothing of the
/// composite it stands for reached its ancestors — a translucent layer holding
/// nothing but a drawn page would read as covering no pixel and be dropped, and
/// one holding more would be sized short of the composite. Walked through the
/// recording's own parent links, bounded by the layer count so a malformed
/// recording cannot loop.
fn grow_enclosing_layers(
    brackets: &[RetainedBracket],
    decisions: &[Option<RetainedDecision>],
    layers: &mut [RecordedLayer],
) {
    let mut parents: Option<Vec<Option<usize>>> = None;
    for (bracket, decision) in brackets.iter().zip(decisions) {
        let Some(RetainedDecision::Draw { dest, .. }) = decision else {
            continue;
        };
        let parents = parents.get_or_insert_with(|| parent_layers(layers));
        let grown = tile_bounds_of(*dest);
        let mut at = parents.get(bracket.layer as usize).copied().flatten();
        let mut steps = 0_usize;
        while let Some(index) = at {
            steps += 1;
            if steps > layers.len() {
                break;
            }
            if let Some(layer) = layers.get_mut(index) {
                if layer.bbox.is_empty() {
                    layer.bbox = grown;
                } else {
                    layer.bbox.union(grown);
                }
            }
            at = parents.get(index).copied().flatten();
        }
    }
}

/// Each recorded layer's enclosing layer, or `None` for one under the frame
/// root.
fn parent_layers(layers: &[RecordedLayer]) -> Vec<Option<usize>> {
    let mut parents = vec![None; layers.len()];
    for (index, layer) in layers.iter().enumerate() {
        for node in &layer.nodes {
            if let Some(child) = node.layer
                && let Some(slot) = parents.get_mut(child as usize)
            {
                *slot = Some(index);
            }
        }
    }
    parents
}

/// `rect` rounded out to whole tiles, saturating at the grid's edge.
fn tile_bounds_of(rect: RectU16) -> RectU16 {
    let down = |value: u16, step: u16| value - value % step;
    let up = |value: u16, step: u16| match value % step {
        0 => value,
        rest => value.saturating_add(step - rest),
    };
    RectU16::new(
        down(rect.x0, Tile::WIDTH),
        down(rect.y0, Tile::HEIGHT),
        up(rect.x1, Tile::WIDTH),
        up(rect.y1, Tile::HEIGHT),
    )
}

/// `rect` grown by one tile on every side, saturating at the grid's edges.
fn rect_grown_by_a_tile(rect: RectU16) -> RectU16 {
    RectU16::new(
        rect.x0.saturating_sub(Tile::WIDTH),
        rect.y0.saturating_sub(Tile::HEIGHT),
        rect.x1.saturating_add(Tile::WIDTH),
        rect.y1.saturating_add(Tile::HEIGHT),
    )
}

/// The bytes a page of `size` texels costs.
fn page_bytes(size: (u32, u32)) -> u64 {
    u64::from(size.0)
        .saturating_mul(u64::from(size.1))
        .saturating_mul(RETAINED_BYTES_PER_TEXEL)
}

/// Whether a texture of `have` can hold a page of `need` at its origin.
fn fits(have: (u32, u32), need: (u32, u32)) -> bool {
    have.0 >= need.0 && have.1 >= need.1
}
