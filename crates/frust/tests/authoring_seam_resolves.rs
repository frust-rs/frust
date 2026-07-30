//! Compile-only proof that `frust::authoring` (and its `text`/`scene`
//! submodules) resolve with `frust` as the test target's only dependency —
//! the machine-checked half of the seam's sufficiency claim alongside the
//! `authoring` module's own doctest (`cargo test -p frust --doc`).

use frust::authoring::scene::SceneBuilder;
use frust::authoring::text::TextContext;
use frust::authoring::*;

#[test]
fn seam_items_resolve() {
    // Naming the types is the assertion; this must compile with `frust` as the
    // only dependency of this test target.
    fn _sig(_: &mut LayoutCtx<'_>, _: BoxConstraints) -> Size {
        Size::ZERO
    }
    let _ = std::mem::size_of::<ChildPod>();
    let _ = std::mem::size_of::<TextContext>();
    let _ = std::mem::size_of::<SceneBuilder>();
}

/// A widget that *contributes* an accessibility node, not merely forwards a
/// child's.
///
/// Regression guard for a real gap: `SemanticsCtx::push_node` is
/// `(role: Role, build: impl FnOnce(&mut Node)) -> NodeId`, all three
/// `accesskit` types. Until they were lifted into `authoring` (and
/// `frust::accesskit` added as the whole-crate valve) this was impossible to
/// write without a direct `frust-core`/`accesskit` dependency — and every
/// in-repo consumer only *forwards* semantics, so no migration could have
/// caught it. Keep this compiling.
struct SemanticsContributingWidget;

impl Widget for SemanticsContributingWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(10.0, 10.0))
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let _id: NodeId = ctx.push_node(Role::Button, |node: &mut Node| {
            node.set_label("press me");
        });
    }
}

#[test]
fn a_widget_can_contribute_semantics_through_the_facade_alone() {
    let _ = std::mem::size_of::<SemanticsContributingWidget>();
}

/// The `TextContext` accessors' return types must themselves be nameable —
/// `register_fonts() -> Result<Vec<RegisteredFamily>, FontError>` and
/// `shape_cache_stats() -> ShapeCacheStats` — as must the UTF-16/byte index
/// bridge between the two `EditingState` types the seam exposes.
#[test]
fn text_surface_is_closed_over_its_own_signatures() {
    fn _register(ctx: &mut frust::authoring::text::TextContext, data: Vec<u8>) {
        let _: Result<
            Vec<frust::authoring::text::RegisteredFamily>,
            frust::authoring::text::FontError,
        > = ctx.register_fonts(data);
        let _: frust::authoring::text::ShapeCacheStats = ctx.shape_cache_stats();
    }
    let _bridge: fn(&str, usize) -> usize = frust::authoring::text::utf16_to_byte;
    let _back: fn(&str, usize) -> usize = frust::authoring::text::byte_to_utf16;
}

/// `LayoutCtx::window_insets()`/`PaintCtx::window_insets()` return
/// `WindowInsets`; a widget laying itself out around the notch or keyboard
/// must be able to name it.
#[test]
fn inset_vocabulary_resolves() {
    fn _read(ctx: &LayoutCtx<'_>) -> WindowInsets {
        ctx.window_insets()
    }
    let _ = std::mem::size_of::<WindowEdgeInsets>();
}
