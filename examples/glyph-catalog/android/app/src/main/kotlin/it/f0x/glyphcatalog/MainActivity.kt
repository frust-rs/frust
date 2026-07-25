package it.f0x.glyphcatalog

import dev.frust.FrustActivity

// Mode B (translucent-surface) testbed — see `glyphcatalog::pages::platform_views`.
class MainActivity : FrustActivity() {
    override val translucentSurface = true
}
