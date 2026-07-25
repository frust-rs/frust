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
}

class SceneDelegate: FrustSceneDelegate {
    override func makeRootViewController() -> FrustViewController {
        MainViewController(nibName: nil, bundle: nil)
    }
}
