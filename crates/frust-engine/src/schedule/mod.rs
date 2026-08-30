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
//! layers exist but are rare and shallow, and they nest rather than branch.
//!
//! The scheduler therefore serves exactly that shape and no more:
//!
//! - a chain of at most [`MAX_CHAIN_DEPTH`] nested isolated layers, each with
//!   at most one child layer;
//! - at most [`MAX_LIVE_PAGES`] intermediate pages live at any moment.
//!
//! Everything else — a branching layer graph, a deeper chain, a filter layer, a
//! non-default blend mode, a layer mask or layer clip path — is refused with
//! [`EngineError::SchedulerEscalation`] carrying a reason that names what was
//! found. The caller falls the frame back to the reference renderer rather than
//! rendering it wrong, so a refusal is a routing decision and never a panic
//! (E17). Porting the reference renderer's general scheduler, which serves any
//! scene at the cost of roughly ten times this module's code, is what the
//! [`full-scheduler`](full) feature marks the site for.
//!
//! ## Bottom-up traversal and the two-page ping-pong
//!
//! Rounds are emitted innermost-first: a layer's contents must exist before the
//! pass that composites them can run, so the deepest layer is rendered first and
//! the surface last. Which group a layer's page comes from is decided by the
//! parity of its depth — even depths take the even group, odd depths the odd one
//! (see [`PageParity`]). That is what bounds a chain to two live pages however
//! deep it runs: while a parent renders into its own page it samples the child's,
//! and the child's page returns to the pool the moment the parent's pass ends, so
//! the grandchild reuses it.
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
/// one is escalated rather than served, because the cases past it are the ones
/// whose cost is better paid by the reference renderer than by growing this
/// module.
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
    /// The rounds `recorder` renders as, innermost layer first and the surface
    /// last.
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
    /// larger than a page can be sized to. Both mean the frame is to be
    /// rendered by another route; neither is a panic.
    pub fn build(
        recorder: &CommandRecorder<EngineDraw>,
        caps: &TierCaps,
        config: &PageConfig,
    ) -> Result<Vec<Round>, EngineError> {
        let chain = collect_chain(recorder)?;

        let isolated = chain
            .iter()
            .filter(|link| link.role == LayerRole::Isolated)
            .count();
        if isolated > MAX_CHAIN_DEPTH {
            return Err(escalate(format!(
                "{isolated} nested isolated layers, deeper than the {MAX_CHAIN_DEPTH}-deep chain \
                 the simple scheduler serves"
            )));
        }

        let mut rounds = Vec::with_capacity(isolated + 1);
        // What the layer just scheduled leaves for its parent to splice in at
        // the node that entered it: one composite for an isolated layer, the
        // layer's own ops for an inlined one, nothing for a dropped one.
        let mut carry: Vec<RoundOp> = Vec::new();
        let mut depth = isolated;

        for link in chain.iter().rev() {
            match link.role {
                LayerRole::Dropped => carry = Vec::new(),
                LayerRole::Inline => carry = node_ops(&link.layer.nodes, carry),
                LayerRole::Isolated => {
                    let ops = node_ops(&link.layer.nodes, carry);
                    let bounds = link.layer.bbox;
                    let parity = PageParity::from_depth(depth);
                    depth = depth.saturating_sub(1);

                    // A layer whose contents cover nothing composites nothing:
                    // it needs no page and no pass of its own.
                    if bounds.is_empty() {
                        carry = Vec::new();
                        continue;
                    }

                    let released = released_pages(&ops);
                    check_live_pages(Some(parity), &released)?;

                    rounds.push(Round {
                        target: RoundTarget::Page(PageTarget {
                            layer: link.id,
                            depth: depth.saturating_add(1),
                            parity,
                            size: page_size(bounds, config, caps)?,
                            bounds,
                        }),
                        ops,
                        released,
                    });

                    carry = vec![RoundOp::Composite(Composite {
                        layer: link.id,
                        parity,
                        bounds,
                        opacity: link.layer.props.opacity,
                    })];
                }
            }
        }

        let ops = node_ops(&recorder.nodes, carry);
        let released = released_pages(&ops);
        check_live_pages(None, &released)?;
        rounds.push(Round {
            target: RoundTarget::Root,
            ops,
            released,
        });

        Ok(rounds)
    }
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

/// One layer on the recording's single-child chain.
struct ChainLink<'a> {
    id: u32,
    layer: &'a RecordedLayer,
    role: LayerRole,
}

/// The recording's layers from outermost to innermost, refusing any shape this
/// scheduler does not serve.
///
/// The walk stops at a dropped layer: nothing nested inside one is rendered, so
/// nothing inside one is validated either.
fn collect_chain(
    recorder: &CommandRecorder<EngineDraw>,
) -> Result<Vec<ChainLink<'_>>, EngineError> {
    let mut chain: Vec<ChainLink<'_>> = Vec::new();
    let mut parent: Option<u32> = None;
    let mut nodes: &[Node] = &recorder.nodes;

    // A child layer is always recorded after its parent, so a well-formed chain
    // visits each recorded layer at most once. The bound is what keeps a
    // recording whose nodes point at each other from looping here; one extra
    // step lets the last layer prove it has no child.
    for _ in 0..=recorder.layers.len() {
        let Some(id) = only_child(nodes, parent)? else {
            return Ok(chain);
        };
        let Some(layer) = recorder.layers.get(id as usize) else {
            return Err(escalate(format!(
                "a node enters layer {id}, which the recording does not hold"
            )));
        };

        let role = layer_role(id, layer)?;
        chain.push(ChainLink { id, layer, role });
        if role == LayerRole::Dropped {
            return Ok(chain);
        }

        parent = Some(id);
        nodes = &layer.nodes;
    }

    Err(escalate(
        "the recorded layers do not form a chain that terminates; the simple scheduler walks \
         each layer once",
    ))
}

/// The one layer `nodes` enters, refusing a stream that enters more than one.
fn only_child(nodes: &[Node], parent: Option<u32>) -> Result<Option<u32>, EngineError> {
    let mut children = nodes.iter().filter_map(|node| node.layer);
    let first = children.next();
    let rest = children.count();

    if rest > 0 {
        let owner = match parent {
            Some(id) => format!("layer {id}"),
            None => "the root".to_string(),
        };
        return Err(escalate(format!(
            "{owner} enters {} child layers; a branching layer graph can need a third live \
             intermediate page, and the simple scheduler ping-pongs between two",
            rest.saturating_add(1)
        )));
    }

    Ok(first)
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

/// One layer's (or the root's) ops: its own draws, with `child` spliced in at
/// the node that enters its child layer.
///
/// `child` is consumed by the first such node. A stream entering more than one
/// child layer never reaches here — [`only_child`] refuses it first.
fn node_ops(nodes: &[Node], mut child: Vec<RoundOp>) -> Vec<RoundOp> {
    let mut ops = Vec::with_capacity(nodes.len().saturating_add(child.len()));

    for node in nodes {
        if node.draws.start < node.draws.end {
            ops.push(RoundOp::Draws(node.draws.clone()));
        }
        if node.layer.is_some() {
            ops.append(&mut child);
        }
    }

    ops
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

/// Refuses a round that would hold more pages live than the ping-pong allows.
///
/// `own` is the page the round renders into, `None` for the surface; `released`
/// are the pages it samples. Both are live for the length of the pass.
fn check_live_pages(own: Option<PageParity>, released: &[PageParity]) -> Result<(), EngineError> {
    if let Some(own) = own
        && released.contains(&own)
    {
        return Err(escalate(format!(
            "a round would render into the {own:?} page group while sampling another page from \
             it; the simple scheduler keeps one page per group"
        )));
    }

    let live = usize::from(own.is_some()).saturating_add(released.len());
    if live > MAX_LIVE_PAGES {
        return Err(escalate(format!(
            "{live} live intermediate pages required, more than the {MAX_LIVE_PAGES} the simple \
             scheduler ping-pongs between"
        )));
    }

    Ok(())
}

/// The escalation error carrying `reason`.
fn escalate(reason: impl Into<String>) -> EngineError {
    EngineError::SchedulerEscalation {
        reason: reason.into(),
    }
}
