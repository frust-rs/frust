import UIKit

// The scene-delegate half of Frust's iOS embedding — everything a generated
// app's `SceneDelegate` used to spell out itself, so an app is left with:
//
//     import UIKit
//     import FrustEmbedding
//
//     class SceneDelegate: FrustSceneDelegate {}
//
// Roots the window at the Frust render surface's view controller, passed
// through `makeWindowRootViewController(hosting:)` — the default returns it
// unchanged, so the window root is that controller. The root may be any
// container. `frustViewController` is the one frust surface: it stays valid
// when a plugin re-roots the window at runtime (the native-widgets chrome may
// place it under a tab bar), and such a re-rooting must leave
// `frustViewController` where UIKit can lay it out. Deep links route to it,
// never to the window root.
//
// Deep links: a frust app is scene-based (see the generated
// `Info.plist`'s `UIApplicationSceneManifest`), so — per Apple's documented
// deep-link contract for scene apps — a cold-start link arrives as
// `connectionOptions.urlContexts` in `scene(_:willConnectTo:options:)`, and a
// running-app link arrives via `scene(_:openURLContexts:)` below;
// `AppDelegate.application(_:open:)` is NOT called for a scene-based app and is
// therefore not used for this. Both paths funnel into the same
// `FrustViewController.handleDeepLink(_:)`, which queues until `frust_init`
// has a handle.
//
// Windowing (iPadOS 26+): an app subclass may override
// `preferredWindowingControlStyle(for:)` (returning `.automatic`, `.unified`
// or `.minimal`; gate it with `#available(iOS 26.0, *)`) and set
// `windowScene.sizeRestrictions?.minimumSize` inside its own
// `scene(_:willConnectTo:options:)` after calling `super`. No shell support is
// needed for either. Whatever style is in effect, the shell reports the window
// control's footprint to Rust as `WindowInsets.corner_insets` (see the rustdoc
// on `CornerInsets`), which is how a bar keeps its slots out from under it.
open class FrustSceneDelegate: UIResponder, UIWindowSceneDelegate {
    public var window: UIWindow?

    /// The one frust surface this scene installs. Set in
    /// `scene(_:willConnectTo:options:)` and independent of where the window
    /// root sits, so deep links keep reaching the surface after a re-rooting.
    public private(set) var frustViewController: FrustViewController?

    /// The root view controller this scene installs. Override to substitute an
    /// app's own `FrustViewController` subclass — which is also how an app opts
    /// into a translucent (Mode B) surface, by overriding
    /// `FrustViewController.translucentSurface`.
    open func makeRootViewController() -> FrustViewController {
        FrustViewController(nibName: nil, bundle: nil)
    }

    /// The window root for `controller`. Override to wrap the frust controller
    /// in an app's own container (`UINavigationController`,
    /// `UITabBarController`). The default returns `controller` unchanged.
    open func makeWindowRootViewController(hosting controller: FrustViewController) -> UIViewController {
        controller
    }

    open func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let windowScene = scene as? UIWindowScene else { return }
        let window = UIWindow(windowScene: windowScene)
        let controller = makeRootViewController()
        frustViewController = controller
        window.rootViewController = makeWindowRootViewController(hosting: controller)
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
            let controller = frustViewController
        else { return }
        controller.handleDeepLink(url)
    }
}
