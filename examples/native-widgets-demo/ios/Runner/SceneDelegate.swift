import UIKit
import FrustEmbedding

// Hand-authored (the scaffold copies the template's bare
// `class SceneDelegate: FrustSceneDelegate {}`, which is Mode A): a
// `FrustViewController` subclass installed via
// `FrustSceneDelegate.makeRootViewController()`, the same shape as
// `examples/playground/ios/Runner/SceneDelegate.swift`.
private class MainViewController: FrustViewController {
    /// Mode B (translucent surface) by default, like playground: the demo's
    /// pages host real UIKit controls as platform views beside their
    /// frust-drawn peers, and Mode B is the surface mode those embedded views
    /// ship under on a device.
    ///
    /// Mode A when the Xcode scheme's environment sets `FRUST_DEMO_OPAQUE`
    /// (any value, e.g. `FRUST_DEMO_OPAQUE=1`): the iOS Simulator renders a
    /// frust surface only in Mode A (`translucentSurface == false`), so the
    /// simulator gates run with the variable set. Read once: `FrustViewController` reads
    /// this before the first frame and the surface's alpha mode is fixed for
    /// the process lifetime.
    override var translucentSurface: Bool {
        ProcessInfo.processInfo.environment["FRUST_DEMO_OPAQUE"] == nil
    }

    /// Arms the present-sync seam so a scroll-driven frame's surface
    /// presentation waits for the already-committed geometry of any hosted
    /// platform view — every catalog page scrolls native controls inside a
    /// frust `scroll_view`, the case the native/frust desync is visible in.
    /// `FrustViewController` implements the coupling contract (drives both
    /// `CAMetalLayer.presentsWithTransaction` and `frust_set_present_sync`
    /// together).
    override var synchronizesPresentWithPlatformViews: Bool { true }
}

class SceneDelegate: FrustSceneDelegate {
    override func makeRootViewController() -> FrustViewController {
        MainViewController(nibName: nil, bundle: nil)
    }
}
