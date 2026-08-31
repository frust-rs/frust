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
//! One target can take several rounds. The surface, and a layer's page, are
//! written by as many passes as the frame needs; each after the first loads what
//! the one before it left rather than clearing (see [`PageTarget::continued`]),
//! so the sequence reads as one painter's-order walk however it was cut up.
//!
//! ## What this scheduler serves, and what it refuses
//!
//! Frust's layer needs are narrow. Clips lower to a scissor rectangle or a
//! coverage mask and never to an intermediate texture (see
//! [`compile::clip`](crate::compile::clip)), so the overwhelming majority of
//! frames contain no layer at all and schedule to a single round. Opacity
//! layers exist but are rare and shallow.
//!
//! The scheduler serves every layer tree [`MAX_LIVE_PAGES`] pooled pages are
//! enough for:
//!
//! - isolated layers nested at most [`MAX_CHAIN_DEPTH`] deep;
//! - isolated layers *beside* each other under one parent, a fan of any width —
//!   two widgets fading at once is the commonest sibling shape a frust screen
//!   records, and a staggered list entrance fading five rows at once is served
//!   on the same two pages;
//! - a chain hanging off an isolated ancestor's later child, which is what a
//!   navigation transition records for the whole of a scrub: a full-screen page
//!   layer holding a chip that carries a translucent chip of its own. This is
//!   the shape the [spill page](PageParity::Spill) exists for — see *The spill
//!   page* below;
//! - any mixture of the above that stays inside [`MAX_LIVE_PAGES`] live pages;
//! - a Gaussian-blur or shadow-only drop-shadow [filter](crate::filters) layer
//!   recorded directly under the frame's own surface, which takes *both*
//!   ping-pong groups for the length of its pass sequence (see *Filter rounds*
//!   below).
//!
//! A fan of any width fits because a parent does not have to composite *all* of
//! its isolated children in one pass. When both groups are taken, the parent's
//! round is [cut](cut_at): the ops accumulated so far are emitted as a round of
//! their own, the pages that batch composites go back to the pool, and the next
//! sibling is rendered into one of them. What a cut cannot pay for is a layer
//! whose own round samples a live page while an isolated ancestor is holding
//! another — three pages really are live at that point — and that is where the
//! spill page comes in.
//!
//! Everything else — a layer that finds no page even after a cut and the spill,
//! a deeper chain, a filter this engine does not render, a filter layer nested
//! inside another recorded layer, a non-default blend mode, a layer mask or
//! layer clip path — is refused with [`EngineError::SchedulerEscalation`]
//! carrying a reason that names what was found.
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
//! Rounds are emitted contents-before-composite: a layer's contents must exist
//! before the pass that composites them can run, so the deepest layer is
//! rendered first and the surface's last round is last. A layer takes the group
//! its depth's parity names when that group is free, and the other group when it
//! is not (see [`PageParity`]).
//!
//! Parity alone is what bounds a *chain* to two live pages however deep it runs:
//! while a parent renders into its own page it samples the child's, and the
//! child's page returns to the pool the moment the parent's pass ends, so the
//! grandchild reuses it. Siblings are what makes the fallback to the other group
//! necessary — two layers at one depth share a parity, and a second page in one
//! group would overwrite the first before the parent ever sampled it. So the
//! second sibling takes the group the first left free, and a third finds both
//! taken. That is what [`cut_at`] answers: the parent's round is closed early,
//! its two composites are done, both pages come back, and the third sibling
//! takes the group the first one had. A fan of any width costs one extra pass
//! per pair rather than a refused frame.
//!
//! Pages are named lazily, by the round that renders into them, rather than
//! reserved on the way down. An outer layer that reserved its page before the
//! descent would hold one group for the whole traversal and defeat the
//! ping-pong. The one exception is a layer whose round has to be cut: a cut
//! renders into the layer's own page, so that layer takes its group at its first
//! cut and keeps it until its last round. That is what makes a cut ancestor
//! expensive for everything below it — from its first cut onwards it is holding
//! one of the two groups, so a chain hanging off its later child has only one
//! group left to ping-pong in.
//!
//! ## The spill page
//!
//! One page beside the pair ([`PageParity::Spill`]), taken only where the walk
//! would otherwise refuse the frame: after [`make_room`] has cut everything it
//! could and both groups are still holding pages later rounds composite, a
//! *regular* layer takes the spill page instead of escalating. It is acquired
//! and released exactly as a parity page is — it goes back to the pool the
//! moment the round whose composite sampled it ends — so it is one more live
//! intermediate at a peak, not a page held for the frame.
//!
//! It exists for one shape, and that shape is a navigation transition: a
//! full-screen layer at a fractional opacity (the page being scrubbed) holding
//! a chip beside a chip that carries a translucent chip of its own. The outer
//! layer takes a group at the cut its first chip forces, the nested chip takes
//! the other, and the chip hosting it — whose own round has to sample the
//! nested page while the outer page is still owed upwards — has none. Because
//! the opacity is fractional for the whole gesture, refusing it refuses *every*
//! frame of the gesture: the surface holds its last presented image until the
//! transition ends, which is a frozen scrub rather than a dropped frame (see *A
//! refusal is a skipped frame* above). The identical content at the root
//! schedules on two pages, which is what made the shape a transition-only
//! defect.
//!
//! Bounded at one, deliberately. A second layer wanting the spill while it is
//! held is refused exactly as before, which is what keeps a frame's live
//! intermediates at [`MAX_LIVE_PAGES`] and keeps this a narrowing of the
//! refused set rather than a step towards the general scheduler. Two callers do
//! not reach for it at all: a [cut](cut_at) never takes it (a cut that cannot
//! find a group is skipped, and the walk carries on to the acquire that can
//! spill), and neither does a [filter layer](filter_rounds), whose passes
//! ping-pong between two groups of their own and so are held to the pair.
//!
//! Everything the pair already served schedules exactly as it did:
//! [`LivePages::free`] counts only the two groups, so the cutting decisions
//! that serve fans and chains are made against the state they were tuned
//! against, and the spill is reached only at the site that used to escalate.
//!
//! ## Inlined and dropped layers
//!
//! A layer only needs isolating when compositing it differs from drawing its
//! contents directly. A recorded layer at full opacity with no blend, mask or
//! clip composites source-over at alpha 1, which is precisely what drawing its
//! contents into the parent does, so its stream is spliced into the parent's
//! round and it costs no page and no pass. At the other end, a layer at zero
//! opacity contributes nothing at all and is dropped along with everything
//! nested inside it, and so does a layer whose contents cover no pixel at all —
//! neither its own round nor any round its descendants would have rendered is
//! emitted, because nothing would ever sample the pages they wrote.
//!
//! Depth is therefore counted over *isolated* layers only. Counting recorded
//! depth instead would let an inlined layer push its isolated descendant onto
//! the same parity as an isolated ancestor, putting two live pages in one group.
//!
//! ## Filter rounds
//!
//! A [filter](crate::filters) layer is an isolated layer whose page is not
//! composited straight away: a sequence of filter passes runs over it first,
//! and the parent composites what the last of them wrote. A pass reads a whole
//! image and writes a whole image, and one render pass cannot do both to one
//! texture, so each pass is a round of its own writing the group it did not
//! read — the same ping-pong the chain uses, run between two pages of one
//! layer instead of two layers. The sequence is arranged to be even in length
//! (see [`crate::filters::blur::blur_passes`] and
//! [`crate::filters::drop_shadow::drop_shadow_passes`]), so the result lands
//! back in the page the layer's own contents were rendered into and the
//! composite is the ordinary one.
//!
//! Every filter round clears its destination rather than loading it. A pass
//! writes only the region its step names — a decimated one writes a quarter of
//! the texels the pass before it did — and the kernels sample bilinearly with
//! no bounds checks, so whatever surrounds the written region has to be
//! transparent rather than a previous holder's pixels.
//!
//! Two consequences bound what filter shapes are served. The layer holds *both*
//! groups from its first filter round to its last, so a filter layer is refused
//! whenever the second group cannot be handed back to it; and it is refused
//! outright when it is recorded inside another layer, because `vello_common`
//! places a filter layer in its parent by undoing a source shift the reference
//! renderer applies to a filter layer's contents and frust's compiler does not,
//! which would size the parent's page from bounds short of the filter's own
//! spread on two sides. Neither is a shape frust records: a backdrop blur is a
//! layer under the surface.

pub mod pages;

pub use pages::{PageConfig, PageParity, PageSize, filter_page_size, page_ceiling, page_size};

use core::ops::Range;

use frust_gpu::TierCaps;
use vello_common::geometry::{RectU16, SizeU16};
use vello_common::peniko::BlendMode;
use vello_common::record::{CommandRecorder, Node, RecordedLayer, RecordedLayerKind};

use crate::compile::EngineDraw;
use crate::error::EngineError;
use crate::filters::{FilterStep, ServedFilter, blur, drop_shadow, served_filter};

/// The deepest chain of nested isolated layers this scheduler serves.
///
/// Four levels covers every layer shape frust's widget set records; a deeper
/// one is escalated rather than served — the frame is skipped, and shapes past
/// this bound are rare enough that skipping them beats the cost and complexity
/// of growing this module to serve them.
pub const MAX_CHAIN_DEPTH: usize = 4;

/// The page groups the depth-parity ping-pong alternates between.
///
/// Two is what the even/odd alternation guarantees for a chain of any depth,
/// and what a fan of any width is served on by cutting its parent's round. It
/// is the *pair*, not the frame's page budget — see [`MAX_LIVE_PAGES`].
pub const PING_PONG_GROUPS: usize = 2;

/// The most intermediate pages this scheduler keeps live at one time.
///
/// The [two ping-pong groups](PING_PONG_GROUPS) plus one: a single spill page
/// ([`PageParity::Spill`]) for the shape that needs a third live page and
/// cannot be cut into needing fewer. A shape needing a *fourth* — after cutting
/// an open round has been tried and could not hand one back — is a shape this
/// scheduler does not serve.
pub const MAX_LIVE_PAGES: usize = PING_PONG_GROUPS + 1;

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
    /// The group the page comes from — [`depth`](Self::depth)'s parity when
    /// either of the pair was free, the other of the pair when it was not, and
    /// the [spill page](PageParity::Spill) when neither was.
    pub parity: PageParity,
    /// The extent to acquire the page at.
    pub size: PageSize,
    /// The layer's tile-aligned device-space bounds.
    ///
    /// The layer is rendered at the page's origin, so every strip drawn into
    /// this round is offset by `-(bounds.x0, bounds.y0)`.
    pub bounds: RectU16,
    /// Whether an earlier round of this same layer already rendered into this
    /// page, so this one loads its contents instead of clearing them.
    ///
    /// A pooled page holds whatever its last holder left there, which is why a
    /// layer's *first* round always clears it. A layer whose round was
    /// [cut](cut_at) keeps the page across the cut — clearing again would wipe
    /// the half already drawn — and no other layer can have taken the group in
    /// between, because the cut is what made this layer the group's holder.
    pub continued: bool,
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

/// One pass of a filter layer's sequence, reading one page and writing the
/// other.
///
/// One instanced quad through the filter pipeline. The page it writes is the
/// round's own target, so only the page it reads is named here; the extents it
/// reads and writes are [`step`](Self::step)'s, both taken at their page's
/// origin because a layer is always rendered there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterPass {
    /// The layer being filtered, indexed into
    /// [`CommandRecorder::layers`](vello_common::record::CommandRecorder::layers).
    pub layer: u32,
    /// Which pass of the filter's sequence this is, and at what extents.
    pub step: FilterStep,
    /// The group holding the page this pass reads.
    pub source: PageParity,
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
///
/// A target is not a round: the surface, and a layer's page, can each take
/// several, and the rounds of one target run in the order they are listed with
/// every one after the first loading what the one before it left.
#[derive(Debug, Clone, PartialEq)]
pub struct Round {
    /// What this pass renders into.
    pub target: RoundTarget,
    /// The pass's work, in execution order. Empty on a filter round, which
    /// issues no draw and composites nothing.
    pub ops: Vec<RoundOp>,
    /// Pages whose contents this round consumed; they return to the pool once
    /// it completes.
    pub released: Vec<PageParity>,
    /// The filter pass this round runs, on a filter round.
    ///
    /// A field rather than a [`RoundOp`] variant, and it stays one now that the
    /// renderer executes filter rounds: a filter round is *only* a filter pass
    /// — it draws nothing and composites nothing, and it is drawn through a
    /// different pipeline, off a different instance buffer, with no viewport
    /// uniform and no strip bind groups. Naming it here is what lets the frame
    /// path branch on the round before it starts issuing a list, and keeps
    /// [`RoundOp`] exactly the set of things one strip pass can interleave.
    pub filter: Option<FilterPass>,
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

    /// The filter pass this round runs, or `None` on an ordinary round.
    #[must_use]
    pub fn filter_pass(&self) -> Option<&FilterPass> {
        self.filter.as_ref()
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
    /// The rounds `recorder` renders as: every layer's rounds before the round
    /// that composites it, and the surface's last round last.
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
    ///
    /// A Gaussian-blur or shadow-only drop-shadow filter layer is *planned*
    /// here: its contents' round, then one round per pass of its filter's
    /// sequence, then the composite of whichever page the last pass wrote.
    /// Every other filter is refused by name (see
    /// [`crate::filters::served_filter`]), and so are the two shapes a served
    /// filter layer still cannot take — one recorded inside another layer, and
    /// one that cannot be handed the second page group its passes ping-pong
    /// into.
    ///
    /// One entry point, not two. While the renderer had no filter pipeline,
    /// this call refused every filter and a second one planned them, so the
    /// frame path could not reach a round nothing could execute; the renderer
    /// runs them now ([`crate::renderer::FilterResources`]), and the split went
    /// with the reason for it.
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
                        let carry =
                            finish(stream, &mut stack, &mut rounds, &mut pages, caps, config)?;
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

                    // Suppression is inherited rather than re-derived: a layer
                    // inside one covering no pixels covers none either, however
                    // its own bounds read.
                    let inherited = stack.last().is_some_and(|stream| stream.suppressed);

                    // Nothing nested inside a dropped layer is rendered, so
                    // nothing inside one is validated either — the walk does
                    // not enter it.
                    let role = layer_role(id, layer)?;
                    // A filter layer's placement in its parent is computed by
                    // undoing a shift the reference renderer applies to a
                    // filter layer's contents and frust's compiler does not, so
                    // the bounds it hands its parent fall short of the filter's
                    // own spread on two sides. Refused rather than rendered
                    // from bounds that would clip it; a backdrop blur is
                    // recorded under the surface, where nothing reads that
                    // placement.
                    if matches!(role, LayerRole::Filtered) && stack.len() > 1 {
                        return Err(escalate(format!(
                            "filter layer {id} is recorded inside another layer, whose own bounds \
                             would be taken from a placement that undoes a source shift this \
                             engine's compiler never applied; a filter layer is served directly \
                             under the frame's own surface"
                        )));
                    }

                    match role {
                        LayerRole::Dropped => {}
                        LayerRole::Inline => stack.push(Stream::inline(layer, depth, inherited)),
                        LayerRole::Isolated | LayerRole::Filtered => {
                            let depth = depth.saturating_add(1);
                            if depth > MAX_CHAIN_DEPTH {
                                return Err(escalate(format!(
                                    "{depth} nested isolated layers, deeper than the \
                                     {MAX_CHAIN_DEPTH}-deep chain the simple scheduler serves"
                                )));
                            }
                            // A layer whose contents cover nothing composites
                            // nothing, so it renders no round — and neither do
                            // its descendants, whose pages nothing would ever
                            // sample. The walk still enters it, so a shape this
                            // scheduler refuses is refused wherever it is
                            // recorded rather than only where it is visible.
                            let suppressed = inherited || layer.bbox.is_empty();
                            stack.push(Stream::isolated(id, layer, depth, suppressed));
                        }
                    }
                }
            }
        }

        Ok(rounds)
    }
}

/// Emits the last round a finished stream renders as, and returns what its
/// parent splices in at the node that entered it: one composite for an isolated
/// layer, the layer's own ops for an inlined one, nothing for a layer that
/// composites nothing. The frame root pushes the last round and carries nothing.
///
/// `stack` is the walk's remaining streams — this stream's ancestors — because
/// a layer short of a page makes room by [cutting](cut_at) one of their rounds.
fn finish<'a>(
    stream: Stream<'a>,
    stack: &mut [Stream<'a>],
    rounds: &mut Vec<Round>,
    pages: &mut LivePages,
    caps: &TierCaps,
    config: &PageConfig,
) -> Result<Vec<RoundOp>, EngineError> {
    let (id, layer) = match stream.owner {
        StreamOwner::Inline => return Ok(stream.ops),
        StreamOwner::Root => {
            let released = released_pages(&stream.ops);
            for parity in &released {
                pages.release(*parity);
            }
            // An empty trailing round is worth a pass only when it is the
            // frame's only one: `build` always returns at least one round, but
            // a surface already written by an earlier cut needs no empty pass
            // behind it.
            if !stream.ops.is_empty() || !stream.emitted {
                rounds.push(Round {
                    target: RoundTarget::Root,
                    ops: stream.ops,
                    released,
                    filter: None,
                });
            }
            return Ok(Vec::new());
        }
        StreamOwner::Isolated { id, layer } => (id, layer),
    };

    // A layer covering no pixels, or one nested inside such a layer, renders
    // nothing: no round of its own was cut, none of its descendants' was
    // emitted, and no page was ever taken for any of them, so there is nothing
    // to release and nothing for the parent to composite.
    if stream.suppressed {
        return Ok(Vec::new());
    }

    let bounds = layer.bbox;
    let depth = stream.depth;

    // A regular layer wider than any single page is banded into column
    // pages (E14) rather than refused — but only while nothing has claimed
    // its page ahead of time (a cut ancestor reserves one this way, see
    // `cut_at`) and its own accumulated ops hold no composite of a nested
    // isolated child: a band's ops are replayed once per band, and
    // replaying a child's composite would read a page a later band has
    // already reused. Both are shapes banding does not reach, and a layer
    // whose height alone exceeds the ceiling is still refused by `page_size`
    // below exactly as it always was — bands are columns, so height has no
    // split to be served by.
    if matches!(layer.kind, RecordedLayerKind::Regular)
        && stream.page.is_none()
        && u32::from(bounds.width()) > page_ceiling(config, caps)
        && !holds_composite(&stream.ops)
    {
        return band_rounds(
            BandedLayer {
                id,
                layer,
                bounds,
                depth,
            },
            stream.ops,
            stack,
            rounds,
            pages,
            caps,
            config,
        );
    }

    // A filter layer's page is sized with the extra atlas margin
    // (`filter_page_size`) its kernel taps can reach past: both this
    // contents round and every filter-pass round the layer costs share this
    // one `size` (see `filter_rounds`'s own `filtered.size`), so the margin
    // has to be reserved here, before the first byte of the layer is ever
    // rendered — sizing only the pass rounds would leave the contents
    // themselves on an unpadded page while the first pass reads past its
    // edge. A regular layer never filters its contents, so it keeps the
    // plain `page_size` this always used.
    let size = match &layer.kind {
        RecordedLayerKind::Regular => page_size(bounds, config, caps)?,
        _ => filter_page_size(bounds, config, caps)?,
    };
    let page = |parity, continued| {
        RoundTarget::Page(PageTarget {
            layer: id,
            depth,
            parity,
            size,
            bounds,
            continued,
        })
    };

    // The group holding the layer's own contents once its last content round
    // has been emitted.
    let contents = match stream.page {
        // The layer's round was cut earlier this frame, so its page is already
        // this stream's and this round loads what the cut left in it.
        Some(parity) => {
            if !stream.ops.is_empty() {
                let released = released_pages(&stream.ops);
                for parity in &released {
                    pages.release(*parity);
                }
                rounds.push(Round {
                    target: page(parity, true),
                    ops: stream.ops,
                    released,
                    filter: None,
                });
            }
            parity
        }
        None => {
            make_room(stack, rounds, pages, caps, config)?;
            // Acquired before the children's pages are freed, never after: a
            // child's page is live for the whole of the pass that samples it,
            // so the group this round renders into can never be one of theirs.
            // With both groups still held after `make_room` has cut everything
            // it could, a regular layer takes the one spill page rather than
            // refusing the frame — see the module header's *The spill page*.
            let spills = matches!(layer.kind, RecordedLayerKind::Regular);
            let parity = pages
                .acquire(PageParity::from_depth(depth))
                .or_else(|| spills.then(|| pages.acquire_spill()).flatten())
                .ok_or_else(|| escalate(out_of_pages(id, spills)))?;
            let released = released_pages(&stream.ops);
            for parity in &released {
                pages.release(*parity);
            }
            rounds.push(Round {
                target: page(parity, false),
                ops: stream.ops,
                released,
                filter: None,
            });
            parity
        }
    };

    // A filter layer's contents are not what the parent composites: the filter
    // passes run over them first, and the parent samples whatever the last of
    // them wrote.
    let composited = match &layer.kind {
        RecordedLayerKind::Regular => contents,
        kind => filter_rounds(
            &FilterLayer {
                id,
                kind,
                depth,
                bounds,
                size,
                contents,
            },
            stack,
            rounds,
            pages,
            caps,
            config,
        )?,
    };

    Ok(vec![RoundOp::Composite(Composite {
        layer: id,
        parity: composited,
        bounds,
        opacity: layer.props.opacity,
    })])
}

/// A filter layer whose contents are rendered, ready for its passes to be
/// planned.
struct FilterLayer<'a> {
    /// The layer, indexed into
    /// [`CommandRecorder::layers`](vello_common::record::CommandRecorder::layers).
    id: u32,
    /// The recorded filter, re-read here rather than carried down from
    /// [`layer_role`]: preparing a blur's kernel is a handful of arithmetic
    /// over a fixed-size array, and threading it through the walk's stack would
    /// put a filter's state in every stream that has none.
    kind: &'a RecordedLayerKind,
    /// The layer's depth counted over isolated layers only, one-based.
    depth: usize,
    /// The layer's tile-aligned device-space bounds, already grown by the
    /// filter's own spread (see [`crate::filters::LayerFilter::filter_data`]).
    bounds: RectU16,
    /// The extent both of the layer's pages are acquired at.
    size: PageSize,
    /// The group the layer's own contents were rendered into.
    contents: PageParity,
}

/// Emits the rounds a filter layer's pass sequence renders as, and answers the
/// group holding the filtered result for its parent to composite.
///
/// One round per pass, each writing the group it did not read. The sequence is
/// even in length, so the result lands back in the page the contents were
/// rendered into; [`blur::blur_passes`] is what keeps that true.
fn filter_rounds(
    filtered: &FilterLayer<'_>,
    stack: &mut [Stream<'_>],
    rounds: &mut Vec<Round>,
    pages: &mut LivePages,
    caps: &TierCaps,
    config: &PageConfig,
) -> Result<PageParity, EngineError> {
    let id = filtered.id;
    // The one dispatch every filter-recognising site in this crate shares
    // (`renderer::filter_block`, `layer_role` below): the recorded primitive
    // names which filter it is, and the reason a refusal carries is that
    // primitive's own.
    let steps = match served_filter(id, filtered.kind).map_err(escalate)? {
        ServedFilter::Blur(blur) => blur::blur_passes(&blur, SizeU16::from(filtered.bounds)),
        ServedFilter::DropShadow(shadow) => {
            drop_shadow::drop_shadow_passes(&shadow, SizeU16::from(filtered.bounds))
        }
    };
    let Some(last) = steps.len().checked_sub(1) else {
        return Ok(filtered.contents);
    };

    // A filter layer holds both groups from its first pass to its last, so an
    // open round still holding the other one is cut to hand it back — innermost
    // first, and only until it comes back. A cut cannot take the group instead:
    // with both held there is none for it to acquire.
    let scratch = filtered.contents.opposite();
    for index in (0..stack.len()).rev() {
        if !pages.holds(scratch) {
            break;
        }
        cut_at(stack, index, rounds, pages, caps, config)?;
    }
    let scratch = pages.acquire(scratch).ok_or_else(|| {
        escalate(format!(
            "filter layer {id} would need both of the {PING_PONG_GROUPS} page groups at once — \
             one for its contents and one for its passes to write — and the second is holding a \
             page a later round composites that no open round could be cut to hand back"
        ))
    })?;

    let mut source = filtered.contents;
    let mut dest = scratch;
    for (index, step) in steps.iter().enumerate() {
        // The page a pass reads is the page the pass before it wrote, so both
        // stay live for the whole sequence. The scratch one goes back to the
        // pool as the last pass ends; the one holding the result goes back when
        // the parent's composite has sampled it.
        let released = if index == last {
            vec![source]
        } else {
            Vec::new()
        };
        for parity in &released {
            pages.release(*parity);
        }

        rounds.push(Round {
            // Cleared, never continued: a pass writes only the region its step
            // names — a decimated one a quarter of the texels the pass before
            // it did — and the kernels sample past that region without bounds
            // checks, so what surrounds it has to be transparent rather than a
            // previous holder's pixels.
            target: RoundTarget::Page(PageTarget {
                layer: id,
                depth: filtered.depth,
                parity: dest,
                size: filtered.size,
                bounds: filtered.bounds,
                continued: false,
            }),
            ops: Vec::new(),
            released,
            filter: Some(FilterPass {
                layer: id,
                step: *step,
                source,
            }),
        });

        if index < last {
            core::mem::swap(&mut source, &mut dest);
        }
    }

    Ok(dest)
}

/// A regular layer wider than [`page_ceiling`] can size a single page to,
/// ready for its column bands to be planned.
struct BandedLayer<'a> {
    /// The layer, indexed into
    /// [`CommandRecorder::layers`](vello_common::record::CommandRecorder::layers).
    id: u32,
    /// The recorded layer, for its opacity.
    layer: &'a RecordedLayer,
    /// The layer's tile-aligned device-space bounds — wider than
    /// [`page_ceiling`], which is why it is here rather than sized by
    /// [`page_size`] like every other regular layer.
    bounds: RectU16,
    /// The layer's depth counted over isolated layers only, one-based.
    depth: usize,
}

/// Emits the rounds a banded layer's column pages render as: one page and
/// one composite per [band](pages::page_bands), the layer's own draws
/// replayed into each.
///
/// A band is rendered and composited one at a time rather than the whole split
/// acquired up front, because a live page is scarce here: each band's composite
/// is spliced straight onto `stack`'s own innermost accumulator (exactly where
/// an ordinary single-page layer's composite would land), `make_room` is asked
/// again before the next band's page is acquired, and the same cutting it
/// already does for a wide sibling fan is what hands a finished band's page
/// back — a banded layer is, from the pool's point of view, a fan of same-depth
/// siblings that happen to share one layer id. See the [`pages`] module
/// header's *A fourth decision* section for why this is a page decision rather
/// than a new kind of target.
///
/// `ops` is `banded`'s stream's own accumulated ops — verified by the caller
/// to hold no [`RoundOp::Composite`] — replayed unchanged into every band's
/// content round; only the page each round targets and the rectangle its
/// composite lands at differ band to band.
///
/// A band takes a ping-pong group or nothing: the [spill
/// page](PageParity::Spill) is deliberately not offered here. It exists for a
/// layer whose own round samples a live page while an ancestor holds another,
/// and a banded layer is reached only when its ops hold no composite at all —
/// so the previous band's composite is always sitting in an open round
/// `make_room` can cut, and a third page is never what a band is short of.
///
/// # Errors
///
/// Whatever [`pages::page_bands`] itself refuses `banded.bounds` with: taller
/// than [`page_ceiling`] (bands are columns, so height has no split to be
/// served by), or wider than [`pages::MAX_PAGE_BANDS`] bands can cover. Both
/// are the refusal a single page gives an over-ceiling layer, unchanged by
/// banding. [`EngineError::SchedulerEscalation`] when a band's own page
/// cannot be found even one at a time.
fn band_rounds(
    banded: BandedLayer<'_>,
    ops: Vec<RoundOp>,
    stack: &mut [Stream<'_>],
    rounds: &mut Vec<Round>,
    pages: &mut LivePages,
    caps: &TierCaps,
    config: &PageConfig,
) -> Result<Vec<RoundOp>, EngineError> {
    let bands = pages::page_bands(banded.bounds, config, caps)?;

    for band in bands {
        make_room(stack, rounds, pages, caps, config)?;
        // Acquired before anything of this band is released, never after —
        // the same ordering a single-page layer's own acquisition keeps.
        let parity = pages
            .acquire(PageParity::from_depth(banded.depth))
            .ok_or_else(|| {
                escalate(format!(
                    "layer {} would need a third live intermediate page to render one of its own \
                     column bands: both of the {PING_PONG_GROUPS} groups the simple scheduler \
                     ping-pongs between are already holding a page a later round composites, and \
                     no open round could be cut to hand one back",
                    banded.id
                ))
            })?;

        rounds.push(Round {
            target: RoundTarget::Page(PageTarget {
                layer: banded.id,
                depth: banded.depth,
                parity,
                size: band.size,
                bounds: band.bounds,
                // Every band is a page of its own rather than a continuation
                // of the one before it: two bands never share a texture at
                // once, so there is nothing here for a later band to load.
                continued: false,
            }),
            ops: ops.clone(),
            released: Vec::new(),
            filter: None,
        });

        // Spliced onto the enclosing stream's own accumulator directly,
        // rather than returned for the caller to append: a later band still
        // has to acquire a group, and `make_room`'s own cutting is what finds
        // this composite and hands the group back — exactly the mechanism a
        // page-hungry sibling already relies on, so nothing here needs to
        // wait for this function to return before a round can use it.
        if let Some(parent) = stack.last_mut() {
            parent.ops.push(RoundOp::Composite(Composite {
                layer: banded.id,
                parity,
                bounds: band.bounds,
                opacity: banded.layer.props.opacity,
            }));
        }
    }

    Ok(Vec::new())
}

/// Frees a page group for a layer about to take one, by cutting an ancestor's
/// round short, when the walk is running out of groups.
///
/// Two moves, in this order.
///
/// - **Ahead of the shortage.** An isolated ancestor that has not taken a page
///   yet is going to need one, and a layer taking the last free group would
///   leave it none — which is what used to limit a parent to two isolated
///   children. Cutting such an ancestor now gives it its page *and* hands back
///   every page that cut composites, so the group this layer is about to take
///   comes straight back and the count is no worse than it started. Outermost
///   first: an outer ancestor's cut is what frees the group an inner one takes.
/// - **At the shortage.** With both groups gone, the innermost open round that
///   composites anything is cut, releasing its pages a pass early. This is what
///   serves a fan of any width under the surface: the siblings already
///   composited go back to the pool before the next one is rendered.
///
/// A cut that cannot help is skipped rather than forced, and running out of
/// ancestors to cut is not itself an error — the acquire this precedes
/// escalates if it still finds no group, which keeps one refusal site and one
/// reason.
fn make_room(
    stack: &mut [Stream<'_>],
    rounds: &mut Vec<Round>,
    pages: &mut LivePages,
    caps: &TierCaps,
    config: &PageConfig,
) -> Result<(), EngineError> {
    if pages.free() > 1 {
        return Ok(());
    }

    for index in 0..stack.len() {
        if pages.free() == 0 {
            break;
        }
        if stack[index].wants_a_page() {
            cut_at(stack, index, rounds, pages, caps, config)?;
        }
    }

    if pages.free() == 0 {
        for index in (0..stack.len()).rev() {
            if cut_at(stack, index, rounds, pages, caps, config)? {
                break;
            }
        }
    }

    Ok(())
}

/// Emits what `stack[index]` has accumulated as a round of its own, so the pages
/// that batch composites return to the pool before the walk renders anything
/// else. Answers whether a round was emitted.
///
/// The round carries that stream's ops followed by those of every inlined stream
/// open above it: an inlined layer draws into its host's round, so its ops
/// belong to this cut, in exactly the painter order the recording laid them in.
/// Whatever any of them records after the cut becomes the host's next round,
/// which loads rather than clears (see [`PageTarget::continued`]) — so a cut
/// changes when a target's pixels are written, never which ones or in what
/// order.
///
/// `false` when there is nothing to gain or no way to emit: a batch holding no
/// composite frees no page by being cut, a suppressed stream renders nothing at
/// all, an inlined one has no target of its own, and an isolated one that has
/// not taken a page cannot take one with both groups held.
fn cut_at(
    stack: &mut [Stream<'_>],
    index: usize,
    rounds: &mut Vec<Round>,
    pages: &mut LivePages,
    caps: &TierCaps,
    config: &PageConfig,
) -> Result<bool, EngineError> {
    let Some(stream) = stack.get(index) else {
        return Ok(false);
    };
    if stream.suppressed || matches!(stream.owner, StreamOwner::Inline) {
        return Ok(false);
    }

    // Every stream above this one up to the next stream with a target of its own
    // is inlined into this round.
    let end = stack
        .iter()
        .enumerate()
        .skip(index.saturating_add(1))
        .find(|(_, stream)| !matches!(stream.owner, StreamOwner::Inline))
        .map_or(stack.len(), |(at, _)| at);
    let batch = stack.get(index..end).unwrap_or(&[]);
    if !batch.iter().any(|stream| holds_composite(&stream.ops)) {
        return Ok(false);
    }

    let continued = stack[index].emitted;
    let target = match stack[index].owner {
        // Both refused above; repeated here because the target is what the
        // round is built from.
        StreamOwner::Inline => return Ok(false),
        StreamOwner::Root => RoundTarget::Root,
        StreamOwner::Isolated { id, layer } => {
            let bounds = layer.bbox;
            // A filter layer's stream can itself be cut mid-frame — nothing
            // stops a regular/opacity layer from being recorded nested inside
            // one, and `make_room` treats every isolated stream on the stack
            // alike — so this has to size the page on the same terms `finish`
            // does above, or a cut filter layer's contents would land on an
            // unpadded page its own first pass then reads past the edge of.
            let size = match &layer.kind {
                RecordedLayerKind::Regular => page_size(bounds, config, caps)?,
                _ => filter_page_size(bounds, config, caps)?,
            };
            let parity = match stack[index].page {
                Some(parity) => parity,
                None => {
                    let Some(parity) = pages.acquire(PageParity::from_depth(stack[index].depth))
                    else {
                        return Ok(false);
                    };
                    stack[index].page = Some(parity);
                    parity
                }
            };
            RoundTarget::Page(PageTarget {
                layer: id,
                depth: stack[index].depth,
                parity,
                size,
                bounds,
                continued,
            })
        }
    };

    let mut ops: Vec<RoundOp> = Vec::new();
    for stream in stack.get_mut(index..end).unwrap_or(&mut []) {
        ops.append(&mut stream.ops);
    }
    let released = released_pages(&ops);
    for parity in &released {
        pages.release(*parity);
    }
    stack[index].emitted = true;
    rounds.push(Round {
        target,
        ops,
        released,
        filter: None,
    });

    Ok(true)
}

/// How a recorded layer is served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayerRole {
    /// Composites identically to drawing its contents directly, so its stream
    /// is spliced into its parent's round.
    Inline,
    /// Needs a page and a pass of its own.
    Isolated,
    /// Needs a page and a pass of its own, plus a second page and a pass per
    /// step of its filter's sequence.
    ///
    /// Never inlined however it composites: filtering a layer's contents is not
    /// what drawing them into the parent does.
    Filtered,
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
    /// The ops accumulated since this stream's last round, in execution order.
    ops: Vec<RoundOp>,
    /// Whether a round of this stream's has been emitted already, which is what
    /// makes the next one a continuation of the same target.
    emitted: bool,
    /// The group this stream's page came from, taken at its first
    /// [cut](cut_at) and held until its last round. `None` while the stream is
    /// still free to have its page named by the round that finishes it, which
    /// is the common case — only a cut stream reserves ahead.
    page: Option<PageParity>,
    /// Whether this stream renders nothing at all: it is a layer covering no
    /// pixels, or nested inside one.
    suppressed: bool,
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
            emitted: false,
            page: None,
            suppressed: false,
        }
    }

    /// An inlined layer's stream, spliced into its parent's round at the depth
    /// its parent already occupies — an inlined layer takes no page, so it
    /// consumes no level (see the module header).
    fn inline(layer: &'a RecordedLayer, depth: usize, suppressed: bool) -> Self {
        Self {
            owner: StreamOwner::Inline,
            nodes: &layer.nodes,
            index: 0,
            depth,
            ops: Vec::new(),
            emitted: false,
            page: None,
            suppressed,
        }
    }

    /// An isolated layer's stream, one level deeper than its parent.
    fn isolated(id: u32, layer: &'a RecordedLayer, depth: usize, suppressed: bool) -> Self {
        Self {
            owner: StreamOwner::Isolated { id, layer },
            nodes: &layer.nodes,
            index: 0,
            depth,
            ops: Vec::new(),
            emitted: false,
            page: None,
            suppressed,
        }
    }

    /// Whether this stream is one a page group is still owed to.
    ///
    /// True only for an isolated layer that will really render — a suppressed
    /// one renders nothing — and only until its first cut names its group.
    fn wants_a_page(&self) -> bool {
        matches!(self.owner, StreamOwner::Isolated { .. })
            && !self.suppressed
            && self.page.is_none()
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

/// Which groups are holding a finished page.
///
/// A group holds one page at a time, so three booleans are this scheduler's
/// whole page allocator: acquiring is finding a group no round still needs,
/// releasing is the round that sampled a page ending. Nothing here allocates a
/// texture — the pool does that at execute time, keyed on the extent the round
/// carries.
///
/// The [ping-pong pair](PING_PONG_GROUPS) and the [spill page](Self::spill) are
/// kept apart deliberately. [`free`](Self::free) counts only the pair, so
/// [`make_room`]'s decisions — when to cut ahead of a shortage, when to cut at
/// one — are made against exactly the state they were tuned against and every
/// shape this scheduler already served schedules unchanged. The spill is only
/// ever reached through [`acquire_spill`](Self::acquire_spill), at the one site
/// that would otherwise refuse the frame.
#[derive(Debug, Default)]
struct LivePages {
    held: [bool; PING_PONG_GROUPS],
    /// Whether the one spill page is holding a page a later round composites.
    spill: bool,
}

impl LivePages {
    /// The ping-pong group to render into, preferring `preferred` and falling
    /// back to the other, or `None` when both are holding a page a later round
    /// still composites.
    ///
    /// The preference is what keeps a chain alternating exactly as its depths'
    /// parities say; the fallback is what lets a second sibling, which shares
    /// its predecessor's depth and so its preference, take the free group
    /// instead of overwriting the page beside it. Never answers the spill page:
    /// that is [`acquire_spill`](Self::acquire_spill)'s, so the fallback to a
    /// third page is a decision a caller makes rather than one this hides.
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

    /// The spill page, or `None` when it is already holding one.
    ///
    /// The whole of the bound: one page, so a second layer wanting it while it
    /// is held finds nothing and the frame is refused exactly as it was before
    /// the spill existed.
    fn acquire_spill(&mut self) -> Option<PageParity> {
        if self.spill {
            return None;
        }
        self.set(PageParity::Spill, true);
        Some(PageParity::Spill)
    }

    /// Hand a group's page back, once the round that sampled it has ended.
    fn release(&mut self, parity: PageParity) {
        self.set(parity, false);
    }

    /// How many of the ping-pong groups are holding no page.
    ///
    /// The spill page is deliberately not counted: it is a last resort rather
    /// than a group in the rotation, and counting it would make [`make_room`]
    /// stop cutting one shortage early — which would change how every already
    /// served shape schedules.
    fn free(&self) -> usize {
        self.held.iter().filter(|held| !**held).count()
    }

    /// Whether `parity`'s group is holding a page.
    fn holds(&self, parity: PageParity) -> bool {
        match parity {
            PageParity::Spill => self.spill,
            group => self.held.get(group.index()).copied().unwrap_or(false),
        }
    }

    fn set(&mut self, parity: PageParity, held: bool) {
        match parity {
            PageParity::Spill => self.spill = held,
            group => {
                if let Some(slot) = self.held.get_mut(group.index()) {
                    *slot = held;
                }
            }
        }
    }
}

/// How `layer` is served, refusing every property this scheduler cannot honour.
fn layer_role(id: u32, layer: &RecordedLayer) -> Result<LayerRole, EngineError> {
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

    if opacity <= 0.0 {
        return Ok(LayerRole::Dropped);
    }

    match &layer.kind {
        RecordedLayerKind::Regular => Ok(if opacity >= 1.0 {
            LayerRole::Inline
        } else {
            LayerRole::Isolated
        }),
        // The two filters the engine renders are a Gaussian blur and a
        // shadow-only drop shadow; every other filter a recording can carry is
        // refused here, by name, and the frame is skipped. A filtered layer is
        // never inlined, whatever its opacity.
        kind => {
            // Recognised here rather than at the filter round, so a refusal
            // names what was found before any round has been emitted and a
            // filter shape this engine cannot render is refused wherever it is
            // recorded — including inside a layer whose contents cover nothing.
            served_filter(id, kind).map_err(escalate)?;
            Ok(LayerRole::Filtered)
        }
    }
}

/// Whether `ops` composites anything, and so whether emitting them as a round
/// would hand a page back to the pool.
fn holds_composite(ops: &[RoundOp]) -> bool {
    ops.iter().any(|op| matches!(op, RoundOp::Composite(_)))
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

/// Why layer `id` is refused when every page it could have rendered into is
/// holding one a later round still composites.
///
/// Two reasons, because two different bounds are reached. `spills` is whether
/// the layer was offered the [spill page](PageParity::Spill) — every regular
/// layer is — and so whether what ran out was all three of this scheduler's
/// live pages or only the ping-pong pair a filter layer is held to.
fn out_of_pages(id: u32, spills: bool) -> String {
    if spills {
        format!(
            "layer {id} would need a fourth live intermediate page: both of the \
             {PING_PONG_GROUPS} groups the simple scheduler ping-pongs between and the one spill \
             page beside them are already holding a page a later round composites, and no open \
             round could be cut to hand one back. A nested chain of any depth fits, a fan of \
             siblings of any width fits, and so does a chain hanging off an isolated ancestor's \
             later child; what does not is a layer whose own round samples two live pages while a \
             third is still owed to a round above it"
        )
    } else {
        format!(
            "filter layer {id} would need a third live intermediate page for its contents: both \
             of the {PING_PONG_GROUPS} groups the simple scheduler ping-pongs between are already \
             holding a page a later round composites, and no open round could be cut to hand one \
             back. The spill page a regular layer falls back on is not offered here — a filter \
             layer's passes ping-pong between the two groups themselves, so it is held to them"
        )
    }
}

/// The escalation error carrying `reason`.
fn escalate(reason: impl Into<String>) -> EngineError {
    EngineError::SchedulerEscalation {
        reason: reason.into(),
    }
}
