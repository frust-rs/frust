import UIKit

// The scene-delegate half of Frust's iOS embedding — everything a generated
// app's `SceneDelegate` used to spell out itself, so an app is left with:
//
//     import UIKit
//     import FrustEmbedding
//
//     class SceneDelegate: FrustSceneDelegate {}
//
// Roots the window at the Frust render surface's view controller.
//
// Deep links (task 07): a frust app is scene-based (see the generated
// `Info.plist`'s `UIApplicationSceneManifest`), so — per Apple's documented
// deep-link contract for scene apps — a cold-start link arrives as
// `connectionOptions.urlContexts` in `scene(_:willConnectTo:options:)`, and a
// running-app link arrives via `scene(_:openURLContexts:)` below;
// `AppDelegate.application(_:open:)` is NOT called for a scene-based app and is
// therefore not used for this. Both paths funnel into the same
// `FrustViewController.handleDeepLink(_:)`, which queues until `frust_init`
// has a handle.
open class FrustSceneDelegate: UIResponder, UIWindowSceneDelegate {
    public var window: UIWindow?

    /// The root view controller this scene installs. Override to substitute an
    /// app's own `FrustViewController` subclass — which is also how an app opts
    /// into a translucent (Mode B) surface, by overriding
    /// `FrustViewController.translucentSurface`.
    open func makeRootViewController() -> FrustViewController {
        FrustViewController(nibName: nil, bundle: nil)
    }

    open func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let windowScene = scene as? UIWindowScene else { return }
        let window = UIWindow(windowScene: windowScene)
        let controller = makeRootViewController()
        window.rootViewController = controller
        self.window = window
        window.makeKeyAndVisible()

        // Cold-start deep link, if the app was launched via a URL. Queued
        // by the controller until `frust_init` has a handle (it hasn't
        // run yet at this point — see `FrustViewController.pendingDeepLink`).
        if let url = connectionOptions.urlContexts.first?.url {
            controller.handleDeepLink(url)
        }
    }

    /// A running app's deep link (the app was already foregrounded/
    /// backgrounded, not launched fresh). First URL only — this app opens a
    /// single scene/window, so `URLContexts` has at most one entry in
    /// practice.
    open func scene(_ scene: UIScene, openURLContexts URLContexts: Set<UIOpenURLContext>) {
        guard let url = URLContexts.first?.url,
            let controller = window?.rootViewController as? FrustViewController
        else { return }
        controller.handleDeepLink(url)
    }
}
