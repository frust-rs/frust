//! Compile-only and headless-GPU proofs for the `frust::gpu` facade seam
//! (see `crates/frust/src/lib.rs`'s `gpu` module): an app — or a future
//! 3D-rendering crate sitting beside the facade — registers an externally
//! owned GPU texture and draws it through
//! `authoring::scene::SceneBuilder::scene_texture`, reaching the seam
//! through `frust::gpu`'s re-exported types alone.
//!
//! Gated behind the `gpu` feature this crate carries — off by default, so a
//! default `cargo test -p frust-ui` run compiles this file to nothing, the same
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
use frust::gpu::{Context, DeviceHandle, Texture, with_context};

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

/// `with_context` before any shell has installed a device answers `None` —
/// the state every host unit test and this crate's own default (`--ignored`
/// excluded) test run is in, since only the ignored test below ever calls
/// `frust_shell_common::gpu::install_gpu_handle`, and cargo's default `cargo
/// test` invocation runs the non-ignored set only (an `--ignored` run
/// replaces it rather than adding to it), so this process never sees an
/// install. Device-free: no GPU adapter needed.
#[test]
fn with_context_answers_none_before_any_shell_installs_a_device() {
    let answer = with_context(|handle| handle.caps.adapter_name.clone());
    assert!(
        answer.is_none(),
        "with_context must answer None until a shell installs a device"
    );
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
#[ignore = "needs a real GPU adapter; run with `cargo test -p frust-ui --features gpu --test \
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

/// Brings up a real headless [`DeviceHandle`] and installs it through
/// `frust_shell_common::gpu::install_gpu_handle` — the exact public seam a
/// shell's own render executor calls (see `frust-shell-desktop`'s
/// `render.rs`'s `publish_gpu_handle`), not a facade-internal shortcut — then
/// reads it back purely through the facade's [`with_context`], proving the
/// full shell-installs / app-reads round trip with no shell in the loop.
///
/// A separate device from `scene_texture_seam_reaches_a_real_headless_device_through_the_facade_context`'s
/// standalone [`Context`] above: that test proves an app can build its own
/// throwaway device; this one proves it can reach the *installed* one back
/// out through `with_context`. Both bring up a real adapter, so both are
/// `--ignored`.
#[test]
#[ignore = "needs a real GPU adapter; run with `cargo test -p frust-ui --features gpu --test \
            gpu_seam -- --ignored` (WGPU_BACKEND/WGPU_ADAPTER_NAME pin the adapter; \
            FRUST_GOLDEN_EXPECT_ADAPTER verifies it resolved)"]
fn with_context_reaches_a_handle_installed_the_way_a_shell_would() {
    let installed_adapter = block_on(async {
        let mut context = Context::new();
        context
            .ensure_device_headless()
            .await
            .expect("frust::gpu::Context headless device creation");
        let handle: DeviceHandle = context.device_handle().clone();
        let adapter_name = handle.caps.adapter_name.clone();

        if let Ok(expected) = std::env::var("FRUST_GOLDEN_EXPECT_ADAPTER") {
            let expected = expected.trim();
            if !expected.is_empty() {
                assert!(
                    adapter_name
                        .to_lowercase()
                        .contains(&expected.to_lowercase()),
                    "expected an adapter matching `{expected}`, resolved `{adapter_name}` instead"
                );
            }
        }

        // The exact call a shell's render executor makes once its own device
        // exists (`frust_shell_common::gpu::install_gpu_handle`) — never a
        // facade-internal function, since installing is shell-glue's job and
        // the facade only ever reads through `with_context`.
        frust_shell_common::gpu::install_gpu_handle(handle);
        adapter_name
    });

    let read_back_adapter = with_context(|handle| handle.caps.adapter_name.clone())
        .expect("a device was installed just above");
    assert_eq!(read_back_adapter, installed_adapter);

    println!("frust::gpu::with_context resolved adapter: {read_back_adapter}");
}
