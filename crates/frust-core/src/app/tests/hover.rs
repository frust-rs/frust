use super::*;

// --- Hover: the claim pipeline -------------------------------------------
//
// Hover has no Enter/Leave phase to lean on (adding one to `PointerPhase`
// would break every out-of-tree exhaustive match). It is instead an opt-in
// claim a widget makes from its uncaptured `Move` arm, recorded as an epoch
// stamp down the pod chain — so the fixture below is deliberately shaped like
// a real container: two hit-tested children, a capture fast-path, and paint
// recording what `PaintCtx::is_hovered` reported.

/// What one hover leaf observed, shared out of the widget tree.
#[derive(Default)]
struct HoverProbe {
    /// `PaintCtx::is_hovered()` as of the last paint.
    painted_hovered: Cell<bool>,
    /// `EventCtx::is_hovered()` as of the last event dispatch that reached it.
    event_hovered: Cell<bool>,
}

/// A leaf that claims hover on any `Move` landing inside its own bounds — the
/// canonical opt-in shape — and optionally captures the pointer on `Down` (the
/// drag fixture: a captured pointer must never create hover).
///
/// With `latches` set it follows the whole consumer contract: the same hit test
/// updates an internal flag, `request_redraw` is gated on that flag changing,
/// and `paint` self-corrects the flag from the authoritative
/// `PaintCtx::is_hovered`. Clearing `latches` is a deliberate negative control —
/// a claimant that keeps no flag — used to pin which frames the pipeline itself
/// does and does not manufacture.
struct HoverLeaf {
    captures: bool,
    latches: bool,
    hovered: bool,
    probe: Rc<HoverProbe>,
}

impl crate::widget::Widget for HoverLeaf {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(100.0, 30.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        self.probe.painted_hovered.set(ctx.is_hovered());
        if self.latches {
            // The self-correction half of the contract: authoritative here,
            // whatever the event arm last recorded.
            self.hovered = ctx.is_hovered();
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        self.probe.event_hovered.set(ctx.is_hovered());
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Move => {
                let size = ctx.size();
                let inside = p.position.x >= 0.0
                    && p.position.y >= 0.0
                    && p.position.x < size.width
                    && p.position.y < size.height;
                if inside {
                    ctx.claim_hover();
                }
                if self.latches && self.hovered != inside {
                    self.hovered = inside;
                    ctx.request_redraw();
                }
                // Deliberately `Ignored`: a hovering widget does not consume a
                // move it merely watched (the shipped `ListItem` shape).
                EventResult::Ignored
            }
            PointerPhase::Down => {
                if self.captures {
                    ctx.capture_pointer();
                }
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }
}

/// Whether the container claims hover for itself, and when relative to routing
/// the move into its child — the ordering the claim contract binds a container
/// to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GroupClaim {
    /// Never claims — the transparent container, hovered only via the path.
    Never,
    /// Claims *after* routing: the contract-following container, whose claim is
    /// a fallback the child's claim beats.
    AfterRouting,
    /// Claims *before* routing: the documented anti-pattern, kept as a
    /// negative control.
    BeforeRouting,
}

/// A container wrapping one hover leaf, recording what its **own**
/// `PaintCtx::is_hovered`/`EventCtx::is_hovered` reported — the
/// ancestor-on-the-claim-path case, which the two sibling leaves alone cannot
/// show. With `claims` set it also wants hover chrome of its own, claiming
/// either side of the route to exercise the ordering rule.
///
/// The child is an `Option` so a rebuild can *remove* it — the unmount case,
/// where the claimant stops existing between hover passes.
struct HoverGroup {
    probe: Rc<HoverProbe>,
    claims: GroupClaim,
    child: Option<crate::widget::ChildPod>,
}

impl HoverGroup {
    /// Whether this event is an uncaptured-move-shaped pass landing inside the
    /// container's own bounds — the same local hit test a leaf claims on.
    fn claims_on(&self, ctx: &EventCtx, event: &InputEvent) -> bool {
        let InputEvent::Pointer(p) = event else {
            return false;
        };
        let size = ctx.size();
        matches!(p.phase, PointerPhase::Move)
            && p.position.x >= 0.0
            && p.position.y >= 0.0
            && p.position.x < size.width
            && p.position.y < size.height
    }
}

impl crate::widget::Widget for HoverGroup {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        match &mut self.child {
            Some(child) => {
                let size = child.layout_child(ctx, bc);
                child.set_origin(Point::ZERO);
                size
            }
            // The same box with nothing in it, so removing the claimant
            // changes what is under the pointer without moving the container.
            None => bc.constrain(Size::new(100.0, 30.0)),
        }
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.probe.painted_hovered.set(ctx.is_hovered());
        if let Some(child) = &mut self.child {
            child.paint_child(ctx, scene);
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        self.probe.event_hovered.set(ctx.is_hovered());
        let claims = self.claims != GroupClaim::Never && self.claims_on(ctx, event);
        if claims && self.claims == GroupClaim::BeforeRouting {
            ctx.claim_hover();
        }
        let result = match &mut self.child {
            Some(child) => child.event_child(ctx, event),
            None => EventResult::Ignored,
        };
        if claims && self.claims == GroupClaim::AfterRouting {
            ctx.claim_hover();
        }
        result
    }
}

/// Two stacked hover leaves with a hit-tested route and a capture fast-path —
/// the minimum container that can show a claim moving between siblings.
struct HoverPair {
    top: crate::widget::ChildPod,
    bottom: crate::widget::ChildPod,
}

impl crate::widget::Widget for HoverPair {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.top.layout_child(ctx, bc);
        self.top.set_origin(Point::ZERO);
        self.bottom.layout_child(ctx, bc);
        self.bottom.set_origin(Point::new(0.0, 30.0));
        bc.max()
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.top.paint_child(ctx, scene);
        self.bottom.paint_child(ctx, scene);
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let releases = matches!(
            event,
            InputEvent::Pointer(p)
                if matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel)
        );
        for pod in [&mut self.top, &mut self.bottom] {
            if pod.is_active() {
                let r = pod.event_child(ctx, event);
                if releases {
                    pod.set_active(false);
                }
                return r;
            }
        }
        let pos = event.position();
        for pod in [&mut self.top, &mut self.bottom] {
            if pod.contains(pos) {
                return pod.event_child(ctx, event);
            }
        }
        EventResult::Ignored
    }
}

/// How one `HoverHarness` is shaped.
#[derive(Clone, Copy)]
struct HoverFixture {
    /// Leaves capture the pointer on `Down` (the drag case).
    captures: bool,
    /// Leaves follow the consumer contract (latched flag + change-gated redraw
    /// + paint-time self-correction).
    latches: bool,
    /// Wrap the top leaf in a [`HoverGroup`], so the claim path has an
    /// ancestor pod between the claimant and the root.
    nested: bool,
    /// Whether (and when) that container claims hover for itself.
    group_claims: GroupClaim,
    /// Rebuild the container without its child: the unmount case, where the
    /// pod holding the hover link is dropped by the view diff.
    drop_claimant: bool,
    /// Report [`ChangeFlags::NONE`] from that removal — the hand-rolled
    /// container outside this workspace, which drops a pod while reporting
    /// whatever it likes. The in-tree reconcilers report `LAYOUT | PAINT`,
    /// which is what the release used to lean on instead of flagging its own.
    silent_reconciler: bool,
}

struct HoverPairView {
    top: Rc<HoverProbe>,
    bottom: Rc<HoverProbe>,
    group: Rc<HoverProbe>,
    fixture: HoverFixture,
}

impl HoverPairView {
    fn leaf(&self, probe: &Rc<HoverProbe>) -> Box<dyn crate::widget::Widget> {
        Box::new(HoverLeaf {
            captures: self.fixture.captures,
            latches: self.fixture.latches,
            hovered: false,
            probe: probe.clone(),
        })
    }
}

impl View<()> for HoverPairView {
    type Element = HoverPair;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> HoverPair {
        let top: Box<dyn crate::widget::Widget> = if self.fixture.nested {
            Box::new(HoverGroup {
                probe: self.group.clone(),
                claims: self.fixture.group_claims,
                child: Some(crate::widget::ChildPod::new(self.leaf(&self.top))),
            })
        } else {
            self.leaf(&self.top)
        };
        HoverPair {
            top: crate::widget::ChildPod::new(top),
            bottom: crate::widget::ChildPod::new(self.leaf(&self.bottom)),
        }
    }
    fn rebuild(&self, p: &Self, e: &mut HoverPair, _c: &mut BuildCtx<'_>) -> ChangeFlags {
        // The only structural op this fixture performs: drop the nested
        // container's child pod, the way a real reconciler drops a truncated
        // or conditionally-removed child.
        let removes_child = self.fixture.drop_claimant && !p.fixture.drop_claimant;
        if !removes_child {
            return ChangeFlags::NONE;
        }
        let group = e
            .top
            .widget_mut()
            .downcast_mut::<HoverGroup>()
            .expect("the drop-claimant fixture is the nested one");
        group.child = None;
        if self.fixture.silent_reconciler {
            ChangeFlags::NONE
        } else {
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
    }
}

/// A `RenderRoot` over the hover fixture, plus its probes.
struct HoverHarness {
    root: RenderRoot<(), HoverPairView>,
    top: Rc<HoverProbe>,
    bottom: Rc<HoverProbe>,
    group: Rc<HoverProbe>,
    state: (),
    /// The shape the next rebuild re-states, so
    /// [`HoverHarness::rebuild_without_claimant`] can flip one flag without
    /// restating the rest.
    fixture: HoverFixture,
}

impl HoverHarness {
    /// The contract-following fixture: two flat leaves that latch their own
    /// hover flag, optionally capturing on `Down`.
    fn new(captures: bool) -> Self {
        Self::build(HoverFixture {
            captures,
            latches: true,
            nested: false,
            group_claims: GroupClaim::Never,
            drop_claimant: false,
            silent_reconciler: false,
        })
    }

    /// The negative control: leaves that claim hover but keep no flag of their
    /// own, so only frames the pipeline manufactures show up.
    fn without_consumer_flag() -> Self {
        Self::build(HoverFixture {
            captures: false,
            latches: false,
            nested: false,
            group_claims: GroupClaim::Never,
            drop_claimant: false,
            silent_reconciler: false,
        })
    }

    /// The top leaf wrapped in a container pod, for the path-semantics case.
    fn nested() -> Self {
        Self::build(HoverFixture {
            captures: false,
            latches: true,
            nested: true,
            group_claims: GroupClaim::Never,
            drop_claimant: false,
            silent_reconciler: false,
        })
    }

    /// The same nesting, with the container claiming hover for itself the way
    /// the contract requires: after routing the move into its child.
    fn nested_group_claiming(claims: GroupClaim) -> Self {
        Self::build(HoverFixture {
            captures: false,
            latches: true,
            nested: true,
            group_claims: claims,
            drop_claimant: false,
            silent_reconciler: false,
        })
    }

    /// The same nesting, with a container that drops its child while
    /// reporting no flags — the hand-rolled container outside this workspace
    /// the destructor route exists to cover.
    fn nested_group_with_silent_reconciler() -> Self {
        Self::build(HoverFixture {
            captures: false,
            latches: true,
            nested: true,
            group_claims: GroupClaim::AfterRouting,
            drop_claimant: false,
            silent_reconciler: true,
        })
    }

    fn build(fixture: HoverFixture) -> Self {
        let top = Rc::new(HoverProbe::default());
        let bottom = Rc::new(HoverProbe::default());
        let group = Rc::new(HoverProbe::default());
        let mut root: RenderRoot<(), HoverPairView> = RenderRoot::new();
        let mut state = ();
        let (t, b, g) = (top.clone(), bottom.clone(), group.clone());
        root.rebuild(
            &mut move |_: &mut ()| HoverPairView {
                top: t.clone(),
                bottom: b.clone(),
                group: g.clone(),
                fixture,
            },
            &mut state,
        );
        root.layout(Size::new(100.0, 60.0));
        HoverHarness {
            root,
            top,
            bottom,
            group,
            state,
            fixture,
        }
    }

    /// Rebuild with the nested container's child removed — the claimant
    /// unmounting between hover passes — and re-lay out, returning what the
    /// diff reported.
    fn rebuild_without_claimant(&mut self) -> ChangeFlags {
        self.fixture.drop_claimant = true;
        self.rebuild_current()
    }

    /// Rebuild restating the shape already on screen: nothing of this root's
    /// own is severed, so any hover end it performs came from elsewhere.
    fn rebuild_unchanged(&mut self) -> ChangeFlags {
        self.rebuild_current()
    }

    /// Re-run the diff against the fixture as it currently stands, then
    /// re-lay out, returning what the diff reported.
    fn rebuild_current(&mut self) -> ChangeFlags {
        let fixture = self.fixture;
        let (t, b, g) = (self.top.clone(), self.bottom.clone(), self.group.clone());
        let flags = self.root.rebuild(
            &mut move |_: &mut ()| HoverPairView {
                top: t.clone(),
                bottom: b.clone(),
                group: g.clone(),
                fixture,
            },
            &mut self.state,
        );
        self.root.layout(Size::new(100.0, 60.0));
        flags
    }

    /// Dispatch a pointer event at `(x, y)` in window space.
    fn dispatch(&mut self, phase: PointerPhase, x: f64, y: f64) -> EventOutcome {
        let event = InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        });
        self.root.event(&mut self.state, &event)
    }

    /// Move the pointer over the given leaf's middle.
    fn move_over(&mut self, leaf: Leaf) -> EventOutcome {
        match leaf {
            Leaf::Top => self.dispatch(PointerPhase::Move, 50.0, 15.0),
            Leaf::Bottom => self.dispatch(PointerPhase::Move, 50.0, 45.0),
        }
    }

    /// Paint the tree, refreshing both probes' recorded hover state.
    fn paint(&mut self) {
        let mut scene = RecordingScene::default();
        self.root.paint(&mut scene, FrameTime::ZERO);
    }

    /// `(top, bottom)` hover as the last paint reported it.
    fn painted(&mut self) -> (bool, bool) {
        self.paint();
        (
            self.top.painted_hovered.get(),
            self.bottom.painted_hovered.get(),
        )
    }
}

#[test]
fn an_uncaptured_move_claims_hover_and_paint_reports_it() {
    let mut h = HoverHarness::new(false);
    assert!(!h.root.is_hover_active(), "nothing is hovered at rest");
    assert_eq!(h.painted(), (false, false));

    let outcome = h.move_over(Leaf::Top);
    assert!(h.root.is_hover_active(), "the claim reached the root");
    assert!(
        !outcome.handled,
        "a hovering widget need not consume the move"
    );
    assert_eq!(
        h.painted(),
        (true, false),
        "the claimant reads as hovered, its sibling does not"
    );

    // A second move within the same leaf keeps the link (the claim is
    // re-recorded every pass) without re-reporting a change.
    h.dispatch(PointerPhase::Move, 60.0, 20.0);
    assert_eq!(h.painted(), (true, false));
}

#[test]
fn a_second_widgets_claim_clears_the_first_and_asks_for_a_repaint() {
    let mut h = HoverHarness::new(false);
    h.move_over(Leaf::Top);
    assert_eq!(h.painted(), (true, false));

    // The pointer moves onto the sibling. The container never has to clear
    // anything: the epoch advance strands the top pod's stamp.
    h.move_over(Leaf::Bottom);
    assert!(h.root.is_hover_active());
    assert_eq!(
        h.painted(),
        (false, true),
        "the previous claimant lost its link when the new one recorded"
    );

    // Both widgets need a repaint, and a repaint is global — one request
    // covers them. The *losing* side is what the root itself must guarantee:
    // moving onto a leaf that claims nothing still repaints.
    let outcome = h.dispatch(PointerPhase::Move, 50.0, 200.0);
    assert!(
        !h.root.is_hover_active(),
        "a move claiming nothing ends the hover"
    );
    assert!(
        outcome.needs_redraw,
        "the widget that lost hover cannot ask for the repaint itself"
    );
    assert_eq!(h.painted(), (false, false));

    // ...and the same move repeated is not a change any more.
    let settled = h.dispatch(PointerPhase::Move, 50.0, 200.0);
    assert!(
        !settled.needs_redraw,
        "an already-hoverless move requests nothing"
    );
}

#[test]
fn a_captured_move_cannot_claim_hover() {
    let mut h = HoverHarness::new(true);
    // Press the top leaf: it captures, and the `Down` itself ends any hover.
    h.dispatch(PointerPhase::Down, 50.0, 15.0);
    assert!(h.root.is_pointer_captured());
    assert!(!h.root.is_hover_active());

    // Drag: every one of these moves routes to the captured leaf, whose `Move`
    // arm hit-tests inside and calls `claim_hover()` — and must record nothing.
    h.dispatch(PointerPhase::Move, 50.0, 16.0);
    assert!(
        !h.root.is_hover_active(),
        "a captured pointer never creates hover"
    );
    assert_eq!(h.painted(), (false, false));

    // Dragging outside the leaf keeps routing to it (capture), still no hover.
    h.dispatch(PointerPhase::Move, 50.0, 45.0);
    assert!(!h.root.is_hover_active());
    assert_eq!(h.painted(), (false, false));

    // Release, then a fresh uncaptured move: hover is claimable again.
    h.dispatch(PointerPhase::Up, 50.0, 15.0);
    h.move_over(Leaf::Top);
    assert!(h.root.is_hover_active());
    assert_eq!(h.painted(), (true, false));
}

#[test]
fn a_down_up_or_cancel_ends_the_hover() {
    // Every pointer phase other than an uncaptured `Move` ends the link. `Up`
    // is in here for touch: a lifted finger sends no further move, so a tint
    // claimed during an uncaptured touch drag would otherwise stand for good.
    for ending in [PointerPhase::Down, PointerPhase::Up, PointerPhase::Cancel] {
        let mut h = HoverHarness::new(false);
        h.move_over(Leaf::Top);
        assert!(h.root.is_hover_active());

        let outcome = h.dispatch(ending, 50.0, 15.0);
        assert!(
            !h.root.is_hover_active(),
            "{ending:?} ends the hover link outright"
        );
        assert!(
            outcome.needs_redraw,
            "{ending:?} that dropped a hover asks for the repaint"
        );
        assert_eq!(h.painted(), (false, false));
    }
}

#[test]
fn a_non_pointer_pass_leaves_a_live_hover_standing() {
    let mut h = HoverHarness::new(false);
    h.move_over(Leaf::Top);
    assert_eq!(h.painted(), (true, false));

    // Neither a scroll, a key, nor the housekeeping broadcast is a hover pass:
    // the pointer has not moved, so the link must survive them untouched.
    h.root.event(
        &mut h.state,
        &InputEvent::Scroll {
            position: Point::new(50.0, 15.0),
            delta: crate::event::ScrollDelta::Lines(0.0, 1.0),
        },
    );
    assert!(h.root.is_hover_active());
    h.root.event(&mut h.state, &InputEvent::Housekeeping);
    assert!(h.root.is_hover_active());
    assert_eq!(h.painted(), (true, false));
}

#[test]
fn a_container_on_the_claim_path_reads_hovered_and_a_sibling_does_not() {
    // The recorded thing is a path, so hover is `:hover`-shaped: the claimant
    // and every ancestor enclosing it read hovered, nothing off the path does.
    let mut h = HoverHarness::nested();
    h.move_over(Leaf::Top);
    h.paint();
    assert!(h.top.painted_hovered.get(), "the claimant itself");
    assert!(
        h.group.painted_hovered.get(),
        "the container enclosing the claimant is on the path too"
    );
    assert!(
        !h.bottom.painted_hovered.get(),
        "a sibling leaf is off the path"
    );

    // Move onto the sibling: the container goes unhovered with its child, and
    // both reads agree about it on the next pass.
    h.move_over(Leaf::Bottom);
    h.paint();
    assert!(!h.group.painted_hovered.get());
    assert!(!h.top.painted_hovered.get());
    assert!(h.bottom.painted_hovered.get());
    h.move_over(Leaf::Top);
    assert!(
        !h.group.event_hovered.get(),
        "the event read reports the previous pass, like the leaf's"
    );
    h.move_over(Leaf::Top);
    assert!(
        h.group.event_hovered.get(),
        "the container observes the link its child holds"
    );
}

#[test]
fn an_ancestor_claiming_after_routing_loses_to_its_child_and_still_reads_hovered() {
    // A container that wants hover chrome of its own claims after routing the
    // move into its child. Only one claim per pass is recorded and the first one
    // recorded wins, so the child's claim is the one that lands; the container's
    // own late call is a silent no-op, and it reads hovered through the stamped
    // path anyway — which is what makes this ordering correct in every case.
    let mut h = HoverHarness::nested_group_claiming(GroupClaim::AfterRouting);
    let gain = h.move_over(Leaf::Top);
    h.paint();
    assert!(
        h.top.painted_hovered.get(),
        "the child under the pointer holds the link"
    );
    assert!(
        h.group.painted_hovered.get(),
        "the container is on that path, so it reads hovered too"
    );
    assert!(
        !h.bottom.painted_hovered.get(),
        "a sibling leaf is off the path"
    );
    assert!(gain.needs_redraw, "hover gain repaints");

    // The child's latched flag now agrees with the authoritative paint read, so
    // wandering on within the same widget settles instead of repainting.
    let settled = h.dispatch(PointerPhase::Move, 60.0, 20.0);
    assert!(!settled.needs_redraw, "an unchanged flag asks for nothing");
    h.paint();
    assert!(h.top.painted_hovered.get());
    assert!(h.group.painted_hovered.get());
}

#[test]
fn an_ancestor_claiming_before_routing_starves_its_subtree() {
    // The negative control for the ordering rule above, pinning the trap it
    // exists to prevent: a container that claims *before* forwarding is recorded
    // first, which closes the pass to every descendant. The child under the
    // pointer can never read hovered, so its hover chrome never appears — and
    // because its latched flag is corrected back to `false` at paint time, it
    // flips and asks for a frame again on every single move.
    let mut h = HoverHarness::nested_group_claiming(GroupClaim::BeforeRouting);
    h.move_over(Leaf::Top);
    h.paint();
    assert!(
        !h.top.painted_hovered.get(),
        "the ancestor's earlier claim made its child ineligible"
    );
    assert!(
        h.group.painted_hovered.get(),
        "the outermost claimant is the one holding the link here"
    );
    assert!(!h.bottom.painted_hovered.get());

    // Repaint-per-move: the flag never converges, because the event arm and the
    // authoritative paint read permanently disagree.
    let again = h.dispatch(PointerPhase::Move, 60.0, 20.0);
    assert!(
        again.needs_redraw,
        "the starved child re-flips its flag on every move"
    );
    h.paint();
    assert!(!h.top.painted_hovered.get(), "and still paints no chrome");
}

#[test]
fn hover_gain_is_repainted_by_the_consumers_own_flag() {
    let mut h = HoverHarness::new(false);
    // Entering a widget: the root manufactures nothing here (its mirror went
    // `false` → `true`, and it cannot know which widget cares), so the frame
    // comes from the claimant's own change-gated request.
    let gain = h.move_over(Leaf::Top);
    assert!(gain.needs_redraw, "hover gain repaints");
    assert_eq!(h.painted(), (true, false));

    // Wandering within the same widget claims again but changes nothing, so it
    // must not repaint per event.
    let settled = h.dispatch(PointerPhase::Move, 60.0, 20.0);
    assert!(!settled.needs_redraw, "an unchanged flag asks for nothing");

    // A handoff repaints both sides at once: the arriving leaf's flag changed
    // (it asks), and because a repaint is global that same frame is what lets
    // the departing leaf drop its chrome from the authoritative paint read.
    let handoff = h.move_over(Leaf::Bottom);
    assert!(handoff.needs_redraw, "a claimant handoff repaints");
    assert_eq!(
        h.painted(),
        (false, true),
        "one frame settles both the loss and the gain"
    );
}

#[test]
fn a_claimant_without_its_own_flag_gets_only_the_loss_frame() {
    // The negative control for the contract above: leaves that claim hover but
    // keep no flag of their own. Gain and handoff are invisible to the root
    // (its hover mirror is identity-free — `false` → `true` and `true` →
    // `true`), so nothing repaints for them, which is exactly why the
    // consumer's latched flag is normative rather than an optimization.
    let mut h = HoverHarness::without_consumer_flag();
    let gain = h.move_over(Leaf::Top);
    assert!(h.root.is_hover_active());
    assert!(!gain.needs_redraw, "no widget asked, and the root cannot");

    let handoff = h.move_over(Leaf::Bottom);
    assert!(h.root.is_hover_active());
    assert!(!handoff.needs_redraw, "a handoff is `true` → `true` here");

    // Loss is the one edge the root does cover, since no widget can see it.
    let loss = h.dispatch(PointerPhase::Move, 50.0, 200.0);
    assert!(!h.root.is_hover_active());
    assert!(loss.needs_redraw, "the root manufactures the loss frame");
}

#[test]
fn a_rebuild_that_removes_the_claimant_ends_the_hover() {
    // The one severance the epoch cannot strand: the claimant is dropped by a
    // view diff, so it will never see the `Move` that would have re-derived
    // the link. Its pod reports the drop and `rebuild` ends the hover.
    let mut h = HoverHarness::nested_group_claiming(GroupClaim::AfterRouting);
    h.move_over(Leaf::Top);
    h.paint();
    assert!(h.root.is_hover_active());
    assert!(h.top.painted_hovered.get(), "the claimant holds the link");
    assert!(
        h.group.painted_hovered.get(),
        "its container is on the path"
    );

    let flags = h.rebuild_without_claimant();
    assert!(
        !h.root.is_hover_active(),
        "the mirror cannot outlive the widget it described"
    );
    assert!(
        flags.contains(ChangeFlags::PAINT),
        "the correction states its own need for a frame, whatever the \
         reconciler that dropped the claimant reported"
    );

    // The survivor is the ancestor that was on the claim path: its own stamp
    // still names the epoch the claim recorded, so nothing but the epoch
    // advance keeps it from painting hover chrome for a child that is gone.
    h.paint();
    assert!(
        !h.group.painted_hovered.get(),
        "the surviving ancestor lost the link with its child"
    );
    assert!(
        !h.bottom.painted_hovered.get(),
        "and the sibling never had it"
    );

    // And the pipeline is not wedged: the next move over the same spot claims
    // cleanly, now for the container itself. (No frame is manufactured for
    // that gain — this container keeps no latched flag of its own, which is
    // the documented consumer-side half of the contract, not a pipeline job.)
    h.move_over(Leaf::Top);
    assert!(h.root.is_hover_active(), "the next move re-claims");
    h.paint();
    assert!(
        h.group.painted_hovered.get(),
        "the container is the claimant now"
    );
}

#[test]
fn a_silent_reconcilers_removal_still_carries_its_own_repaint() {
    // The reason the release flags `PAINT` itself rather than trusting the
    // diff to have reported one. The destructor route deliberately reaches
    // containers this workspace never sees, and such a container can drop the
    // claimant while reporting nothing — leaving the hover correctly ended but
    // the frame that shows it unrequested, on a pointer the user has stopped
    // moving.
    let mut h = HoverHarness::nested_group_with_silent_reconciler();
    h.move_over(Leaf::Top);
    assert!(h.root.is_hover_active());

    let flags = h.rebuild_without_claimant();
    assert!(!h.root.is_hover_active(), "the link ends either way");
    assert_eq!(
        flags,
        ChangeFlags::PAINT,
        "and the frame it needs comes from the release, not from the diff"
    );
}

#[test]
fn a_rebuild_that_keeps_the_claimant_leaves_the_hover_standing() {
    // The negative control for the release above, and the reason the mark is
    // gated on the *live* epoch rather than on "some pod with a stamp died":
    // an ordinary rebuild — including one that drops pods carrying stale
    // stamps — must not touch a link the pointer still rests on.
    let mut h = HoverHarness::nested_group_claiming(GroupClaim::AfterRouting);
    // Hover the top leaf, then hand the link to the sibling. The top pod chain
    // keeps its (now stale) stamp, which is what the next rebuild drops.
    h.move_over(Leaf::Top);
    h.move_over(Leaf::Bottom);
    assert!(h.root.is_hover_active());

    h.rebuild_without_claimant();
    assert!(
        h.root.is_hover_active(),
        "dropping a stale stamp is not a severance"
    );
    h.paint();
    assert!(
        h.bottom.painted_hovered.get(),
        "the widget actually under the pointer keeps its chrome"
    );
}

#[test]
fn another_roots_dying_claimant_cannot_end_this_roots_hover() {
    // Two roots on one thread. Each has run exactly one hover pass, so their
    // epoch counters hold the identical integer — the collision the root
    // identity exists to break. Without it, the first root's pods dying (its
    // window closing, a page tearing down) raise a mark the second root's
    // next rebuild drains, ending a hover the pointer is still resting on.
    let mut first = HoverHarness::nested_group_claiming(GroupClaim::AfterRouting);
    let mut second = HoverHarness::nested_group_claiming(GroupClaim::AfterRouting);
    first.move_over(Leaf::Top);
    second.move_over(Leaf::Top);
    assert_eq!(
        first.root.hover_epoch, second.root.hover_epoch,
        "the two roots' epochs collide, which is the whole premise"
    );
    assert!(first.root.is_hover_active());
    assert!(second.root.is_hover_active());

    // Drop the first root outright: every pod it owns runs the destructor
    // that reports a severed hover link, including the claimant's.
    drop(first);

    second.rebuild_unchanged();
    assert!(
        second.root.is_hover_active(),
        "a mark another root raised is none of this root's business"
    );
    second.paint();
    assert!(
        second.top.painted_hovered.get(),
        "the widget under the pointer keeps its chrome"
    );
}

#[test]
fn event_ctx_hover_reports_the_previous_pass_not_this_ones_claim() {
    let mut h = HoverHarness::new(false);
    // First move over the top leaf: it was not hovered when its handler ran.
    h.move_over(Leaf::Top);
    assert!(
        !h.top.event_hovered.get(),
        "a fresh claim does not retroactively flip `is_hovered`"
    );
    // Second move over the same leaf: now it observes the link it holds.
    h.move_over(Leaf::Top);
    assert!(
        h.top.event_hovered.get(),
        "the link recorded last pass is visible to this pass's handler"
    );
}
