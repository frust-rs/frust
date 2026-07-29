import UIKit
import FrustEmbedding

// This catalog is the framework's Mode B (translucent-surface) testbed —
// see `glyphcatalog::pages::platform_views`'s module docs (Rust side). The
// iOS mirror of Android's `MainActivity.translucentSurface = true`
// (`android/.../it/f0x/glyphcatalog/MainActivity.kt`): a `FrustViewController`
// subclass overriding `translucentSurface`, installed via
// `FrustSceneDelegate.makeRootViewController()`.
private class MainViewController: FrustViewController {
    override var translucentSurface: Bool { true }

    /// Arm the present-sync seam so the catalog exercises task 13's iOS fling-scroll fix.
    /// The catalog is the Mode B testbed (gate finding D3 — embedded native views with
    /// translucency); without this override, the stock testbed would skip this path.
    /// `FrustViewController` implements the coupling contract (drives both
    /// `CAMetalLayer.presentsWithTransaction` and `frust_set_present_sync` together).
    override var synchronizesPresentWithPlatformViews: Bool { true }
}

class SceneDelegate: FrustSceneDelegate {
    override func makeRootViewController() -> FrustViewController {
        MainViewController(nibName: nil, bundle: nil)
    }
}
