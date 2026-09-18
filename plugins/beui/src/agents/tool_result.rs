//! Ports beUI's `tool-result` agent-interface part.
//!
//! **Source:** `components/agents/tool-result.tsx` of the beUI monorepo, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01.
//!
//! | upstream | here |
//! |---|---|
//! | trigger `min-h-9 gap-2 py-1` | [`TOOL_RESULT_HEADER_HEIGHT`], [`TOOL_RESULT_HEADER_GAP`] |
//! | `KindIcon` terminal / braces / wrench | [`ToolResultKind`] |
//! | `StatusIcon` spinner / check / cross / ban | [`ToolResultStatus`], crossfaded on [`TOOL_RESULT_MORPH`] |
//! | `getStatusLabel` Running/Completed/Failed/Cancelled | [`ToolResultStatus::label`] |
//! | title `font-medium text-foreground/90` | the title run |
//! | tool `font-mono text-[11px] text-muted-foreground/55` | the mono tool run |
//! | `meta` beside the title | [`ToolResultView::meta`] / [`ToolResultView::duration`] |
//! | `rotate: open ? 180 : 0` chevron | the reveal drives the rotation |
//! | `AgentDisclosure` reveal, `pl-6 pt-1.5`, `rounded-xl bg-muted/80` | the reveal lane over [`TOOL_RESULT_INDENT`] / [`TOOL_RESULT_PANEL_RADIUS`] |
//! | viewport `overflow-y-auto` `style={{maxHeight}}` | [`TOOL_RESULT_MAX_HEIGHT`], clipped |
//! | foot row: copy, retry, status word | [`TOOL_RESULT_FOOT_HEIGHT`] |
//!
//! # The payload is a child, not a string
//!
//! Upstream's `children` is arbitrary JSX, with `ToolResultOutput` as the
//! convenience wrapper that renders a highlighted string. Here the payload is a
//! **child view** — a [`code_block`](super::code_block) for terminal output or
//! a JSON response, a plain `text`, a table, whatever the app has — laid out at
//! its natural height inside the disclosed panel and clipped to the revealed
//! band. That keeps the slot as open as upstream's while letting the shipped
//! code panel do the code rendering.
//!
//! # The status morph
//!
//! A status change crossfades the old glyph out and the new one in on
//! [`TOOL_RESULT_MORPH`] rather than swapping instantly; a running result also
//! turns its spinner, which is a perpetual decorative loop and so asks for a
//! **paced** tick and freezes under `reduce_motion`. The crossfade is
//! deliberately restarted (not retargeted) on a real status change: it is a new
//! animation between a new pair of glyphs, not a re-aim of the running one.
//!
//! # Degradations against the web original
//!
//! - **No rolling text swap.** Upstream runs the title, meta, tool and status
//!   word through `ActionSwapRollText`, so each rolls when its value changes.
//!   The catalog's own roll cell is a `components::action_swap` construction
//!   over child views; a leaf panel shaping its own runs cannot reach it
//!   without becoming a container of four text widgets, so a changed label is
//!   re-shaped in place and only the *status glyph* morphs.
//! - **No scrolling.** Upstream's viewport is `overflow-y-auto` and follows the
//!   tail while running; a payload taller than
//!   [`ToolResultView::max_height`] is clipped at the cap here.
//! - **`collapseOnComplete` is the caller's.** Upstream opens itself when a run
//!   starts and closes itself when it finishes; this port is controlled end to
//!   end, so the app watches the status and passes the `open` it wants.
//! - **`on_copy` is the whole contract** — the component never touches a
//!   clipboard; the app's clipboard plugin does.

use std::rc::Rc;
use std::time::Duration;

use frust::Theme;
use frust::authoring::{
    Action, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, ErasedCallback, EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey,
    PaintCtx, PaintScene, Point, PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size,
    View, Widget, any, build_child, erase_callback, erase_callback_arg, rebuild_child,
    route_event_single, teardown_child, text::FontWeight, text::TextStyle, visit_children,
};

use crate::agents::code_block::{
    code_palette, code_style, draw_check, draw_chevron, draw_cross, draw_spinner, spinner_angle,
};
use crate::motion::Ramp;
use crate::press::{inside, is_activation_key, presses};
use crate::style::{self, with_alpha};
use crate::text::{LabelRun, SHAPING_INK, ThemeTextType, themed_style};
use crate::tokens::motion::{EASE_OUT, SPRING_PANEL};
use crate::tokens::{BEUI_LIGHT, BeuiTokens};

/// The header row's height, in logical px (`min-h-9`).
pub const TOOL_RESULT_HEADER_HEIGHT: f64 = 36.0;

/// The gap between the header's parts, in logical px (`gap-2`).
pub const TOOL_RESULT_HEADER_GAP: f64 = style::GAP_MD;

/// How far the disclosed panel is indented, in logical px (`pl-6`).
pub const TOOL_RESULT_INDENT: f64 = 24.0;

/// The gap between the header and the panel, in logical px (`pt-1.5`).
pub const TOOL_RESULT_CONTENT_GAP: f64 = 6.0;

/// The panel's corner radius (`rounded-xl`).
pub const TOOL_RESULT_PANEL_RADIUS: f64 = style::RADIUS_XL;

/// The panel's inner padding, in logical px (`p-3`).
pub const TOOL_RESULT_PANEL_PADDING: f64 = 12.0;

/// The action row's height, in logical px (`size-7` buttons plus `pb-1.5`).
pub const TOOL_RESULT_FOOT_HEIGHT: f64 = 34.0;

/// One action affordance's box, in logical px (`size-7`).
pub const TOOL_RESULT_ACTION_BOX: f64 = 28.0;

/// The height cap on the disclosed panel, in logical px (`maxHeight = 220`).
pub const TOOL_RESULT_MAX_HEIGHT: f64 = 220.0;

/// The title's type size, in logical px (`text-sm font-medium`).
pub const TOOL_RESULT_TITLE_SIZE: f64 = style::TEXT_SM;

/// The meta and status type size, in logical px (`text-[11px]`).
pub const TOOL_RESULT_META_SIZE: f64 = 11.0;

/// Alpha the title is painted at (`text-foreground/90`).
pub const TOOL_RESULT_TITLE_ALPHA: f32 = 0.9;

/// Alpha the tool name is painted at (`text-muted-foreground/55`).
pub const TOOL_RESULT_TOOL_ALPHA: f32 = 0.55;

/// Alpha the meta value is painted at (`text-muted-foreground/60`).
pub const TOOL_RESULT_META_ALPHA: f32 = 0.6;

/// The ramp the disclosure plays, in both directions.
pub const TOOL_RESULT_REVEAL: Ramp = Ramp::spring(SPRING_PANEL);

/// The ramp a status-glyph crossfade plays.
pub const TOOL_RESULT_MORPH: Ramp = Ramp::eased(Duration::from_millis(220), EASE_OUT);

/// How a tool run ended, or that it has not (`ToolResultStatus`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolResultStatus {
    /// `"running"` — still executing.
    #[default]
    Running,
    /// `"success"` — finished cleanly.
    Success,
    /// `"error"` — finished with a failure.
    Error,
    /// `"cancelled"` — stopped before finishing.
    Cancelled,
}

impl ToolResultStatus {
    /// The word the header and the action row show (`getStatusLabel`).
    pub const fn label(self) -> &'static str {
        match self {
            ToolResultStatus::Running => "Running",
            ToolResultStatus::Success => "Completed",
            ToolResultStatus::Error => "Failed",
            ToolResultStatus::Cancelled => "Cancelled",
        }
    }

    /// Whether the run is still going.
    pub const fn is_running(self) -> bool {
        matches!(self, ToolResultStatus::Running)
    }
}

/// What kind of tool produced the result (`ToolResultKind`), which picks the
/// leading glyph.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolResultKind {
    /// A shell command — the terminal glyph (`SquareTerminal`).
    Terminal,
    /// A network request — the braces glyph (`Braces`).
    Request,
    /// Anything else — the wrench glyph (`Wrench`).
    #[default]
    Custom,
}

/// Render `duration` the way an agent transcript shows one: sub-second in
/// milliseconds, seconds to one decimal, and minutes split out past sixty
/// seconds.
///
/// An addition — upstream's `meta` is a caller-formatted `ReactNode`. The
/// formatter is provided because every consumer of a tool-result panel needs
/// the same three cases, and [`ToolResultView::duration`] runs a value through
/// it so a caller does not have to.
pub fn format_duration(duration: Duration) -> String {
    let millis = duration.as_millis();
    if millis < 1_000 {
        return format!("{millis}ms");
    }
    let seconds = duration.as_secs_f64();
    if seconds < 60.0 {
        return format!("{seconds:.1}s");
    }
    let minutes = duration.as_secs() / 60;
    let rest = duration.as_secs() % 60;
    format!("{minutes}m {rest:02}s")
}

/// A view-held callback, erased on build.
type OnState<State> = Rc<dyn Fn(&mut State)>;
/// A view-held one-argument callback, erased on build.
type OnArg<State, A> = Rc<dyn Fn(&mut State, A)>;

/// A declarative beUI tool-result panel. See the [module docs](self).
pub struct ToolResultView<State: 'static> {
    tool: String,
    title: String,
    meta: Option<String>,
    payload: AnyView<State>,
    status: ToolResultStatus,
    kind: ToolResultKind,
    open: bool,
    max_height: f64,
    on_open_change: Option<OnArg<State, bool>>,
    on_copy: Option<OnState<State>>,
    on_retry: Option<OnState<State>>,
}

/// Create a tool-result disclosure headed by `title`, attributed to `tool`, and
/// disclosing `payload`.
///
/// **Controlled**: chain [`ToolResultView::open`] and
/// [`ToolResultView::on_open_change`].
pub fn tool_result<State: 'static, V: View<State>>(
    tool: impl Into<String>,
    title: impl Into<String>,
    payload: V,
) -> ToolResultView<State> {
    ToolResultView {
        tool: tool.into(),
        title: title.into(),
        meta: None,
        payload: any(payload),
        status: ToolResultStatus::default(),
        kind: ToolResultKind::default(),
        open: true,
        max_height: TOOL_RESULT_MAX_HEIGHT,
        on_open_change: None,
        on_copy: None,
        on_retry: None,
    }
}

impl<State: 'static> ToolResultView<State> {
    /// How the run ended, or that it has not (`status`).
    pub fn status(mut self, status: ToolResultStatus) -> Self {
        self.status = status;
        self
    }

    /// Which leading glyph the header carries (`kind`).
    pub fn kind(mut self, kind: ToolResultKind) -> Self {
        self.kind = kind;
        self
    }

    /// The small value shown beside the title (`meta`).
    pub fn meta(mut self, meta: impl Into<String>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    /// The run's elapsed time, shown as the meta value through
    /// [`format_duration`].
    pub fn duration(self, duration: Duration) -> Self {
        self.meta(format_duration(duration))
    }

    /// Whether the payload is disclosed (`open`).
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Cap the disclosed panel at `max_height` logical px (`maxHeight`).
    pub fn max_height(mut self, max_height: f64) -> Self {
        self.max_height = max_height.max(0.0);
        self
    }

    /// Report the disclosure a press on the header asks for.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_change: F) -> Self {
        self.on_open_change = Some(Rc::new(on_change));
        self
    }

    /// Report a press on the copy affordance. Wiring this is what paints it.
    pub fn on_copy<F: Fn(&mut State) + 'static>(mut self, on_copy: F) -> Self {
        self.on_copy = Some(Rc::new(on_copy));
        self
    }

    /// Report a press on the run-again affordance (`onRetry`). Wiring this is
    /// what paints it.
    pub fn on_retry<F: Fn(&mut State) + 'static>(mut self, on_retry: F) -> Self {
        self.on_retry = Some(Rc::new(on_retry));
        self
    }
}

/// Which affordance the roving cursor sits on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ResultTarget {
    /// The header, which toggles the disclosure.
    #[default]
    Header,
    /// The copy affordance in the action row.
    Copy,
    /// The run-again affordance in the action row.
    Retry,
}

/// The retained widget for a [`ToolResultView`].
pub struct ToolResultWidget {
    tool: LabelRun,
    tool_text: String,
    title: LabelRun,
    title_text: String,
    meta: Option<LabelRun>,
    meta_text: Option<String>,
    status_label: LabelRun,
    payload: ChildPod,
    status: ToolResultStatus,
    /// The glyph the crossfade is leaving, while one runs.
    previous_status: Option<ToolResultStatus>,
    kind: ToolResultKind,
    open: bool,
    max_height: f64,
    /// The disclosure lane.
    reveal: crate::press::Lane,
    /// The status-glyph crossfade.
    morph: crate::press::Lane,
    width: f64,
    /// The payload's natural height, measured every layout pass.
    payload_height: f64,
    focused: ResultTarget,
    hovered: Option<ResultTarget>,
    captured: Option<ResultTarget>,
    on_open_change: Option<ErasedArgCallback<bool>>,
    on_copy: Option<ErasedCallback>,
    on_retry: Option<ErasedCallback>,
}

/// The title style (`text-sm font-medium`), in the theme's `label_large`
/// family.
fn title_style(theme: Option<&Theme>) -> TextStyle {
    let style = TextStyle {
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(TOOL_RESULT_TITLE_SIZE as f32, SHAPING_INK)
    };
    themed_style(style, ThemeTextType::LabelLarge, theme)
}

impl<State: 'static> View<State> for ToolResultView<State> {
    type Element = ToolResultWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ToolResultWidget {
        ToolResultWidget {
            tool: LabelRun::new(self.tool.clone()),
            tool_text: self.tool.clone(),
            title: LabelRun::new(self.title.clone()),
            title_text: self.title.clone(),
            meta: self.meta.as_ref().map(LabelRun::new),
            meta_text: self.meta.clone(),
            status_label: LabelRun::new(self.status.label()),
            payload: build_child(&self.payload, ctx),
            status: self.status,
            previous_status: None,
            kind: self.kind,
            open: self.open,
            max_height: self.max_height,
            reveal: crate::press::Lane::at_rest(
                TOOL_RESULT_REVEAL,
                if self.open { 1.0 } else { 0.0 },
            ),
            morph: crate::press::Lane::at_rest(TOOL_RESULT_MORPH, 1.0),
            width: 0.0,
            payload_height: 0.0,
            focused: ResultTarget::default(),
            hovered: None,
            captured: None,
            on_open_change: self.on_open_change.as_ref().map(erase_callback_arg),
            on_copy: self.on_copy.as_ref().map(erase_callback),
            on_retry: self.on_retry.as_ref().map(erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ToolResultWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_open_change = self.on_open_change.as_ref().map(erase_callback_arg);
        element.on_copy = self.on_copy.as_ref().map(erase_callback);
        element.on_retry = self.on_retry.as_ref().map(erase_callback);
        let mut flags = ChangeFlags::NONE;

        if element.tool_text != self.tool {
            element.tool = LabelRun::new(self.tool.clone());
            element.tool_text = self.tool.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.title_text != self.title {
            element.title = LabelRun::new(self.title.clone());
            element.title_text = self.title.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.meta_text != self.meta {
            element.meta = self.meta.as_ref().map(LabelRun::new);
            element.meta_text = self.meta.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.kind != self.kind {
            element.kind = self.kind;
            flags |= ChangeFlags::PAINT;
        }
        if element.status != self.status {
            // A real status change is a new crossfade between a new pair of
            // glyphs, so the lane is restarted rather than re-aimed.
            element.previous_status = Some(element.status);
            element.status = self.status;
            element.status_label = LabelRun::new(self.status.label());
            element.morph = crate::press::Lane::at_rest(TOOL_RESULT_MORPH, 0.0);
            element.morph.retarget(1.0);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.max_height != self.max_height {
            element.max_height = self.max_height;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.open != self.open {
            element.open = self.open;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Retargeted, never restarted: a rebuild re-passing the flag the lane is
        // already flying toward leaves its clock alone.
        element
            .reveal
            .retarget(if element.open { 1.0 } else { 0.0 });

        flags | rebuild_child(&prev.payload, &self.payload, &mut element.payload, ctx)
    }

    fn teardown(&self, element: &mut ToolResultWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.payload, &mut element.payload, ctx);
    }
}

/// Paint a circle outline of `box_size` at `origin`.
fn stroke_circle(scene: &mut dyn PaintScene, origin: Point, box_size: f64, color: Color) {
    let inset = box_size * 0.08;
    let rect = Rect::new(inset, inset, box_size - inset, box_size - inset);
    let circle = RoundedRect::from_rect(rect, (box_size - inset * 2.0) / 2.0);
    scene.stroke_path(
        origin,
        &Shape::to_path(&circle, style::PATH_TOLERANCE),
        1.3,
        &Brush::Solid(color),
    );
}

/// Paint the `Ban` glyph — a circle with a slash through it.
fn draw_ban(scene: &mut dyn PaintScene, origin: Point, box_size: f64, color: Color) {
    stroke_circle(scene, origin, box_size, color);
    let mut slash = BezPath::new();
    slash.move_to(Point::new(box_size * 0.28, box_size * 0.28));
    slash.line_to(Point::new(box_size * 0.72, box_size * 0.72));
    scene.stroke_path(origin, &slash, 1.3, &Brush::Solid(color));
}

/// Paint one kind's leading glyph.
fn draw_kind(
    scene: &mut dyn PaintScene,
    kind: ToolResultKind,
    origin: Point,
    box_size: f64,
    color: Color,
) {
    let unit = box_size / 16.0;
    let mut path = BezPath::new();
    match kind {
        ToolResultKind::Terminal => {
            // A prompt caret and a command line inside a rounded frame.
            let frame = RoundedRect::from_rect(
                Rect::new(unit * 2.0, unit * 3.0, unit * 14.0, unit * 13.0),
                unit * 2.0,
            );
            scene.stroke_path(
                origin,
                &Shape::to_path(&frame, style::PATH_TOLERANCE),
                1.2,
                &Brush::Solid(color),
            );
            path.move_to(Point::new(unit * 5.0, unit * 6.5));
            path.line_to(Point::new(unit * 7.5, unit * 8.0));
            path.line_to(Point::new(unit * 5.0, unit * 9.5));
            path.move_to(Point::new(unit * 9.0, unit * 10.0));
            path.line_to(Point::new(unit * 11.5, unit * 10.0));
        }
        ToolResultKind::Request => {
            // `{ }` — the braces glyph.
            path.move_to(Point::new(unit * 6.5, unit * 3.0));
            path.line_to(Point::new(unit * 5.0, unit * 4.5));
            path.line_to(Point::new(unit * 5.0, unit * 11.5));
            path.line_to(Point::new(unit * 6.5, unit * 13.0));
            path.move_to(Point::new(unit * 9.5, unit * 3.0));
            path.line_to(Point::new(unit * 11.0, unit * 4.5));
            path.line_to(Point::new(unit * 11.0, unit * 11.5));
            path.line_to(Point::new(unit * 9.5, unit * 13.0));
        }
        ToolResultKind::Custom => {
            // A wrench: a head and a shaft.
            path.move_to(Point::new(unit * 10.5, unit * 3.0));
            path.line_to(Point::new(unit * 13.0, unit * 5.5));
            path.line_to(Point::new(unit * 10.5, unit * 8.0));
            path.line_to(Point::new(unit * 8.5, unit * 6.0));
            path.close_path();
            path.move_to(Point::new(unit * 9.0, unit * 7.0));
            path.line_to(Point::new(unit * 3.5, unit * 12.5));
        }
    }
    scene.stroke_path(origin, &path, 1.2, &Brush::Solid(color));
}

impl ToolResultWidget {
    /// Whether a copy affordance is painted.
    fn copyable(&self) -> bool {
        self.on_copy.is_some()
    }

    /// Whether a run-again affordance is painted.
    fn retryable(&self) -> bool {
        self.on_retry.is_some()
    }

    /// Whether the panel carries an action row at all.
    fn has_foot(&self) -> bool {
        self.copyable() || self.retryable()
    }

    /// The action row's height, or zero when there is none.
    fn foot_height(&self) -> f64 {
        if self.has_foot() {
            TOOL_RESULT_FOOT_HEIGHT
        } else {
            0.0
        }
    }

    /// The panel's natural (uncapped) height.
    fn panel_natural(&self) -> f64 {
        self.payload_height + TOOL_RESULT_PANEL_PADDING * 2.0 + self.foot_height()
    }

    /// The panel's height after the cap.
    fn panel_height(&self) -> f64 {
        self.panel_natural().min(self.max_height)
    }

    /// The whole disclosed block's height, tracking the reveal.
    fn content_height(&self) -> f64 {
        (self.reveal.value().clamp(0.0, 1.0) * (TOOL_RESULT_CONTENT_GAP + self.panel_height()))
            .max(0.0)
    }

    /// The panel's box, in widget-local space.
    fn panel_rect(&self) -> Rect {
        Rect::from_origin_size(
            Point::new(
                TOOL_RESULT_INDENT,
                TOOL_RESULT_HEADER_HEIGHT + TOOL_RESULT_CONTENT_GAP,
            ),
            Size::new(
                (self.width - TOOL_RESULT_INDENT).max(0.0),
                self.panel_height(),
            ),
        )
    }

    /// The header's box, in widget-local space.
    fn header_rect(&self) -> Rect {
        Rect::from_origin_size(
            Point::ORIGIN,
            Size::new(self.width, TOOL_RESULT_HEADER_HEIGHT),
        )
    }

    /// The action affordances the panel paints, in visual order.
    fn actions(&self) -> Vec<ResultTarget> {
        let mut actions = Vec::new();
        if self.copyable() {
            actions.push(ResultTarget::Copy);
        }
        if self.retryable() {
            actions.push(ResultTarget::Retry);
        }
        actions
    }

    /// Action `target`'s box, in widget-local space — `None` when it is not
    /// painted or the panel is closed.
    fn action_rect(&self, target: ResultTarget) -> Option<Rect> {
        if self.reveal.value() <= 0.5 {
            return None;
        }
        let slot = self.actions().iter().position(|t| *t == target)?;
        let panel = self.panel_rect();
        let y = panel.y1 - TOOL_RESULT_FOOT_HEIGHT
            + (TOOL_RESULT_FOOT_HEIGHT - TOOL_RESULT_ACTION_BOX) / 2.0;
        let x =
            panel.x0 + style::GAP_MD + slot as f64 * (TOOL_RESULT_ACTION_BOX + style::spacing(0.5));
        Some(Rect::from_origin_size(
            Point::new(x, y),
            Size::new(TOOL_RESULT_ACTION_BOX, TOOL_RESULT_ACTION_BOX),
        ))
    }

    /// The affordance under a widget-local `pos`, if any.
    fn hit(&self, pos: Point) -> Option<ResultTarget> {
        for target in self.actions() {
            if let Some(rect) = self.action_rect(target)
                && rect.contains(pos)
            {
                return Some(target);
            }
        }
        if self.header_rect().contains(pos) {
            return Some(ResultTarget::Header);
        }
        None
    }

    /// The affordances the roving cursor can reach, in visual order.
    fn targets(&self) -> Vec<ResultTarget> {
        let mut targets = vec![ResultTarget::Header];
        if self.open {
            targets.extend(self.actions());
        }
        targets
    }

    /// The next affordance `step` places from the focused one, wrapping.
    fn step_target(&self, step: isize) -> Option<ResultTarget> {
        let targets = self.targets();
        if targets.is_empty() {
            return None;
        }
        let at = targets.iter().position(|t| *t == self.focused).unwrap_or(0);
        let next = (at as isize + step).rem_euclid(targets.len() as isize) as usize;
        targets.get(next).copied()
    }

    /// Report the decision `target` asks for. Fires exactly one callback.
    fn activate(&mut self, ctx: &mut EventCtx, target: ResultTarget) {
        match target {
            ResultTarget::Header => {
                let next = !self.open;
                if let Some(on_change) = self.on_open_change.as_mut() {
                    on_change(ctx, next);
                }
            }
            ResultTarget::Copy => {
                if let Some(on_copy) = self.on_copy.as_mut() {
                    on_copy(ctx);
                }
            }
            ResultTarget::Retry => {
                if let Some(on_retry) = self.on_retry.as_mut() {
                    on_retry(ctx);
                }
            }
        }
    }

    /// The ink `status` reads in.
    fn status_ink(status: ToolResultStatus, theme: Option<&Theme>) -> Color {
        let tokens = BeuiTokens::resolve(theme);
        let palette = code_palette(theme);
        match status {
            ToolResultStatus::Running => palette.function,
            ToolResultStatus::Success => tokens.success,
            ToolResultStatus::Error => theme.map_or(BEUI_LIGHT.destructive, |t| t.scheme().error),
            ToolResultStatus::Cancelled => palette.comment,
        }
    }

    /// Paint `status`'s glyph, faded to `alpha`.
    fn draw_status(
        scene: &mut dyn PaintScene,
        status: ToolResultStatus,
        at: Point,
        box_size: f64,
        ink: Color,
        alpha: f32,
        angle: f64,
    ) {
        let ink = with_alpha(ink, ink.components[3] * alpha);
        match status {
            ToolResultStatus::Running => draw_spinner(scene, at, box_size, angle, ink),
            ToolResultStatus::Success => {
                stroke_circle(scene, at, box_size, ink);
                draw_check(scene, at, box_size, ink);
            }
            ToolResultStatus::Error => {
                stroke_circle(scene, at, box_size, ink);
                draw_cross(scene, at, box_size, ink);
            }
            ToolResultStatus::Cancelled => draw_ban(scene, at, box_size, ink),
        }
    }
}

impl Widget for ToolResultWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.width = bc.max().width;
        let theme = Theme::from_layout_ctx(ctx);
        self.title.layout(ctx, &title_style(theme));
        // The tool name, the status word and the meta keep the mono family
        // `code_style` names explicitly.
        let mono = code_style(TOOL_RESULT_META_SIZE);
        self.tool.layout(ctx, &mono);
        self.status_label.layout(ctx, &mono);
        if let Some(meta) = &mut self.meta {
            meta.layout(ctx, &mono);
        }

        // The payload is always laid out at its natural height and stays laid
        // out, so the reveal never re-measures it mid-flight.
        let panel_width = (self.width - TOOL_RESULT_INDENT).max(0.0);
        let inner = BoxConstraints::new(
            Size::ZERO,
            Size::new(
                (panel_width - TOOL_RESULT_PANEL_PADDING * 2.0).max(0.0),
                bc.max().height,
            ),
        );
        self.payload_height = self.payload.layout_child(ctx, &inner).height;
        let panel = self.panel_rect();
        self.payload.set_origin(Point::new(
            panel.x0 + TOOL_RESULT_PANEL_PADDING,
            panel.y0 + TOOL_RESULT_PANEL_PADDING,
        ));

        bc.constrain(Size::new(
            self.width,
            TOOL_RESULT_HEADER_HEIGHT + self.content_height(),
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let palette = code_palette(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let ink_now = Self::status_ink(self.status, theme);
        let ink_was = self
            .previous_status
            .map(|status| Self::status_ink(status, theme));
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();

        let before = self.reveal.value();
        let moving = if reduce_motion {
            self.reveal.snap();
            self.morph.snap();
            self.previous_status = None;
            false
        } else {
            let reveal = self.reveal.advance(now);
            if !self.morph.advance(now) {
                self.previous_status = None;
            }
            reveal
        };
        let morph = self.morph.value().clamp(0.0, 1.0);

        // ---- the header ------------------------------------------------------
        let mut x = origin.x;
        let icon_y = origin.y + (TOOL_RESULT_HEADER_HEIGHT - style::ICON_SIZE) / 2.0;
        draw_kind(
            scene,
            self.kind,
            Point::new(x, icon_y),
            style::ICON_SIZE,
            palette.comment,
        );
        x += style::ICON_SIZE + TOOL_RESULT_HEADER_GAP;

        let title_size = self.title.size();
        self.title.paint(
            Point::new(
                x,
                origin.y + (TOOL_RESULT_HEADER_HEIGHT - title_size.height) / 2.0,
            ),
            with_alpha(palette.plain, TOOL_RESULT_TITLE_ALPHA),
            scene,
        );
        x += title_size.width + TOOL_RESULT_HEADER_GAP;

        if let Some(meta) = &self.meta {
            let measured = meta.size();
            meta.paint(
                Point::new(
                    x,
                    origin.y + (TOOL_RESULT_HEADER_HEIGHT - measured.height) / 2.0,
                ),
                with_alpha(palette.comment, TOOL_RESULT_META_ALPHA),
                scene,
            );
            x += measured.width + TOOL_RESULT_HEADER_GAP;
        }

        let tool_size = self.tool.size();
        self.tool.paint(
            Point::new(
                x,
                origin.y + (TOOL_RESULT_HEADER_HEIGHT - tool_size.height) / 2.0,
            ),
            with_alpha(palette.comment, TOOL_RESULT_TOOL_ALPHA),
            scene,
        );

        // The right cluster: status word, status glyph, chevron.
        let chevron_box = style::ICON_SIZE;
        let mut right = origin.x + size.width - chevron_box;
        draw_chevron(
            scene,
            Point::new(
                right + chevron_box / 2.0,
                origin.y + TOOL_RESULT_HEADER_HEIGHT / 2.0,
            ),
            chevron_box,
            self.reveal.value().clamp(0.0, 1.0) * std::f64::consts::PI,
            palette.comment,
        );

        let status_size = self.status_label.size();
        right -= status_size.width + style::GAP_SM;
        self.status_label.paint(
            Point::new(
                right,
                origin.y + (TOOL_RESULT_HEADER_HEIGHT - status_size.height) / 2.0,
            ),
            ink_now,
            scene,
        );

        let mark_box = style::ICON_SIZE * 0.85;
        right -= mark_box + style::GAP_SM / 2.0;
        let mark_at = Point::new(
            right,
            origin.y + (TOOL_RESULT_HEADER_HEIGHT - mark_box) / 2.0,
        );
        let angle = spinner_angle(now, reduce_motion);
        if let (Some(was), Some(ink)) = (self.previous_status, ink_was)
            && morph < 1.0
        {
            Self::draw_status(
                scene,
                was,
                mark_at,
                mark_box,
                ink,
                1.0 - morph as f32,
                angle,
            );
        }
        Self::draw_status(
            scene,
            self.status,
            mark_at,
            mark_box,
            ink_now,
            morph as f32,
            angle,
        );
        if self.status.is_running() && !reduce_motion {
            ctx.request_frame_paced();
        }

        // ---- the disclosed panel --------------------------------------------
        let content = self.content_height();
        if content > 0.5 {
            let panel = self.panel_rect();
            let band = (content - TOOL_RESULT_CONTENT_GAP).max(0.0);
            let at = Point::new(origin.x + panel.x0, origin.y + panel.y0);
            let panel_size = Size::new(panel.width(), band);
            scene.fill_rounded_rect(at, panel_size, TOOL_RESULT_PANEL_RADIUS, palette.surface);
            scene.push_clip_rounded(at, panel_size, TOOL_RESULT_PANEL_RADIUS);

            let body = (band - self.foot_height()).max(0.0);
            if body > 0.5 {
                scene.push_clip(at, Size::new(panel.width(), body));
                self.payload.paint_child(ctx, scene);
                scene.pop_clip();
            }

            if self.has_foot() {
                for target in self.actions() {
                    let Some(rect) = self.action_rect(target) else {
                        continue;
                    };
                    let action_at = Point::new(origin.x + rect.x0, origin.y + rect.y0);
                    let hovered = self.hovered == Some(target);
                    if hovered {
                        scene.fill_rounded_rect(
                            action_at,
                            rect.size(),
                            style::RADIUS_MD,
                            with_alpha(palette.plain, style::HOVER_WASH_ALPHA),
                        );
                    }
                    let glyph_at = Point::new(
                        action_at.x + (TOOL_RESULT_ACTION_BOX - style::ICON_SIZE) / 2.0,
                        action_at.y + (TOOL_RESULT_ACTION_BOX - style::ICON_SIZE) / 2.0,
                    );
                    let ink = if hovered {
                        palette.plain
                    } else {
                        palette.comment
                    };
                    match target {
                        ResultTarget::Copy => {
                            draw_kind(
                                scene,
                                ToolResultKind::Request,
                                glyph_at,
                                style::ICON_SIZE,
                                ink,
                            );
                        }
                        _ => draw_retry(scene, glyph_at, style::ICON_SIZE, ink),
                    }
                }
                let measured = self.status_label.size();
                self.status_label.paint(
                    Point::new(
                        origin.x + panel.x1 - style::GAP_MD - measured.width,
                        origin.y + panel.y1 - TOOL_RESULT_FOOT_HEIGHT
                            + (TOOL_RESULT_FOOT_HEIGHT - measured.height) / 2.0,
                    ),
                    with_alpha(palette.comment, TOOL_RESULT_META_ALPHA),
                    scene,
                );
            }
            scene.pop_clip();
        }

        // The reveal feeds the reported height, so a bare frame request would let
        // the disclosure freeze on the intra-frame layout skip. Ask for layout
        // while it moves, and once more on the frame the value changed — which
        // covers the landing frame and the `reduce_motion` snap.
        if moving || self.reveal.value() != before {
            ctx.request_layout();
        }
        if morph < 1.0 {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            self.payload.event_child(ctx, event);
            return EventResult::Ignored;
        }
        // The payload is routed first, so an interactive body wins over the
        // panel's own fallback handling.
        if route_event_single(&mut self.payload, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        match event {
            InputEvent::Key(key) => {
                let step = match &key.key {
                    Key::Named(NamedKey::ArrowDown) => Some(1),
                    Key::Named(NamedKey::ArrowUp) => Some(-1),
                    _ => None,
                };
                if let Some(step) = step {
                    let Some(next) = self.step_target(step) else {
                        return EventResult::Ignored;
                    };
                    self.focused = next;
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if is_activation_key(key) {
                    let target = self.focused;
                    if !self.targets().contains(&target) {
                        return EventResult::Ignored;
                    }
                    self.activate(ctx, target);
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) || !inside(p.position, ctx.size()) {
                        return EventResult::Ignored;
                    }
                    let Some(target) = self.hit(p.position) else {
                        return EventResult::Ignored;
                    };
                    self.captured = Some(target);
                    self.focused = target;
                    ctx.capture_pointer();
                    ctx.request_focus();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    if self.captured.is_some() {
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                        return EventResult::Handled;
                    }
                    let over = self.hit(p.position);
                    if over.is_some() {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.hovered != over {
                        self.hovered = over;
                        ctx.request_redraw();
                    }
                    EventResult::Ignored
                }
                PointerPhase::Up => {
                    let Some(armed) = self.captured.take() else {
                        return EventResult::Ignored;
                    };
                    if self.hit(p.position) == Some(armed) {
                        self.activate(ctx, armed);
                    }
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if self.captured.take().is_none() {
                        return EventResult::Ignored;
                    }
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = format!(
            "{} ({}) — {}",
            self.title_text,
            self.tool_text,
            self.status.label()
        );
        ctx.push_node(Role::Button, |node| {
            node.set_label(label.as_str());
            node.set_expanded(self.open);
            node.add_action(Action::Click);
        });
        if self.open {
            for (target, name) in [
                (ResultTarget::Copy, "Copy result"),
                (ResultTarget::Retry, "Run again"),
            ] {
                if self.actions().contains(&target) {
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(name);
                        node.add_action(Action::Click);
                    });
                }
            }
            // Only a disclosed payload is published: a closed one is `inert` and
            // `aria-hidden` upstream, and here it is not painted or routed to.
            self.payload.semantics_child(ctx);
        }
    }

    visit_children!(payload);
}

/// Paint the run-again glyph — a three-quarter arrow loop (`RotateCcw`).
fn draw_retry(scene: &mut dyn PaintScene, origin: Point, box_size: f64, color: Color) {
    let radius = box_size * 0.32;
    let centre = Point::new(box_size / 2.0, box_size / 2.0);
    let mut path = BezPath::new();
    let steps = 20;
    let sweep = std::f64::consts::PI * 1.6;
    for step in 0..=steps {
        let theta = std::f64::consts::PI * 0.6 + sweep * f64::from(step) / f64::from(steps);
        let point = Point::new(
            centre.x + radius * theta.cos(),
            centre.y + radius * theta.sin(),
        );
        if step == 0 {
            path.move_to(point);
        } else {
            path.line_to(point);
        }
    }
    // The arrow head at the start of the sweep.
    let head = Point::new(
        centre.x + radius * (std::f64::consts::PI * 0.6).cos(),
        centre.y + radius * (std::f64::consts::PI * 0.6).sin(),
    );
    path.move_to(Point::new(
        head.x - box_size * 0.12,
        head.y - box_size * 0.02,
    ));
    path.line_to(head);
    path.line_to(Point::new(
        head.x + box_size * 0.02,
        head.y + box_size * 0.12,
    ));
    scene.stroke_path(origin, &path, 1.2, &Brush::Solid(color));
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Affine, CornerRadii, KeyEvent, Modifiers, PointerButton, PointerEvent};
    use frust::text;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rounded: Vec<(Point, Size, Color)>,
        clips: Vec<(Point, Size)>,
        inks: Vec<Color>,
        transforms: Vec<Affine>,
        strokes: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, _r: f64, c: Color) {
            self.rounded.push((o, s, c));
        }
        fn fill_rounded_rect_radii(&mut self, o: Point, s: Size, _r: CornerRadii, c: Color) {
            self.rounded.push((o, s, c));
        }
        fn push_clip(&mut self, o: Point, s: Size) {
            self.clips.push((o, s));
        }
        fn push_clip_rounded(&mut self, o: Point, s: Size, _r: f64) {
            self.clips.push((o, s));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {
            self.strokes += 1;
        }
        fn push_transform(&mut self, t: Affine) {
            self.transforms.push(t);
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    #[derive(Default)]
    struct Acted {
        open: Option<bool>,
        open_calls: u32,
        copies: u32,
        retries: u32,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn view(open: bool, status: ToolResultStatus) -> ToolResultView<Acted> {
        tool_result::<Acted, _>(
            "bash",
            "npm run build",
            text("dist/index.js  12.4 kB\ndist/index.css  3.1 kB"),
        )
        .kind(ToolResultKind::Terminal)
        .status(status)
        .duration(Duration::from_millis(1_820))
        .open(open)
        .on_open_change(|s: &mut Acted, next: bool| {
            s.open = Some(next);
            s.open_calls += 1;
        })
        .on_copy(|s: &mut Acted| s.copies += 1)
        .on_retry(|s: &mut Acted| s.retries += 1)
    }

    fn build(v: &ToolResultView<Acted>) -> ToolResultWidget {
        let mut counter = 0u64;
        View::<Acted>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut ToolResultWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(460.0, 900.0)),
        )
    }

    fn laid_out(open: bool, status: ToolResultStatus) -> (ToolResultWidget, Size) {
        let mut w = build(&view(open, status));
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(
        w: &mut ToolResultWidget,
        size: Size,
        theme: Option<&Theme>,
        ms: f64,
    ) -> (Recorder, bool, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame(), ctx.needs_layout())
    }

    fn rebuild(
        w: &mut ToolResultWidget,
        from: (bool, ToolResultStatus),
        to: (bool, ToolResultStatus),
    ) {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Acted>::rebuild(&view(to.0, to.1), &view(from.0, from.1), w, &mut ctx);
    }

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut ToolResultWidget, size: Size, event: &InputEvent, state: &mut Acted) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    fn press(w: &mut ToolResultWidget, size: Size, at: Point, state: &mut Acted) {
        dispatch(w, size, &pointer(PointerPhase::Down, at), state);
        dispatch(w, size, &pointer(PointerPhase::Up, at), state);
    }

    fn key(named: NamedKey) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    /// The duration formatter's three cases, and the boundaries between them.
    #[test]
    fn durations_read_in_milliseconds_seconds_then_minutes() {
        assert_eq!(format_duration(Duration::from_millis(0)), "0ms");
        assert_eq!(format_duration(Duration::from_millis(999)), "999ms");
        assert_eq!(format_duration(Duration::from_millis(1_000)), "1.0s");
        assert_eq!(format_duration(Duration::from_millis(1_820)), "1.8s");
        assert_eq!(format_duration(Duration::from_secs(59)), "59.0s");
        assert_eq!(format_duration(Duration::from_secs(60)), "1m 00s");
        assert_eq!(format_duration(Duration::from_secs(154)), "2m 34s");
    }

    /// Every status carries its own word, and only `Running` reports itself as
    /// still going.
    #[test]
    fn every_status_carries_its_own_word() {
        assert_eq!(ToolResultStatus::Running.label(), "Running");
        assert_eq!(ToolResultStatus::Success.label(), "Completed");
        assert_eq!(ToolResultStatus::Error.label(), "Failed");
        assert_eq!(ToolResultStatus::Cancelled.label(), "Cancelled");
        assert!(ToolResultStatus::default().is_running());
        assert!(!ToolResultStatus::Success.is_running());
    }

    /// Every status resolves a distinct ink, and the error one is the theme's
    /// error role rather than a hand-picked hue.
    #[test]
    fn every_status_resolves_a_distinct_ink() {
        let theme = crate::theme();
        let inks: Vec<_> = [
            ToolResultStatus::Running,
            ToolResultStatus::Success,
            ToolResultStatus::Error,
            ToolResultStatus::Cancelled,
        ]
        .into_iter()
        .map(|s| ToolResultWidget::status_ink(s, Some(&theme)))
        .collect();
        for (a, ink) in inks.iter().enumerate() {
            for (b, other) in inks.iter().enumerate() {
                assert!(a == b || ink != other, "statuses {a} and {b} share an ink");
            }
        }
        assert_eq!(inks[2], theme.scheme().error);
        assert_eq!(
            ToolResultWidget::status_ink(ToolResultStatus::Success, None),
            BeuiTokens::beui().success,
            "the unthemed fallback is the vendored table"
        );
    }

    /// The whole chrome paints: the kind glyph, title, meta, tool, status word,
    /// status glyph, chevron, panel, payload and the action row.
    #[test]
    fn an_open_result_paints_its_header_panel_payload_and_actions() {
        let (mut w, size) = laid_out(true, ToolResultStatus::Success);
        let (rec, _, _) = paint_at(&mut w, size, None, 0.0);
        assert!(!rec.rounded.is_empty(), "the panel surface is filled");
        assert_eq!(rec.rounded[0].2, code_palette(None).surface);
        assert_eq!(
            rec.clips.len(),
            2,
            "the panel round-clip plus the body clip"
        );
        assert_eq!(rec.transforms.len(), 1, "only the chevron rotates");
        // title, meta, tool, header status word, foot status word, payload.
        assert!(rec.inks.len() >= 6, "painted {} runs", rec.inks.len());
        assert!(rec.strokes >= 4, "kind, status circle, check, chevron");
    }

    /// A closed result is its header alone, and publishes no payload.
    #[test]
    fn a_closed_result_is_just_its_header() {
        let (mut w, size) = laid_out(false, ToolResultStatus::Success);
        let (rec, _, _) = paint_at(&mut w, size, None, 0.0);
        assert!(rec.clips.is_empty(), "no panel, so nothing to clip");
        assert_eq!(layout(&mut w).height, TOOL_RESULT_HEADER_HEIGHT);
        assert!(w.action_rect(ResultTarget::Copy).is_none());
        assert_eq!(w.targets(), vec![ResultTarget::Header]);
    }

    /// Closing morphs the widget's own height and every moving frame asks for
    /// relayout — the layout-skip hazard a lane-driven height carries.
    #[test]
    fn closing_morphs_the_height_and_asks_for_relayout() {
        let (mut w, size) = laid_out(true, ToolResultStatus::Success);
        let open_height = layout(&mut w).height;
        paint_at(&mut w, size, None, 0.0);
        assert!(open_height > TOOL_RESULT_HEADER_HEIGHT);

        rebuild(
            &mut w,
            (true, ToolResultStatus::Success),
            (false, ToolResultStatus::Success),
        );
        let (_, _, needs_layout) = paint_at(&mut w, size, None, 100.0);
        assert!(needs_layout, "a closing disclosure must ask for relayout");

        let mut shortest = open_height;
        for step in 1..=40 {
            paint_at(&mut w, size, None, 100.0 + f64::from(step) * 25.0);
            shortest = shortest.min(layout(&mut w).height);
        }
        assert!(shortest < open_height, "the disclosure never shrank");
        paint_at(&mut w, size, None, 5_000.0);
        assert_eq!(layout(&mut w).height, TOOL_RESULT_HEADER_HEIGHT);
        let (_, _, still) = paint_at(&mut w, size, None, 5_100.0);
        assert!(!still, "a settled disclosure asks for no more layout");
    }

    /// Re-passing the disclosure flag already being flown toward must not
    /// restart the reveal.
    #[test]
    fn a_redundant_rebuild_does_not_restart_the_reveal() {
        let (mut w, size) = laid_out(true, ToolResultStatus::Success);
        paint_at(&mut w, size, None, 0.0);
        rebuild(
            &mut w,
            (true, ToolResultStatus::Success),
            (false, ToolResultStatus::Success),
        );
        paint_at(&mut w, size, None, 100.0);
        paint_at(&mut w, size, None, 160.0);
        let mid = w.reveal.value();
        rebuild(
            &mut w,
            (false, ToolResultStatus::Success),
            (false, ToolResultStatus::Success),
        );
        paint_at(&mut w, size, None, 200.0);
        assert!(
            w.reveal.value() < mid,
            "the clock kept running: {mid} -> {}",
            w.reveal.value()
        );
    }

    /// A status change crossfades the two glyphs and then settles; an unchanged
    /// status starts nothing.
    #[test]
    fn a_status_change_crossfades_its_glyphs_and_settles() {
        let (mut w, size) = laid_out(true, ToolResultStatus::Running);
        paint_at(&mut w, size, None, 0.0);
        assert!(w.previous_status.is_none());

        rebuild(
            &mut w,
            (true, ToolResultStatus::Running),
            (true, ToolResultStatus::Success),
        );
        assert_eq!(w.previous_status, Some(ToolResultStatus::Running));
        let (_, needs_frame, _) = paint_at(&mut w, size, None, 100.0);
        assert!(needs_frame, "a running crossfade owes another frame");
        paint_at(&mut w, size, None, 150.0);
        let mid = w.morph.value();
        assert!(mid > 0.0 && mid < 1.0, "mid-crossfade: {mid}");

        paint_at(&mut w, size, None, 1_000.0);
        assert_eq!(w.morph.value(), 1.0);
        assert!(w.previous_status.is_none(), "the old glyph is dropped");

        rebuild(
            &mut w,
            (true, ToolResultStatus::Success),
            (true, ToolResultStatus::Success),
        );
        assert!(
            w.previous_status.is_none(),
            "an unchanged status starts no crossfade"
        );
    }

    /// `reduce_motion` lands the disclosure and the crossfade on the frame they
    /// are painted, and freezes the running spinner.
    #[test]
    fn reduce_motion_lands_everything_immediately() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let (mut w, size) = laid_out(true, ToolResultStatus::Running);
        paint_at(&mut w, size, Some(&theme), 0.0);
        rebuild(
            &mut w,
            (true, ToolResultStatus::Running),
            (false, ToolResultStatus::Success),
        );
        let (_, _, needs_layout) = paint_at(&mut w, size, Some(&theme), 100.0);
        assert!(needs_layout, "the snap still publishes its new height");
        assert_eq!(w.reveal.value(), 0.0);
        assert_eq!(w.morph.value(), 1.0, "the crossfade landed with it");
        assert!(w.previous_status.is_none());

        // Nothing is owed once the published height has been taken up.
        layout(&mut w);
        let (_, needs_frame, still) = paint_at(&mut w, size, Some(&theme), 120.0);
        assert!(!needs_frame && !still, "a snapped panel asks for nothing");
    }

    /// A running result turns its spinner off a paced tick; a settled, finished
    /// one asks for nothing.
    #[test]
    fn a_running_result_paces_its_spinner_and_a_settled_one_is_quiet() {
        let (mut w, size) = laid_out(true, ToolResultStatus::Running);
        let (_, needs_frame, _) = paint_at(&mut w, size, None, 0.0);
        assert!(needs_frame, "the spinner asks for its next tick");

        let (mut done, size) = laid_out(true, ToolResultStatus::Success);
        paint_at(&mut done, size, None, 0.0);
        let (_, needs_frame, needs_layout) = paint_at(&mut done, size, None, 50.0);
        assert!(!needs_frame && !needs_layout, "a settled result is quiet");
    }

    /// Every trigger reports its decision exactly once, and the widget never
    /// writes its own disclosure.
    #[test]
    fn every_trigger_reports_once_and_writes_nothing() {
        let (mut w, size) = laid_out(true, ToolResultStatus::Error);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Acted::default();

        press(
            &mut w,
            size,
            Point::new(120.0, TOOL_RESULT_HEADER_HEIGHT / 2.0),
            &mut state,
        );
        assert_eq!(state.open, Some(false));
        assert_eq!(state.open_calls, 1);
        assert!(w.open, "the widget never writes its own disclosure");

        let copy = w.action_rect(ResultTarget::Copy).unwrap().center();
        press(&mut w, size, copy, &mut state);
        assert_eq!(state.copies, 1);

        let retry = w.action_rect(ResultTarget::Retry).unwrap().center();
        press(&mut w, size, retry, &mut state);
        assert_eq!(state.retries, 1);
        assert_eq!(state.copies, 1, "the retry press did not also copy");
    }

    /// A press released off its armed target fires nothing — the up-inside
    /// rule.
    #[test]
    fn a_press_released_off_target_reports_nothing() {
        let (mut w, size) = laid_out(true, ToolResultStatus::Error);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Acted::default();
        let copy = w.action_rect(ResultTarget::Copy).unwrap().center();
        dispatch(&mut w, size, &pointer(PointerPhase::Down, copy), &mut state);
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Up, Point::new(copy.x + 200.0, copy.y)),
            &mut state,
        );
        assert_eq!(state.copies, 0);
    }

    /// Keyboard: the arrows rove the header and both actions, and the
    /// activation keys fire the one under the cursor.
    #[test]
    fn the_arrows_rove_every_trigger_and_enter_fires_it() {
        let (mut w, size) = laid_out(true, ToolResultStatus::Error);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Acted::default();
        assert_eq!(w.targets().len(), 3);

        dispatch(&mut w, size, &key(NamedKey::ArrowDown), &mut state);
        assert_eq!(w.focused, ResultTarget::Copy);
        dispatch(&mut w, size, &key(NamedKey::Enter), &mut state);
        assert_eq!(state.copies, 1);
        dispatch(&mut w, size, &key(NamedKey::ArrowDown), &mut state);
        assert_eq!(w.focused, ResultTarget::Retry);
        dispatch(&mut w, size, &key(NamedKey::Enter), &mut state);
        assert_eq!(state.retries, 1);
        dispatch(&mut w, size, &key(NamedKey::ArrowDown), &mut state);
        assert_eq!(w.focused, ResultTarget::Header, "the cursor wraps");
    }

    /// A panel with no actions wired paints no action row, and its panel is
    /// exactly the payload plus its padding.
    #[test]
    fn a_result_with_no_actions_has_no_action_row() {
        let mut w = build(
            &tool_result::<Acted, _>("http", "GET /v1/models", text("200 OK"))
                .kind(ToolResultKind::Request),
        );
        let size = layout(&mut w);
        paint_at(&mut w, size, None, 0.0);
        assert!(!w.has_foot());
        assert_eq!(w.foot_height(), 0.0);
        assert_eq!(
            w.panel_natural(),
            w.payload_height + TOOL_RESULT_PANEL_PADDING * 2.0
        );
        assert_eq!(w.targets(), vec![ResultTarget::Header]);
    }

    /// The cap bounds the panel however tall the payload measures.
    #[test]
    fn the_panel_is_capped_at_its_max_height() {
        let mut w = build(&view(true, ToolResultStatus::Success).max_height(60.0));
        let size = layout(&mut w);
        paint_at(&mut w, size, None, 0.0);
        assert!(w.panel_natural() > 60.0, "the payload really is taller");
        assert_eq!(w.panel_height(), 60.0);
        assert_eq!(
            layout(&mut w).height,
            TOOL_RESULT_HEADER_HEIGHT + TOOL_RESULT_CONTENT_GAP + 60.0
        );
    }

    // ---- Typeface: the title follows the live theme, the rest stays mono -----

    use crate::agents::code_block::mixed_face_probe::{
        assert_mixed_follows_a_live_family_swap, assert_paints_geist_beside_mono,
    };

    const PROBE_WINDOW: Size = Size::new(420.0, 400.0);

    /// The explicit mono runs [`probe_view`] paints: the meta, the tool name
    /// and the status word. With no copy or retry there is no action row, so
    /// the status word does not paint a second time in the panel's foot.
    const PROBE_MONO_RUNS: usize = 3;

    /// An open result over a glyph-free payload, so the title is the one
    /// themed run.
    fn probe_view(_: &mut ()) -> ToolResultView<()> {
        tool_result::<(), _>(
            "bash",
            "npm run build",
            frust::SizedBox::<()>(Some(40.0), Some(20.0)),
        )
        .meta("1.8s")
    }

    #[test]
    fn the_title_paints_in_geist_beside_mono_meta() {
        assert_paints_geist_beside_mono(
            "the tool result",
            probe_view,
            PROBE_WINDOW,
            PROBE_MONO_RUNS,
        );
    }

    #[test]
    fn the_title_follows_a_live_theme_family_swap() {
        assert_mixed_follows_a_live_family_swap(
            "the tool result",
            probe_view,
            PROBE_WINDOW,
            PROBE_MONO_RUNS,
        );
    }
}
