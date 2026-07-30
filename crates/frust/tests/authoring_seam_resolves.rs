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
