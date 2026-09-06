import UIKit
import FrustEmbedding

// Playground is the framework's Mode B (translucent-surface) testbed —
// see `playground::pages::platform_views`'s module docs (Rust side). The
// iOS mirror of Android's `MainActivity.translucentSurface = true`
// (`android/.../it/f0x/playground/MainActivity.kt`): a `FrustViewController`
// subclass overriding `translucentSurface`, installed via
// `FrustSceneDelegate.makeRootViewController()`.
private class MainViewController: FrustViewController {
    override var translucentSurface: Bool { true }

    /// Arms the present-sync seam so a scroll-driven frame's surface presentation
    /// stays deferred until it meets the already-committed geometry of any hosted
    /// platform view — playground is the Mode B (translucent-surface) testbed for
    /// embedded native views, so leaving this off would leave that iOS-side
    /// surface/view correction unexercised. `FrustViewController` implements the
    /// coupling contract (drives both `CAMetalLayer.presentsWithTransaction` and
    /// `frust_set_present_sync` together).
    override var synchronizesPresentWithPlatformViews: Bool { true }
}

class SceneDelegate: FrustSceneDelegate {
    override func makeRootViewController() -> FrustViewController {
        MainViewController(nibName: nil, bundle: nil)
    }
}
