//! Keyboard shortcuts for app code: [`shortcuts`] wraps a child and binds chords to app-state
//! handlers.
//!
//! The wrapper declares every binding on each paint through [`PaintCtx::register_shortcut`]; a
//! key the focus path leaves unhandled that presses one comes back as [`InputEvent::Shortcut`]
//! and runs its handler like a button press. Ids are allocated once, when the wrapper is built;
//! an unmounted wrapper stops declaring at the next paint. The child is held inline, not in a
//! pod, so the wrapper is transparent to layout, hit testing, focus traversal and slivers.
//!
//! A hold binding ([`ShortcutsView::on_hold`]) reports `true` once when its chord goes down and
//! `false` once when the key is released or held keys are cancelled; auto-repeat is answered but
//! does not re-fire.
//!
//! Bindings are declared before the child paints, so a nested wrapper's binding for the same
//! chord in the same tier is the newer declaration and wins (`frust_core::shortcuts`).

use std::rc::Rc;

use frust_core::event::PointerSource;
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Chord, EventCtx, EventResult, InputEvent,
    InspectDetail, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, ShortcutDecl, ShortcutId,
    ShortcutScope, SliverWidget, View, Widget,
};
use kurbo::Size;

use crate::authoring::{
    ErasedArgCallback, ErasedCallback, TypedArgCallback, erase_callback, erase_callback_arg,
};

/// One binding as the view holds it: what to declare, and the typed handler.
struct Binding<State> {
    chord: Chord,
    label: String,
    scope: ShortcutScope,
    handler: Handler<State>,
}

/// How a binding reacts: once per press (and repeat), or as a held-state toggle.
enum Handler<State> {
    Press(Rc<dyn Fn(&mut State)>),
    Hold(TypedArgCallback<State, bool>),
}

/// A child plus the chords it binds. See the [module docs](self).
pub struct ShortcutsView<State: 'static, V> {
    child: V,
    bindings: Vec<Binding<State>>,
}

/// Wrap `child` so it can bind keyboard shortcuts with [`ShortcutsView::on`] (local) and
/// [`ShortcutsView::global`] (app-global).
///
/// ```
/// use frust_widgets::{shortcuts, text};
///
/// struct App(bool);
///
/// let _ = shortcuts(text("Editor"))
///     .global("Mod+K", "Open palette", |app: &mut App| app.0 = true)
///     .on("Escape", "Close palette", |app: &mut App| app.0 = false)
///     .on_hold("Space", "Hold to compare", |app: &mut App, held| app.0 = held);
/// ```
pub fn shortcuts<State: 'static, V: View<State>>(child: V) -> ShortcutsView<State, V> {
    ShortcutsView {
        child,
        bindings: Vec::new(),
    }
}

impl<State: 'static, V> ShortcutsView<State, V> {
    /// Bind `chord` ([`Chord::parse`] syntax, e.g. `"Mod+K"`) in the
    /// [`ShortcutScope::Local`] tier, listed under `label` in shortcut hints.
    ///
    /// # Panics
    ///
    /// When `chord` does not parse; build a [`Chord`] and call [`Self::bind`] for a spec
    /// read at runtime.
    pub fn on(
        self,
        chord: &str,
        label: impl Into<String>,
        handler: impl Fn(&mut State) + 'static,
    ) -> Self {
        self.bind(parse(chord), label, ShortcutScope::Local, handler)
    }

    /// Like [`Self::on`], in the [`ShortcutScope::AppGlobal`] tier: tried after every local
    /// binding.
    ///
    /// # Panics
    ///
    /// When `chord` does not parse.
    pub fn global(
        self,
        chord: &str,
        label: impl Into<String>,
        handler: impl Fn(&mut State) + 'static,
    ) -> Self {
        self.bind(parse(chord), label, ShortcutScope::AppGlobal, handler)
    }

    /// Bind an already-built `chord` in `scope`.
    pub fn bind(
        self,
        chord: Chord,
        label: impl Into<String>,
        scope: ShortcutScope,
        handler: impl Fn(&mut State) + 'static,
    ) -> Self {
        self.push(chord, label, scope, Handler::Press(Rc::new(handler)))
    }

    /// Bind `chord` in the [`ShortcutScope::Local`] tier as a hold: `handler` gets `true` once
    /// when the chord goes down and `false` exactly once when it ends.
    ///
    /// - `false` also arrives when the window loses focus (the root cancels held keys).
    /// - Auto-repeat does not re-fire the handler; the repeat is still answered, so it falls to
    ///   no lower tier.
    /// - The chord needs a non-modifier key.
    /// - Release fires on the key's release even if a modifier was let go first.
    /// - A wrapper that unmounts while held gets no `false`: keep the wrapper mounted above
    ///   whatever the hold toggles.
    /// - A hold binding removed, or turned into a press binding, while held drops its flag
    ///   silently.
    ///
    /// # Panics
    ///
    /// When `chord` does not parse; use [`Self::bind_hold`] for a spec read at runtime.
    pub fn on_hold(
        self,
        chord: &str,
        label: impl Into<String>,
        handler: impl Fn(&mut State, bool) + 'static,
    ) -> Self {
        self.bind_hold(parse(chord), label, ShortcutScope::Local, handler)
    }

    /// Like [`Self::on_hold`], in the [`ShortcutScope::AppGlobal`] tier.
    ///
    /// # Panics
    ///
    /// When `chord` does not parse.
    pub fn global_hold(
        self,
        chord: &str,
        label: impl Into<String>,
        handler: impl Fn(&mut State, bool) + 'static,
    ) -> Self {
        self.bind_hold(parse(chord), label, ShortcutScope::AppGlobal, handler)
    }

    /// Bind an already-built `chord` in `scope` as a hold; see [`Self::on_hold`].
    pub fn bind_hold(
        self,
        chord: Chord,
        label: impl Into<String>,
        scope: ShortcutScope,
        handler: impl Fn(&mut State, bool) + 'static,
    ) -> Self {
        self.push(chord, label, scope, Handler::Hold(Rc::new(handler)))
    }

    fn push(
        mut self,
        chord: Chord,
        label: impl Into<String>,
        scope: ShortcutScope,
        handler: Handler<State>,
    ) -> Self {
        self.bindings.push(Binding {
            chord,
            label: label.into(),
            scope,
            handler,
        });
        self
    }
}

fn parse(spec: &str) -> Chord {
    Chord::parse(spec).unwrap_or_else(|err| panic!("invalid shortcut chord `{spec}`: {err}"))
}

/// One binding as the widget keeps it: its declaration (stable id) and erased handler.
struct Entry {
    decl: ShortcutDecl,
    handler: EntryHandler,
}

/// The erased handler; a hold also tracks whether its chord is down.
enum EntryHandler {
    Press(ErasedCallback),
    Hold {
        callback: ErasedArgCallback<bool>,
        held: bool,
    },
}

impl EntryHandler {
    fn is_held(&self) -> bool {
        matches!(self, Self::Hold { held: true, .. })
    }
}

/// The retained widget for a [`ShortcutsView`]: the child's widget plus its bindings.
pub struct ShortcutsWidget<W> {
    child: W,
    entries: Vec<Entry>,
}

impl<State: 'static> Binding<State> {
    /// An entry for this binding; a hold starts with `held` as given.
    fn entry(&self, id: ShortcutId, held: bool) -> Entry {
        Entry {
            decl: ShortcutDecl {
                id,
                chord: self.chord.clone(),
                label: self.label.clone(),
                scope: self.scope,
            },
            handler: match &self.handler {
                Handler::Press(callback) => EntryHandler::Press(erase_callback(callback)),
                Handler::Hold(callback) => EntryHandler::Hold {
                    callback: erase_callback_arg(callback),
                    held,
                },
            },
        }
    }
}

impl<State: 'static, V: View<State>> View<State> for ShortcutsView<State, V> {
    type Element = ShortcutsWidget<V::Element>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        let entries = self.bindings.iter();
        ShortcutsWidget {
            child: self.child.build(ctx),
            entries: entries
                .map(|b| b.entry(ShortcutId::next(), false))
                .collect(),
        }
    }

    /// Bindings keep their ids by position; a changed declaration only needs a repaint.
    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = self.child.rebuild(&prev.child, &mut element.child, ctx);
        let before: Vec<ShortcutDecl> = element.entries.iter().map(|e| e.decl.clone()).collect();
        element.entries.truncate(self.bindings.len());
        for (i, binding) in self.bindings.iter().enumerate() {
            match element.entries.get_mut(i) {
                Some(entry) => {
                    // The flag belongs to the binding, not the slot: keep it only when the
                    // same chord and scope stays a hold here.
                    let held = entry.handler.is_held()
                        && matches!(binding.handler, Handler::Hold(_))
                        && entry.decl.chord == binding.chord
                        && entry.decl.scope == binding.scope;
                    *entry = binding.entry(entry.decl.id, held);
                }
                None => element
                    .entries
                    .push(binding.entry(ShortcutId::next(), false)),
            }
        }
        if element.entries.iter().map(|e| &e.decl).ne(before.iter()) {
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        self.child.teardown(&mut element.child, ctx);
    }

    fn message(&self, element: &mut Self::Element, state: &mut State) {
        self.child.message(&mut element.child, state);
    }
}

impl<W: Widget> Widget for ShortcutsWidget<W> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.child.layout(ctx, bc)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for entry in &self.entries {
            ctx.register_shortcut(entry.decl.clone());
        }
        self.child.paint(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Shortcut(id) => {
                if let Some(entry) = self.entries.iter_mut().find(|e| e.decl.id == *id) {
                    match &mut entry.handler {
                        EntryHandler::Press(callback) => callback(ctx),
                        EntryHandler::Hold { callback, held } => {
                            if !*held {
                                *held = true;
                                callback(ctx, true);
                            }
                        }
                    }
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
            }
            InputEvent::ShortcutReleased(id) => {
                if let Some(entry) = self.entries.iter_mut().find(|e| e.decl.id == *id)
                    && let EntryHandler::Hold { callback, held } = &mut entry.handler
                {
                    if *held {
                        *held = false;
                        callback(ctx, false);
                        ctx.request_redraw();
                    }
                    return EventResult::Ignored;
                }
            }
            _ => {}
        }
        self.child.event(ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics(ctx);
    }

    /// The child's name: the wrapper adds bindings, never a tree element of its own.
    fn type_name(&self) -> &'static str {
        self.child.type_name()
    }

    /// The child's layout report, so a wrapped text stays visible.
    fn inspect_detail(&self) -> Option<InspectDetail> {
        self.child.inspect_detail()
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        self.child.visit_children(visitor);
    }

    fn is_focusable(&self) -> bool {
        self.child.is_focusable()
    }

    fn hit_test_self(&self) -> bool {
        self.child.hit_test_self()
    }

    fn visit_focus_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        self.child.visit_focus_children(visitor);
    }

    fn as_sliver(&mut self) -> Option<&mut dyn SliverWidget> {
        self.child.as_sliver()
    }
}

/// Whether the widget holds focus and `event` is an activation key (see [`KeyEvent::is_activation`]).
pub(crate) fn activation(ctx: &EventCtx, event: &InputEvent) -> bool {
    ctx.has_focus() && matches!(event, InputEvent::Key(key) if key.is_activation())
}

/// Whether a primary press may claim focus: only a mouse contact does. A touch tap activates
/// the control but leaves focus alone, so a focused text field keeps its session and keyboard.
pub(crate) fn press_claims_focus(ctx: &EventCtx) -> bool {
    ctx.pointer_id().source == PointerSource::Mouse
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::cell::Cell;

    use frust_core::{
        DiscardScene, FrameTime, Key, KeyEvent, Modifiers, NamedKey, RenderRoot, ShortcutHint,
    };
    use frust_text::TextContext;

    use super::*;
    use crate::{FlexView, IndexedStack, button, column, either, text};

    #[derive(Default)]
    struct App {
        opened: u32,
        closed: u32,
        holds: Vec<bool>,
    }

    fn key(key: Key, modifiers: Modifiers) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key,
            modifiers,
            repeat: false,
        })
    }

    fn key_up(c: &str) -> InputEvent {
        let InputEvent::Key(event) = ctrl(c) else {
            unreachable!()
        };
        InputEvent::KeyUp(event)
    }

    fn repeat(c: &str) -> InputEvent {
        let InputEvent::Key(event) = ctrl(c) else {
            unreachable!()
        };
        InputEvent::Key(KeyEvent {
            repeat: true,
            ..event
        })
    }

    fn holding() -> ShortcutsView<App, crate::TextView> {
        shortcuts(text("body")).on_hold("Ctrl+K", "Compare", |app: &mut App, held| {
            app.holds.push(held);
        })
    }

    fn ctrl(c: &str) -> InputEvent {
        let modifiers = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        key(Key::Character(c.to_string()), modifiers)
    }

    fn frame<V: View<App>>(root: &mut RenderRoot<App, V>, logic: &mut impl FnMut(&mut App) -> V) {
        let mut state = App::default();
        root.rebuild(logic, &mut state);
        root.layout_with_text(
            Size::new(300.0, 300.0),
            &mut TextContext::new() as &mut dyn Any,
        );
        root.paint(&mut DiscardScene, FrameTime::ZERO);
    }

    fn labels(root: &RenderRoot<App, impl View<App>>) -> Vec<String> {
        root.shortcut_hints(|_| true)
            .into_iter()
            .map(|hint| hint.label)
            .collect()
    }

    fn bound() -> ShortcutsView<App, crate::TextView> {
        shortcuts(text("body"))
            .on("Ctrl+K", "Open", |app: &mut App| app.opened += 1)
            .global("Ctrl+J", "Close", |app: &mut App| app.closed += 1)
    }

    #[test]
    fn a_shortcut_delivery_runs_only_its_own_handler() {
        let mut counter = 0u64;
        let view = bound();
        let mut widget = view.build(&mut BuildCtx::new(&mut counter));
        let (open, close) = (widget.entries[0].decl.id, widget.entries[1].decl.id);
        let mut app = App::default();
        let deliver = |widget: &mut ShortcutsWidget<_>, app: &mut App, id| {
            let state: &mut dyn Any = app;
            let mut ctx = EventCtx::new(state, kurbo::Point::ZERO, Size::new(10.0, 10.0));
            widget.event(&mut ctx, &InputEvent::Shortcut(id))
        };
        assert_eq!(deliver(&mut widget, &mut app, open), EventResult::Handled);
        assert_eq!(deliver(&mut widget, &mut app, close), EventResult::Handled);
        assert_eq!((app.opened, app.closed), (1, 1));
        let foreign = ShortcutId::next();
        assert_eq!(
            deliver(&mut widget, &mut app, foreign),
            EventResult::Ignored
        );
        assert_eq!((app.opened, app.closed), (1, 1), "another id is not ours");
    }

    #[test]
    fn a_pressed_chord_fires_its_handler_and_hints_list_the_bindings() {
        let mut root: RenderRoot<App, FlexView<App>> = RenderRoot::new();
        let mut logic = |_: &mut App| column().child(bound());
        frame(&mut root, &mut logic);
        let hints = root.shortcut_hints(|_| true);
        assert_eq!(
            hints[0],
            ShortcutHint {
                chord: Chord::parse("Ctrl+K").unwrap(),
                label: "Open".to_string(),
                scope: ShortcutScope::Local,
            }
        );
        assert_eq!(hints[1].scope, ShortcutScope::AppGlobal);
        assert_eq!(
            hints[1].chord.display(),
            Chord::parse("Ctrl+J").unwrap().display()
        );

        let mut app = App::default();
        assert!(root.event(&mut app, &ctrl("k")).handled);
        assert!(root.event(&mut app, &ctrl("j")).handled);
        assert!(
            !root.event(&mut app, &ctrl("x")).handled,
            "an unbound chord"
        );
        assert_eq!((app.opened, app.closed), (1, 1));
    }

    #[test]
    fn an_unmounted_wrapper_declares_nothing() {
        let shown = Cell::new(true);
        let mut root = RenderRoot::new();
        let mut logic = |_: &mut App| either(shown.get(), bound, || text("gone"));
        frame(&mut root, &mut logic);
        assert_eq!(labels(&root), ["Open", "Close"]);

        shown.set(false);
        frame(&mut root, &mut logic);
        assert!(labels(&root).is_empty(), "nothing declared after unmount");
        let mut app = App::default();
        root.event(&mut app, &ctrl("k"));
        assert_eq!(app.opened, 0);
    }

    #[test]
    fn a_nested_binding_wins_its_tier_and_rebuilds_keep_ids() {
        let mut root: RenderRoot<App, ShortcutsView<App, ShortcutsView<App, crate::TextView>>> =
            RenderRoot::new();
        let inner_label = Cell::new("Inner");
        let mut logic = |_: &mut App| {
            let inner =
                shortcuts(text("x")).on("Ctrl+K", inner_label.get(), |a: &mut App| a.closed += 1);
            shortcuts(inner).on("Ctrl+K", "Outer", |a: &mut App| a.opened += 1)
        };
        frame(&mut root, &mut logic);
        let mut app = App::default();
        root.event(&mut app, &ctrl("k"));
        assert_eq!(
            (app.opened, app.closed),
            (0, 1),
            "the inner declaration is newer"
        );

        let id = |root: &RenderRoot<App, _>| {
            let pod = root.tree().pod(root.root_id().unwrap()).unwrap();
            let widget = (pod.widget() as &dyn Any)
                .downcast_ref::<ShortcutsWidget<ShortcutsWidget<crate::TextWidget>>>();
            widget.unwrap().child.entries[0].decl.id
        };
        let before = id(&root);
        inner_label.set("Renamed");
        frame(&mut root, &mut logic);
        assert_eq!(id(&root), before, "a rebuild keeps the binding's id");
        assert_eq!(labels(&root), ["Outer", "Renamed"]);
    }

    #[test]
    fn rebuild_reports_paint_only_when_a_declaration_changes() {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        let prev = bound();
        let mut widget = prev.build(&mut ctx);
        let same = bound();
        assert_eq!(
            same.rebuild(&prev, &mut widget, &mut ctx),
            ChangeFlags::NONE
        );
        let more = bound().on("Ctrl+L", "Third", |_: &mut App| {});
        assert!(more.rebuild(&same, &mut widget, &mut ctx).needs_paint());
        assert_eq!(widget.entries.len(), 3);
        let fewer = shortcuts(text("body")).on("Ctrl+K", "Open", |_: &mut App| {});
        assert!(fewer.rebuild(&more, &mut widget, &mut ctx).needs_paint());
        assert_eq!(widget.entries.len(), 1);
    }

    #[test]
    fn focus_queries_forward_to_the_child() {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        let wrapped = shortcuts(button::<App, _>("Go", |_| {})).on("Ctrl+K", "k", |_| {});
        assert!(wrapped.build(&mut ctx).is_focusable());
        assert!(!bound().build(&mut ctx).is_focusable());
        let stack = IndexedStack(Some(0), (text("a"), text("b")));
        let widget = shortcuts::<App, _>(stack).build(&mut ctx);
        let (mut all, mut focus) = (0, 0);
        widget.visit_children(&mut |_| all += 1);
        widget.visit_focus_children(&mut |_| focus += 1);
        assert_eq!(
            (all, focus),
            (2, 1),
            "the stack's narrowing survives the wrapper"
        );
    }

    #[test]
    fn tab_reaches_a_wrapped_control_and_its_keys_pass_through() {
        let mut root: RenderRoot<App, FlexView<App>> = RenderRoot::new();
        let mut logic = |_: &mut App| {
            let go = button::<App, _>("Go", |app: &mut App| app.opened += 1);
            column().child(shortcuts(go).on("Ctrl+K", "k", |app: &mut App| app.closed += 1))
        };
        frame(&mut root, &mut logic);
        let mut app = App::default();
        root.event(
            &mut app,
            &key(Key::Named(NamedKey::Tab), Modifiers::default()),
        );
        assert!(
            root.is_focus_active(),
            "the wrapper's pod is the button's stop"
        );
        root.event(
            &mut app,
            &key(Key::Character(" ".into()), Modifiers::default()),
        );
        root.event(&mut app, &ctrl("k"));
        assert_eq!((app.opened, app.closed), (1, 1));
    }

    #[test]
    #[should_panic(expected = "invalid shortcut chord `F25`")]
    fn an_unparsable_chord_panics_naming_it() {
        let _ = shortcuts::<App, _>(text("x")).on("F25", "Rename", |_| {});
    }

    #[test]
    fn a_hold_binding_reports_true_on_press_and_false_on_key_up() {
        let mut root: RenderRoot<App, ShortcutsView<App, crate::TextView>> = RenderRoot::new();
        let mut logic = |_: &mut App| holding();
        frame(&mut root, &mut logic);
        let mut app = App::default();
        assert!(root.event(&mut app, &ctrl("k")).handled);
        assert_eq!(app.holds, [true]);
        root.event(&mut app, &key_up("k"));
        assert_eq!(app.holds, [true, false]);
        root.event(&mut app, &key_up("k"));
        assert_eq!(app.holds, [true, false], "a second release is dropped");
    }

    #[test]
    fn a_hold_binding_ignores_auto_repeat_but_answers_it() {
        let mut root: RenderRoot<App, ShortcutsView<App, crate::TextView>> = RenderRoot::new();
        let mut logic = |_: &mut App| holding();
        frame(&mut root, &mut logic);
        let mut app = App::default();
        root.event(&mut app, &ctrl("k"));
        let repeated = root.event(&mut app, &repeat("k"));
        assert!(repeated.handled, "the repeat is answered");
        root.event(&mut app, &repeat("k"));
        assert_eq!(app.holds, [true], "the handler ran once");
    }

    #[test]
    fn cancel_held_keys_reports_false_once() {
        let mut root: RenderRoot<App, ShortcutsView<App, crate::TextView>> = RenderRoot::new();
        let mut logic = |_: &mut App| holding();
        frame(&mut root, &mut logic);
        let mut app = App::default();
        root.event(&mut app, &ctrl("k"));
        root.cancel_held_keys(&mut app);
        root.cancel_held_keys(&mut app);
        root.event(&mut app, &key_up("k"));
        assert_eq!(app.holds, [true, false]);
    }

    #[test]
    fn a_hold_binding_survives_a_rebuild_while_held() {
        let mut root: RenderRoot<App, ShortcutsView<App, crate::TextView>> = RenderRoot::new();
        let mut logic = |_: &mut App| holding();
        frame(&mut root, &mut logic);
        let mut app = App::default();
        root.event(&mut app, &ctrl("k"));
        frame(&mut root, &mut logic);
        root.event(&mut app, &repeat("k"));
        assert_eq!(app.holds, [true], "the held flag survived the rebuild");
        root.event(&mut app, &key_up("k"));
        assert_eq!(app.holds, [true, false]);
    }

    #[test]
    fn a_hold_binding_that_moves_into_a_held_slot_does_not_inherit_the_flag() {
        let mut root: RenderRoot<App, ShortcutsView<App, crate::TextView>> = RenderRoot::new();
        let both = std::cell::Cell::new(true);
        let mut logic = |_: &mut App| {
            let view = shortcuts(text("body"));
            let view = if both.get() {
                view.on_hold("Ctrl+K", "A", |app: &mut App, held| {
                    app.opened += u32::from(held);
                    app.closed += u32::from(!held);
                })
            } else {
                view
            };
            view.on_hold("Ctrl+J", "B", |app: &mut App, held| app.holds.push(held))
        };
        frame(&mut root, &mut logic);
        let mut app = App::default();
        root.event(&mut app, &ctrl("k"));
        assert_eq!(app.opened, 1, "A is held");
        both.set(false);
        frame(&mut root, &mut logic);
        // A's release is delivered by A's recorded id, which B now holds by position; B was
        // never pressed, so it gets nothing.
        root.event(&mut app, &key_up("k"));
        assert!(app.holds.is_empty(), "A's release never reaches B");
        assert_eq!(app.closed, 0);
        root.event(&mut app, &ctrl("j"));
        assert_eq!(app.holds, [true], "B reports its own press");
    }

    #[test]
    fn a_press_binding_ignores_a_release_and_still_refires_on_repeat() {
        let mut root: RenderRoot<App, ShortcutsView<App, crate::TextView>> = RenderRoot::new();
        let mut logic = |_: &mut App| {
            shortcuts(text("body")).on("Ctrl+K", "Open", |app: &mut App| app.opened += 1)
        };
        frame(&mut root, &mut logic);
        let mut app = App::default();
        root.event(&mut app, &ctrl("k"));
        root.event(&mut app, &repeat("k"));
        root.event(&mut app, &key_up("k"));
        assert_eq!(
            app.opened, 2,
            "press and repeat each fire; release does not"
        );
    }

    #[test]
    fn a_release_for_another_id_is_forwarded_and_ignored() {
        let mut counter = 0u64;
        let mut widget = holding().build(&mut BuildCtx::new(&mut counter));
        let mut app = App::default();
        let mut deliver = |widget: &mut ShortcutsWidget<_>, event: InputEvent| {
            let state: &mut dyn Any = &mut app;
            let mut ctx = EventCtx::new(state, kurbo::Point::ZERO, Size::new(10.0, 10.0));
            widget.event(&mut ctx, &event)
        };
        let own = widget.entries[0].decl.id;
        let foreign = ShortcutId::next();
        let released = InputEvent::ShortcutReleased(foreign);
        assert_eq!(deliver(&mut widget, released), EventResult::Ignored);
        let released = InputEvent::ShortcutReleased(own);
        assert_eq!(deliver(&mut widget, released), EventResult::Ignored);
        assert!(app.holds.is_empty(), "no release without a press");
    }

    #[test]
    fn a_hold_inside_an_indexed_stack_is_released_after_its_tab_is_hidden() {
        let tab = Cell::new(0usize);
        let mut root = RenderRoot::new();
        let mut logic = |_: &mut App| IndexedStack(Some(tab.get()), (holding(), text("other")));
        frame(&mut root, &mut logic);
        let mut app = App::default();
        root.event(&mut app, &ctrl("k"));
        tab.set(1);
        frame(&mut root, &mut logic);
        root.event(&mut app, &key_up("k"));
        assert_eq!(app.holds, [true, false]);
    }

    #[test]
    #[should_panic(expected = "invalid shortcut chord `F25`")]
    fn on_hold_panics_on_a_bad_chord() {
        let _ = shortcuts::<App, _>(text("x")).on_hold("F25", "Rename", |_, _| {});
    }
}
