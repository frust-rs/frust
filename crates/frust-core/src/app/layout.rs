//! `RenderRoot`'s layout pass: lays the root out against the window and runs
//! the optional layout audit.

use std::any::Any;
use std::collections::HashMap;

use kurbo::{Point, Size};

use super::RenderRoot;
use crate::layout::BoxConstraints;
use crate::view::{View, WidgetId};
use crate::widget::LayoutCtx;

/// Layout-audit bookkeeping ([`RenderRoot::set_layout_audit`]): an audited
/// layout runs the skipping pass, then a forced full pass over the same inputs;
/// each pod whose geometry differs bumps `mismatches` and logs a `layout-audit:`
/// warning (once per 60 audited frames per widget type and label).
#[derive(Default)]
pub(super) struct LayoutAudit {
    pub(super) enabled: bool,
    pub(super) mismatches: u64,
    /// Audited frames so far.
    frame: u64,
    /// Audited frame each `(type, label)` was last logged on.
    last_logged: HashMap<(&'static str, Option<String>), u64>,
}

/// One node's geometry in a layout-audit snapshot.
struct AuditGeom {
    id: WidgetId,
    type_name: &'static str,
    label: Option<String>,
    origin: Point,
    size: Size,
}

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// Lay out the root widget against `window_size` and record its geometry.
    ///
    /// The root gets loose constraints and sits at the origin; returns its size.
    /// No text context is threaded (use [`RenderRoot::layout_with_text`]).
    pub fn layout(&mut self, window_size: Size) -> Size {
        self.layout_inner(window_size, None)
    }

    /// Lay out the root widget, threading a shared text-shaping context down to
    /// text widgets.
    ///
    /// `text_ctx` is the shell-owned `frust_text::TextContext`, type-erased so
    /// this crate needs no `frust-text` dependency; widgets recover it via
    /// [`crate::widget::LayoutCtx::text_context`].
    pub fn layout_with_text(&mut self, window_size: Size, text_ctx: &mut dyn Any) -> Size {
        self.layout_inner(window_size, Some(text_ctx))
    }

    /// Shared body of [`RenderRoot::layout`] and [`RenderRoot::layout_with_text`].
    fn layout_inner(&mut self, window_size: Size, text_ctx: Option<&mut dyn Any>) -> Size {
        // The window size is a global layout input (overlay pods lay out
        // against it), so a change invalidates every cached layout.
        if self.window_size != window_size {
            self.bump_layout_epoch();
        }
        self.window_size = window_size;
        // A floated surface dirtied outside its owner's pod chain (see
        // `overlay_pods`) is reachable only through a full pass.
        if self.overlay_pod_dirty() {
            self.bump_layout_epoch();
        }
        if self.root_id.is_none() {
            return Size::ZERO;
        }
        if !(self.layout_audit.enabled && self.layout_skip) {
            return self.layout_pass(window_size, text_ctx, !self.layout_skip);
        }
        // Audit: the forced pass runs last, so its geometry is what stays.
        let mut text_ctx = text_ctx;
        self.layout_pass(window_size, text_ctx.as_deref_mut(), false);
        let before = self.audit_snapshot();
        let size = self.layout_pass(window_size, text_ctx, true);
        let after = self.audit_snapshot();
        self.audit_compare(&before, &after);
        size
    }

    /// Every node's `(id, type, label, origin relative to parent, size)`.
    fn audit_snapshot(&self) -> Vec<AuditGeom> {
        let nodes = self.tree.inspect();
        let origins: HashMap<WidgetId, Point> =
            nodes.iter().map(|n| (n.id, n.bounds.origin())).collect();
        nodes
            .into_iter()
            .map(|n| {
                let parent = n
                    .parent
                    .and_then(|p| origins.get(&p).copied())
                    .unwrap_or(Point::ZERO);
                AuditGeom {
                    id: n.id,
                    type_name: n.type_name,
                    label: n.debug_label.map(|l| l.into_owned()),
                    origin: Point::new(n.bounds.x0 - parent.x, n.bounds.y0 - parent.y),
                    size: n.bounds.size(),
                }
            })
            .collect()
    }

    /// Report every pod that differs between the two snapshots.
    fn audit_compare(&mut self, before: &[AuditGeom], after: &[AuditGeom]) {
        const TOLERANCE: f64 = 1e-3;
        let frame = self.layout_audit.frame;
        self.layout_audit.frame += 1;
        let by_id: HashMap<WidgetId, &AuditGeom> = before.iter().map(|g| (g.id, g)).collect();
        for now in after {
            let Some(was) = by_id.get(&now.id) else {
                continue;
            };
            let differs = (was.origin.x - now.origin.x).abs() > TOLERANCE
                || (was.origin.y - now.origin.y).abs() > TOLERANCE
                || (was.size.width - now.size.width).abs() > TOLERANCE
                || (was.size.height - now.size.height).abs() > TOLERANCE;
            if !differs {
                continue;
            }
            self.layout_audit.mismatches += 1;
            let key = (now.type_name, now.label.clone());
            let due = self
                .layout_audit
                .last_logged
                .get(&key)
                .is_none_or(|last| frame.saturating_sub(*last) >= 60);
            if due {
                self.layout_audit.last_logged.insert(key, frame);
                log::warn!(
                    "layout-audit: {} (label {:?}) changed geometry without signalling layout: skipped pass origin {:?} size {:?}, forced pass origin {:?} size {:?}",
                    now.type_name,
                    now.label,
                    was.origin,
                    was.size,
                    now.origin,
                    now.size,
                );
            }
        }
    }

    /// One layout pass over the root; `force` lays out every pod regardless of
    /// the skip rule.
    fn layout_pass(
        &mut self,
        window_size: Size,
        text_ctx: Option<&mut dyn Any>,
        force: bool,
    ) -> Size {
        let Some(root_id) = self.root_id else {
            return Size::ZERO;
        };
        let bc = BoxConstraints::loose(window_size);
        let theme = self.theme.as_deref();
        let insets = self.insets;
        let layout_epoch = self.layout_epoch;
        let Some(pod) = self.tree.pod_mut(root_id) else {
            return Size::ZERO;
        };
        let mut ctx = LayoutCtx::with_resources(text_ctx, theme);
        // One context reaches the whole tree, so the global inputs (insets,
        // window size, layout epoch, force switch) are set once here. The root
        // sits in no `ChildPod`, so it is always laid out.
        ctx.set_window_insets(insets);
        ctx.set_window_size(window_size);
        ctx.set_layout_epoch(layout_epoch);
        ctx.set_layout_force(force);
        let size = pod.widget_mut().layout(&mut ctx, &bc);
        pod.set_layout(Point::ZERO, size);
        size
    }
}
