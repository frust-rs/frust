import UIKit

// The application-delegate half of Frust's iOS embedding — everything a
// generated app's `AppDelegate` used to spell out itself, so an app is left
// with:
//
//     import UIKit
//     import FrustEmbedding
//
//     @main class AppDelegate: FrustAppDelegate {}
//
// A minimal UIKit bootstrap: the app is scene-based, so the delegate just
// declares the default scene configuration and hands the rest to
// `FrustSceneDelegate`. Every method is `open` — an app that needs launch-time
// work of its own overrides one and calls `super`.
open class FrustAppDelegate: UIResponder, UIApplicationDelegate {
    open func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        return true
    }

    open func application(
        _ application: UIApplication,
        configurationForConnecting connectingSceneSession: UISceneSession,
        options: UIScene.ConnectionOptions
    ) -> UISceneConfiguration {
        return UISceneConfiguration(
            name: "Default Configuration",
            sessionRole: connectingSceneSession.role
        )
    }
}
