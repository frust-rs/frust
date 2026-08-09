package it.f0x.playground

import dev.frust.FrustActivity

// Mode B (translucent-surface) testbed — see `playground::pages::platform_views`.
class MainActivity : FrustActivity() {
    override val translucentSurface = true
}
