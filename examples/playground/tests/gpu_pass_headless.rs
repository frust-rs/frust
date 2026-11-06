//! The playground's `SpinningQuad` external pass rendered offscreen through
//! the public facade (`frust::gpu::headless`), proven with pixel probes — no
//! stored baseline.
//!
//! Needs a real queue, so the case is `#[ignore]`d. From `examples/playground`:
//!
//! ```text
//! WGPU_BACKEND=vulkan cargo test --test gpu_pass_headless -- --ignored --test-threads=1
//! ```

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use frust::authoring::scene::{Scene, SceneBuilder};
use frust::gpu::headless::{HeadlessOptions, HeadlessRenderer, HeadlessSpec};
use frust::gpu::{ExternalPass, SceneTextureId};
use kurbo::Rect;
use playground::pages::gpu_pass::SpinningQuad;

const SIZE: u32 = 64;
const DEST: Rect = Rect::new(8.0, 8.0, 56.0, 56.0);

/// Wakes the thread blocked in [`block_on`].
struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// A std-only executor: the playground has no async runtime dependency.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(output) => return output,
            Poll::Pending => thread::park(),
        }
    }
}

#[test]
#[ignore = "requires a GPU; run with `cargo test --test gpu_pass_headless -- --ignored` from examples/playground"]
fn spinning_quad_composites_through_the_facade() {
    block_on(async {
        let mut renderer = HeadlessRenderer::new(HeadlessOptions::default())
            .await
            .expect("a GPU adapter");
        let id = SceneTextureId::mint();
        let mut scene = Scene::new();
        SceneBuilder::new(&mut scene).scene_texture(id.get(), DEST);
        let spec = HeadlessSpec::new(SIZE, SIZE);
        let base = [0, 0, 0, 255];
        let pass = Arc::new(SpinningQuad::new());
        let passes: Vec<(SceneTextureId, Arc<dyn ExternalPass>)> = vec![(id, pass.clone())];

        let image = renderer
            .render_with_passes(&scene, &spec, &passes)
            .await
            .expect("render with the pass");

        assert_ne!(
            image.pixel(SIZE / 2, SIZE / 2),
            base,
            "the centre holds the pass"
        );
        for (x, y) in [(1, 1), (SIZE - 2, 1), (1, SIZE - 2), (SIZE - 2, SIZE - 2)] {
            assert_eq!(image.pixel(x, y), base, "corner ({x},{y}) is the base");
        }
        let (drain, _frame, records) = pass.counters();
        assert_ne!(drain, 0, "the pass saw a drain id");
        assert_eq!(records, 1, "one record for one render");
    });
}
