//! Scheduling a recording into render passes.
//!
//! A recorded frame is a tree: a root command stream that may enter a layer,
//! whose own stream may enter another. Draws inside a layer that composites
//! with anything other than plain source-over at full opacity cannot be written
//! straight to the surface — the layer has to be rendered in isolation first
//! and then composited as a whole. A GPU render pass writes one target, so each
//! isolated layer costs a pass of its own. This module decides how many passes
//! a frame needs, what each one targets, and in what order they run; it
//! allocates nothing and touches no device.
//!
//! [`Schedule::build`] returns the frame's [`Round`]s in execution order. A
//! round is one render pass over either the frame's own surface or one pooled
//! intermediate [page](pages). Its [`ops`](Round::ops) run in the order they
//! are listed: a batch of draws, then the composite of a finished page onto the
//! round's target, then more draws, and so on, exactly as the recording
//! interleaved them.
//!
//! ## What this scheduler serves, and what it refuses
//!
//! Frust's layer needs are narrow. Clips lower to a scissor rectangle or a
//! coverage mask and never to an intermediate texture (see
//! [`compile::clip`](crate::compile::clip)), so the overwhelming majority of
//! frames contain no layer at all and schedule to a single round. Opacity
//! layers exist but are rare and shallow.
//!
//! The scheduler serves every layer tree two pooled pages are enough for:
//!
//! - isolated layers nested at most [`MAX_CHAIN_DEPTH`] deep;
//! - isolated layers *beside* each other under one parent, for as long as no
//!   single round needs more than [`MAX_LIVE_PAGES`] pages at once — which is
//!   what two widgets fading at the same time, the commonest sibling shape a
//!   frust screen records, costs;
//! - any mixture of the two that stays inside that bound.
//!
//! The bound is per round, not per frame, because a parent composites *all* of
//! its isolated children in the one pass that renders it: their pages are live
//! together, whether they were finished one after another or not. The frame's
//! own surface occupies no page, which is why a sibling pair directly under it
//! fits — the two groups are both free — while the same pair inside a layer does
//! not: that layer's own page is one of the two.
//!
//! Everything else — a round needing a third live page, a deeper chain, a filter
//! layer, a non-default blend mode, a layer mask or layer clip path — is refused
//! with [`EngineError::SchedulerEscalation`] carrying a reason that names what
//! was found.
//!
//! ## A refusal is a skipped frame, not a fallback
//!
//! Refusing is a return value and never a panic (E17), but it is not a route to
//! a second renderer: the engine tier carries none, and which renderer draws a
//! surface is decided once when the surface is configured, not per frame. What
//! becomes of a refused frame is the caller's own contract. `frust-render`'s
//! engine arm drops it — the acquired texture is released unpresented, the
//! refusal is counted, and the reason is logged rate-limited — so nothing new
//! reaches the screen and whatever was presented last stays there.
//!
//! A shape that refuses on *every* frame therefore freezes a surface rather
//! than degrading it, which is the reason the refused set is kept to shapes
//! frust's own widget tree does not record, and the reason a shape it does
//! record (two fading siblings) is served here instead of escalated. Porting the
//! reference renderer's general scheduler, which serves any scene at the cost of
//! roughly ten times this module's code, is what the
//! [`full-scheduler`](full) feature marks the site for.
//!
//! ## Bottom-up traversal and the two-page ping-pong
//!
//! Rounds are emitted innermost-first: a layer's contents must exist before the
//! pass that composites them can run, so the deepest layer is rendered first and
//! the surface last. A layer takes the group its depth's parity names when that
//! group is free, and the other group when it is not (see [`PageParity`]).
//!
//! Parity alone is what bounds a *chain* to two live pages however deep it runs:
//! while a parent renders into its own page it samples the child's, and the
//! child's page returns to the pool the moment the parent's pass ends, so the
//! grandchild reuses it. Siblings are what makes the fallback to the other group
//! necessary — two layers at one depth share a parity, and a second page in one
//! group would overwrite the first before the parent ever sampled it. So the
//! second sibling takes the group the first left free, and a third sibling, with
//! no group left to take, is refused.
//!
//! Pages are named lazily, by the round that renders into them, rather than
//! reserved on the way down. An outer layer that reserved its page before the
//! descent would hold one group for the whole traversal and defeat the
//! ping-pong.
//!
//! ## Inlined and dropped layers
//!
//! A layer only needs isolating when compositing it differs from drawing its
//! contents directly. A recorded layer at full opacity with no blend, mask or
//! clip composites source-over at alpha 1, which is precisely what drawing its
//! contents into the parent does, so its stream is spliced into the parent's
//! round and it costs no page and no pass. At the other end, a layer at zero
//! opacity contributes nothing at all and is dropped along with everything
//! nested inside it.
//!
//! Depth is therefore counted over *isolated* layers only. Counting recorded
//! depth instead would let an inlined layer push its isolated descendant onto
//! the same parity as an isolated ancestor, putting two live pages in one group.

pub mod pages;

pub use pages::{PageConfig, PageParity, PageSize, page_ceiling, page_size};

use core::ops::Range;

use frust_gpu::TierCaps;
use vello_common::geometry::RectU16;
use vello_common::peniko::BlendMode;
use vello_common::record::{CommandRecorder, Node, RecordedLayer, RecordedLayerKind};

use crate::compile::EngineDraw;
use crate::error::EngineError;

/// The deepest chain of nested isolated layers this scheduler serves.
///
/// Four levels covers every layer shape frust's widget set records; a deeper
/// one is escalated rather than served — the frame is skipped, and shapes past
/// this bound are rare enough that skipping them beats the cost and complexity
/// of growing this module to serve them.
pub const MAX_CHAIN_DEPTH: usize = 4;

/// The most intermediate pages this scheduler keeps live at one time.
///
/// Two is what the even/odd ping-pong guarantees for a chain. A shape needing a
/// third is a shape this scheduler does not serve.
pub const MAX_LIVE_PAGES: usize = 2;

/// Where the reference renderer's general scheduler is ported.
///
/// Deliberately empty. The scheduler this module implements covers the layer
/// shapes frust actually records and escalates the rest; the general algorithm
/// — an atlas per parity group, batching independent layers into shared rounds,
/// filter pass plans, non-default blend through a scratch texture — is an order
/// of magnitude more code and lands here, behind this feature, if and when a
/// frame shape needs it. The feature exists now so the module path is settled
/// and the escalation route above has a named destination.
#[cfg(feature = "full-scheduler")]
pub mod full {}

/// One pooled intermediate page a round renders into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageTarget {
    /// The layer this page holds, indexed into
    /// [`CommandRecorder::layers`](vello_common::record::CommandRecorder::layers).
    pub layer: u32,
    /// The layer's depth counted over isolated layers only, one-based.
    pub depth: usize,
    /// The group the page comes from, derived from [`depth`](Self::depth).
    pub parity: PageParity,
    /// The extent to acquire the page at.
    pub size: PageSize,
    /// The layer's tile-aligned device-space bounds.
    ///
    /// The layer is rendered at the page's origin, so every strip drawn into
    /// this round is offset by `-(bounds.x0, bounds.y0)`.
    pub bounds: RectU16,
}

/// What a round renders into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoundTarget {
    /// The frame's own surface. Draws land at their recorded coordinates.
    Root,
    /// A pooled intermediate page holding one isolated layer.
    Page(PageTarget),
}

/// Compositing a finished page onto the round's target.
///
/// One instanced quad through the intermediate-strip pipeline: it samples
/// `source()` of the page in [`parity`](Self::parity) and writes
/// [`bounds`](Self::bounds), modulated by [`opacity`](Self::opacity). The page
/// is free for reuse once the round holding this composite ends.
#[derive(Debug, Clone, PartialEq)]
pub struct Composite {
    /// The layer being composited, indexed into
    /// [`CommandRecorder::layers`](vello_common::record::CommandRecorder::layers).
    pub layer: u32,
    /// The group holding the finished page.
    pub parity: PageParity,
    /// Where the layer lands in the round's target, in that target's own
    /// coordinates.
    pub bounds: RectU16,
    /// Constant alpha applied while compositing, strictly between 0 and 1.
    pub opacity: f32,
}

impl Composite {
    /// The region of the page to sample, in page texels.
    ///
    /// The layer was rendered at the page's origin, so this is
    /// [`bounds`](Self::bounds) moved there.
    #[must_use]
    pub fn source(&self) -> RectU16 {
        RectU16::new(0, 0, self.bounds.width(), self.bounds.height())
    }
}

/// One unit of work inside a round, executed in list order.
#[derive(Debug, Clone, PartialEq)]
pub enum RoundOp {
    /// Draws to issue into the round's target, indexed into
    /// [`CommandRecorder::draws`](vello_common::record::CommandRecorder::draws).
    ///
    /// Never empty: a node that recorded no draw contributes no op.
    Draws(Range<u32>),
    /// A finished page composited onto the round's target.
    Composite(Composite),
}

/// One render pass over one target.
#[derive(Debug, Clone, PartialEq)]
pub struct Round {
    /// What this pass renders into.
    pub target: RoundTarget,
    /// The pass's work, in execution order.
    pub ops: Vec<RoundOp>,
    /// Pages whose contents this round consumed; they return to the pool once
    /// it completes.
    pub released: Vec<PageParity>,
}

impl Round {
    /// The page this round renders into, or `None` for the surface.
    #[must_use]
    pub fn page(&self) -> Option<&PageTarget> {
        match &self.target {
            RoundTarget::Root => None,
            RoundTarget::Page(page) => Some(page),
        }
    }

    /// Whether this round renders into the frame's own surface.
    #[must_use]
    pub fn is_root(&self) -> bool {
        matches!(self.target, RoundTarget::Root)
    }

    /// The composites this round performs, in execution order.
    pub fn composites(&self) -> impl Iterator<Item = &Composite> {
        self.ops.iter().filter_map(|op| match op {
            RoundOp::Composite(composite) => Some(composite),
            RoundOp::Draws(_) => None,
        })
    }

    /// How many recorded draws this round issues.
    #[must_use]
    pub fn draw_count(&self) -> u32 {
        self.ops
            .iter()
            .map(|op| match op {
                RoundOp::Draws(range) => range.end.saturating_sub(range.start),
                RoundOp::Composite(_) => 0,
            })
            .sum()
    }
}

/// Turns a recording into the rounds that render it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule;

impl Schedule {
    /// The rounds `recorder` renders as: every layer before the round that
    /// composites it, innermost first, and the surface last.
    ///
    /// Always at least one round: a recording with no layers, and one whose
    /// layers all inline or drop away, both schedule to the single root round
    /// that clears and paints the surface.
    ///
    /// # Errors
    ///
    /// [`EngineError::SchedulerEscalation`] for a layer shape outside what this
    /// scheduler serves (see the module header), with a reason naming what was
    /// found, and [`EngineError::IntermediateTextureTooLarge`] for a layer
    /// larger than a page can be sized to. Neither is a panic, and neither
    /// routes the frame anywhere: both hand the caller a frame it is expected
    /// to skip.
    pub fn build(
        recorder: &CommandRecorder<EngineDraw>,
        caps: &TierCaps,
        config: &PageConfig,
    ) -> Result<Vec<Round>, EngineError> {
        let mut rounds: Vec<Round> = Vec::new();
        let mut pages = LivePages::default();
        // A child layer is always recorded after its parent, so a well-formed
        // recording enters each layer exactly once. Marking them is what keeps
        // a recording whose nodes re-enter one from being walked forever, and
        // it bounds the stack below to one entry per recorded layer.
        let mut entered = vec![false; recorder.layers.len()];
        let mut stack = vec![Stream::root(&recorder.nodes)];

        while !stack.is_empty() {
            // Taking one node off the innermost open stream is a borrow of the
            // stack that has to end before the step below can push or pop it.
            let step = match stack.last_mut() {
                None => break,
                Some(stream) => match stream.nodes.get(stream.index) {
                    None => Step::Done,
                    Some(node) => {
                        stream.index = stream.index.saturating_add(1);
                        if node.draws.start < node.draws.end {
                            stream.ops.push(RoundOp::Draws(node.draws.clone()));
                        }
                        match node.layer {
                            None => Step::Next,
                            Some(id) => Step::Enter {
                                id,
                                depth: stream.depth,
                            },
                        }
                    }
                },
            };

            match step {
                Step::Next => {}
                Step::Done => {
                    if let Some(stream) = stack.pop() {
                        let carry = finish(stream, &mut rounds, &mut pages, caps, config)?;
                        if let Some(parent) = stack.last_mut() {
                            parent.ops.extend(carry);
                        }
                    }
                }
                Step::Enter { id, depth } => {
                    let index = id as usize;
                    let (Some(layer), Some(seen)) =
                        (recorder.layers.get(index), entered.get_mut(index))
                    else {
                        return Err(escalate(format!(
                            "a node enters layer {id}, which the recording does not hold"
                        )));
                    };
                    if *seen {
                        return Err(escalate(format!(
                            "layer {id} is entered a second time; the simple scheduler walks \
                             each recorded layer once, and a recording whose nodes re-enter one \
                             describes no tree it could render"
                        )));
                    }
                    *seen = true;

                    // Nothing nested inside a dropped layer is rendered, so
                    // nothing inside one is validated either — the walk does
                    // not enter it.
                    match layer_role(id, layer)? {
                        LayerRole::Dropped => {}
                        LayerRole::Inline => stack.push(Stream::inline(layer, depth)),
                        LayerRole::Isolated => {
                            let depth = depth.saturating_add(1);
                            if depth > MAX_CHAIN_DEPTH {
                                return Err(escalate(format!(
                                    "{depth} nested isolated layers, deeper than the \
                                     {MAX_CHAIN_DEPTH}-deep chain the simple scheduler serves"
                                )));
                            }
                            stack.push(Stream::isolated(id, layer, depth));
                        }
                    }
                }
            }
        }

        Ok(rounds)
    }
}

/// Emits the round a finished stream renders as, and returns what its parent
/// splices in at the node that entered it: one composite for an isolated layer,
/// the layer's own ops for an inlined one, nothing for a layer that composites
/// nothing. The frame root pushes the last round and carries nothing.
fn finish(
    stream: Stream<'_>,
    rounds: &mut Vec<Round>,
    pages: &mut LivePages,
    caps: &TierCaps,
    config: &PageConfig,
) -> Result<Vec<RoundOp>, EngineError> {
    let released = released_pages(&stream.ops);

    let (id, layer) = match stream.owner {
        StreamOwner::Inline => return Ok(stream.ops),
        StreamOwner::Root => {
            for parity in &released {
                pages.release(*parity);
            }
            rounds.push(Round {
                target: RoundTarget::Root,
                ops: stream.ops,
                released,
            });
            return Ok(Vec::new());
        }
        StreamOwner::Isolated { id, layer } => (id, layer),
    };

    // A layer whose contents cover nothing composites nothing: it needs no page
    // and no pass of its own. The pages its own children finished into are
    // freed here, because no round of this layer's will ever sample them.
    let bounds = layer.bbox;
    if bounds.is_empty() {
        for parity in &released {
            pages.release(*parity);
        }
        return Ok(Vec::new());
    }

    let size = page_size(bounds, config, caps)?;
    // Acquired before the children's pages are freed, never after: a child's
    // page is live for the whole of the pass that samples it, so the group this
    // round renders into can never be one of theirs.
    let parity = pages
        .acquire(PageParity::from_depth(stream.depth))
        .ok_or_else(|| {
            escalate(format!(
                "layer {id} would need a third live intermediate page: both of the \
                 {MAX_LIVE_PAGES} groups the simple scheduler ping-pongs between are already \
                 holding a page a later round composites. A nested chain of any depth fits; a \
                 fan of siblings stops fitting once their pages outnumber the groups"
            ))
        })?;
    for parity in &released {
        pages.release(*parity);
    }

    rounds.push(Round {
        target: RoundTarget::Page(PageTarget {
            layer: id,
            depth: stream.depth,
            parity,
            size,
            bounds,
        }),
        ops: stream.ops,
        released,
    });

    Ok(vec![RoundOp::Composite(Composite {
        layer: id,
        parity,
        bounds,
        opacity: layer.props.opacity,
    })])
}

/// How a recorded layer is served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayerRole {
    /// Composites identically to drawing its contents directly, so its stream
    /// is spliced into its parent's round.
    Inline,
    /// Needs a page and a pass of its own.
    Isolated,
    /// Contributes nothing, along with everything nested inside it.
    Dropped,
}

/// What taking one node off the innermost open stream left the walk to do.
///
/// The step is decided under a borrow of the stack and acted on after it, since
/// entering a layer pushes a stream and finishing one pops it.
enum Step {
    /// The node entered a layer.
    Enter {
        /// The layer's index into
        /// [`CommandRecorder::layers`](vello_common::record::CommandRecorder::layers).
        id: u32,
        /// The isolated nesting depth of the stream that entered it.
        depth: usize,
    },
    /// The node entered nothing; the stream continues.
    Next,
    /// The stream is exhausted and renders as its round now.
    Done,
}

/// One command stream the walk has open: the frame root's own nodes, or one
/// recorded layer's.
///
/// The walk keeps these on an explicit stack rather than recursing, so a
/// recording nesting layers far past anything a widget tree records costs heap
/// rather than call frames. The stack is bounded by the recording's layer count,
/// because a layer is entered at most once.
struct Stream<'a> {
    /// Whose stream this is, and how it is served.
    owner: StreamOwner<'a>,
    /// The stream's nodes, in recording order.
    nodes: &'a [Node],
    /// The next node to visit.
    index: usize,
    /// The isolated nesting depth of this stream's contents, one-based; zero at
    /// the frame root.
    depth: usize,
    /// The ops accumulated so far, in execution order.
    ops: Vec<RoundOp>,
}

impl<'a> Stream<'a> {
    /// The frame root's stream, which renders into no page.
    fn root(nodes: &'a [Node]) -> Self {
        Self {
            owner: StreamOwner::Root,
            nodes,
            index: 0,
            depth: 0,
            ops: Vec::new(),
        }
    }

    /// An inlined layer's stream, spliced into its parent's round at the depth
    /// its parent already occupies — an inlined layer takes no page, so it
    /// consumes no level (see the module header).
    fn inline(layer: &'a RecordedLayer, depth: usize) -> Self {
        Self {
            owner: StreamOwner::Inline,
            nodes: &layer.nodes,
            index: 0,
            depth,
            ops: Vec::new(),
        }
    }

    /// An isolated layer's stream, one level deeper than its parent.
    fn isolated(id: u32, layer: &'a RecordedLayer, depth: usize) -> Self {
        Self {
            owner: StreamOwner::Isolated { id, layer },
            nodes: &layer.nodes,
            index: 0,
            depth,
            ops: Vec::new(),
        }
    }
}

/// Whose stream a [`Stream`] walks, and so what finishing it produces.
enum StreamOwner<'a> {
    /// The frame's own surface. It occupies no page, which is what leaves both
    /// groups free for the layers directly under it.
    Root,
    /// A layer whose ops are spliced into its parent's round.
    Inline,
    /// A layer with a page and a round of its own.
    Isolated {
        /// The layer's index into
        /// [`CommandRecorder::layers`](vello_common::record::CommandRecorder::layers).
        id: u32,
        /// The recorded layer, for its bounds and its opacity.
        layer: &'a RecordedLayer,
    },
}

/// Which of the two ping-pong groups is holding a finished page.
///
/// A group holds one page at a time, so two booleans are this scheduler's whole
/// page allocator: acquiring is finding a group no round still needs, releasing
/// is the round that sampled a page ending. Nothing here allocates a texture —
/// the pool does that at execute time, keyed on the parity this hands out.
#[derive(Debug, Default)]
struct LivePages {
    held: [bool; MAX_LIVE_PAGES],
}

impl LivePages {
    /// The group to render into, preferring `preferred` and falling back to the
    /// other, or `None` when both are holding a page a later round still
    /// composites.
    ///
    /// The preference is what keeps a chain alternating exactly as its depths'
    /// parities say; the fallback is what lets a second sibling, which shares
    /// its predecessor's depth and so its preference, take the free group
    /// instead of overwriting the page beside it.
    fn acquire(&mut self, preferred: PageParity) -> Option<PageParity> {
        let parity = if !self.holds(preferred) {
            preferred
        } else if !self.holds(preferred.opposite()) {
            preferred.opposite()
        } else {
            return None;
        };

        self.set(parity, true);
        Some(parity)
    }

    /// Hand a group's page back, once the round that sampled it has ended.
    fn release(&mut self, parity: PageParity) {
        self.set(parity, false);
    }

    /// Whether `parity`'s group is holding a page.
    fn holds(&self, parity: PageParity) -> bool {
        self.held.get(parity.index()).copied().unwrap_or(false)
    }

    fn set(&mut self, parity: PageParity, held: bool) {
        if let Some(slot) = self.held.get_mut(parity.index()) {
            *slot = held;
        }
    }
}

/// How `layer` is served, refusing every property this scheduler cannot honour.
fn layer_role(id: u32, layer: &RecordedLayer) -> Result<LayerRole, EngineError> {
    if !matches!(layer.kind, RecordedLayerKind::Regular) {
        return Err(escalate(format!(
            "layer {id} is a filter layer; the simple scheduler serves opacity layers only"
        )));
    }
    if layer.props.blend_mode != BlendMode::default() {
        return Err(escalate(format!(
            "layer {id} composites with a non-default blend mode ({:?}), which has to read the \
             target it blends into",
            layer.props.blend_mode
        )));
    }
    if layer.props.mask.is_some() {
        return Err(escalate(format!(
            "layer {id} carries a layer mask, which needs an intermediate of its own"
        )));
    }
    if layer.props.clip_path.is_some() {
        return Err(escalate(format!(
            "layer {id} carries a layer clip path; frust lowers clips through the clip stack, not \
             through a layer"
        )));
    }

    let opacity = layer.props.opacity;
    if !opacity.is_finite() {
        return Err(escalate(format!(
            "layer {id} carries a non-finite opacity ({opacity})"
        )));
    }

    Ok(if opacity >= 1.0 {
        LayerRole::Inline
    } else if opacity <= 0.0 {
        LayerRole::Dropped
    } else {
        LayerRole::Isolated
    })
}

/// The pages `ops` samples, each listed once.
fn released_pages(ops: &[RoundOp]) -> Vec<PageParity> {
    let mut pages: Vec<PageParity> = Vec::new();

    for op in ops {
        if let RoundOp::Composite(composite) = op
            && !pages.contains(&composite.parity)
        {
            pages.push(composite.parity);
        }
    }

    pages
}

/// The escalation error carrying `reason`.
fn escalate(reason: impl Into<String>) -> EngineError {
    EngineError::SchedulerEscalation {
        reason: reason.into(),
    }
}
