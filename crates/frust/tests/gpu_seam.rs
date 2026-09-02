//! Compile-only and headless-GPU proofs for the `frust::gpu` facade seam
//! (see `crates/frust/src/lib.rs`'s `gpu` module): an app — or a future
//! 3D-rendering crate sitting beside the facade — registers an externally
//! owned GPU texture and draws it through
//! `authoring::scene::SceneBuilder::scene_texture`, reaching the seam
//! through `frust::gpu`'s re-exported types alone.
//!
//! Gated behind the `gpu` feature this crate carries — off by default, so a
//! default `cargo test -p frust` run compiles this file to nothing, the same
//! shape `frust-shell-common/tests/devtools_loopback.rs`'s
//! `#![cfg(feature = "devtools")]` already uses for a feature-gated
//! integration test.
//!
//! Neither test here constructs a real `frust::gpu::Texture`: building one
//! needs a `TextureDesc { format, usage, .. }` naming `wgpu::TextureFormat`/
//! `wgpu::TextureUsages` directly (this crate re-exports the GPU-substrate
//! *types* built on `wgpu`, not `wgpu` itself), and this crate must not add
//! a new external dependency just to exercise its own re-export surface. A
//! real consumer of the `gpu` feature depends on `wgpu` in its own right to
//! fill those two fields in, exactly as any other code driving a render pass
//! beside the engine does.

#![cfg(feature = "gpu")]

use frust::authoring::Rect;
use frust::authoring::scene::{Command, Scene, SceneBuilder};
use frust::gpu::{Context, Texture};

/// Checked at compile time over any texture/view pair a caller supplies:
/// `Texture::as_scene_texture()`'s minted id composes with
/// `SceneBuilder::scene_texture` with no live `Texture` needed to prove it —
/// the generic body type-checks regardless of what `T`/`V` a caller
/// eventually instantiates it with.
#[allow(dead_code)]
fn draw_registered_texture<T, V>(texture: &Texture<T, V>, scene: &mut Scene, dest: Rect) {
    let mut builder = SceneBuilder::new(scene);
    builder.scene_texture(texture.as_scene_texture().get(), dest);
}

/// Records a `Command::SceneTexture` through `SceneBuilder::scene_texture`,
/// using the same opaque `u64` id a minted `SceneTextureId::get()`
/// ultimately is (see [`draw_registered_texture`] for the compile-time proof
/// that a real minted id composes identically) — the seam's draw-recording
/// half, which needs no GPU device in the loop.
fn record_scene_texture_draw() {
    let id = 42_u64;
    let dest = Rect::new(0.0, 0.0, 64.0, 64.0);

    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    builder.scene_texture(id, dest);

    match &scene.commands()[0] {
        Command::SceneTexture {
            id: recorded,
            dest: recorded_dest,
            ..
        } => {
            assert_eq!(*recorded, id);
            assert_eq!(*recorded_dest, dest);
        }
        other => panic!("expected exactly one SceneTexture command, got {other:?}"),
    }
}

#[test]
fn scene_texture_seam_compiles_and_records_through_facade_types_only() {
    record_scene_texture_draw();
}

/// A minimal, dependency-free `block_on`: this crate carries no async
/// executor of its own to reach for (`pollster` is a shell-only dependency,
/// and adding one here just for this test's one-shot device-creation future
/// would be a new dependency this seam does not otherwise need).
/// `Waker::noop()` (stable) is sufficient — the polled future never
/// registers real wake-up interest, it simply resolves on a later poll.
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    loop {
        match fut.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(output) => return output,
            std::task::Poll::Pending => std::thread::yield_now(),
        }
    }
}

/// Brings up a real headless GPU device through `frust::gpu::Context` alone
/// (`frust-gpu`'s `RenderContext` under the facade's name), verifies the
/// resolved adapter against `FRUST_GOLDEN_EXPECT_ADAPTER` when the operator
/// set one — the same expectation-before-any-work contract
/// `frust_render::HeadlessRenderer` uses for its own `--ignored` GPU tests,
/// reimplemented inline here since that type sits outside this seam's
/// facade-only re-export list — then proves the draw-recording half of the
/// seam.
///
/// This crate does not depend on `frust-engine`, and `frust-render`'s own
/// headless harness exposes no way to bind an external texture for
/// `EngineRenderer` to sample, so unlike `frust-render`'s own GPU smoke
/// test this cannot render actual pixels through a registered
/// `SceneTexture` — it proves real device bring-up and draw-recording
/// independently instead of end to end.
#[test]
#[ignore = "needs a real GPU adapter; run with `cargo test -p frust --features gpu --test \
            gpu_seam -- --ignored` (WGPU_BACKEND/WGPU_ADAPTER_NAME pin the adapter; \
            FRUST_GOLDEN_EXPECT_ADAPTER verifies it resolved)"]
fn scene_texture_seam_reaches_a_real_headless_device_through_the_facade_context() {
    block_on(async {
        let mut context = Context::new();
        context
            .ensure_device_headless()
            .await
            .expect("frust::gpu::Context headless device creation");
        let caps = context
            .caps()
            .expect("caps are populated once a device exists");

        if let Ok(expected) = std::env::var("FRUST_GOLDEN_EXPECT_ADAPTER") {
            let expected = expected.trim();
            if !expected.is_empty() {
                assert!(
                    caps.adapter_name
                        .to_lowercase()
                        .contains(&expected.to_lowercase()),
                    "expected an adapter matching `{expected}`, resolved `{}` instead",
                    caps.adapter_name
                );
            }
        }
        println!(
            "frust::gpu::Context resolved adapter: {}",
            caps.adapter_name
        );
    });

    record_scene_texture_draw();
}
