import UIKit

// Catalog-local (glyph-catalog only, not part of the template) native-view
// factory for the "Platform Views" section's Mode B demo slot
// (`glyphcatalog::pages::platform_views`) — the iOS mirror of
// `dev/frust/DemoStreamFactory.kt`'s Android counterpart. Hosts a `UILabel`
// that ticks its own counter at ~20Hz on a plain `Timer` — deliberately not
// the template's debug `FrustTestLabelFactory` (which ticks at 2Hz): this
// factory exists specifically to prove the zero-frust-frames self-update
// property (platform-views task 11's device trace) with an update cadence
// visibly independent of both the display refresh rate and the frust frame
// loop.
//
// `@objc(DemoStreamFactory)` fixes the ObjC runtime name `FrustViewHost`
// resolves via `NSClassFromString` — see `FrustPlatformViewFactory.swift`'s
// module doc: no package/dot syntax, matching `platform_views.rs`'s
// `DEMO_STREAM_VIEW_TYPE` iOS arm (`"DemoStreamFactory"`, not
// `"dev.frust.DemoStreamFactory"`).
@objc(DemoStreamFactory)
final class DemoStreamFactory: NSObject, FrustPlatformViewFactory {
    func createView(paramsJson: String) -> UIView {
        return SelfUpdatingStreamLabel(paramsJson: paramsJson)
    }

    func updateParams(_ view: UIView, paramsJson: String) {
        (view as? SelfUpdatingStreamLabel)?.updateParams(paramsJson)
    }
}

/// A `UILabel` that redraws itself every `TICK_INTERVAL` (~20Hz) off its own
/// `Timer`, wholly independent of Frust's `CADisplayLink`-driven frame loop
/// — so a device trace can confirm this view redraws with ZERO frust frames.
private final class SelfUpdatingStreamLabel: UILabel {
    /// ~20Hz (1.0 / 20 = 0.05s) — see the enclosing factory's doc comment.
    private static let tickInterval: TimeInterval = 0.05

    private var paramsJson: String
    private var counter = 0
    private var timer: Timer?

    init(paramsJson: String) {
        self.paramsJson = paramsJson
        super.init(frame: .zero)
        numberOfLines = 0
        textAlignment = .center
        textColor = .white
        backgroundColor = UIColor(red: 0, green: 0.35, blue: 0.24, alpha: 0.7)
        render()
        let timer = Timer(timeInterval: Self.tickInterval, repeats: true) { [weak self] _ in
            guard let self else { return }
            self.counter += 1
            self.render()
        }
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func updateParams(_ paramsJson: String) {
        self.paramsJson = paramsJson
        render()
    }

    private func render() {
        text = "DemoStream[\(counter)]\n\(paramsJson)"
    }

    deinit {
        timer?.invalidate()
    }
}
