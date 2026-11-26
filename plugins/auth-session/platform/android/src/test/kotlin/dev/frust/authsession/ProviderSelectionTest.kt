package dev.frust.authsession

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Test

/**
 * JVM-only coverage for [chooseCustomTabsProvider] — the
 * pure decision half of `FrustAuthSessionHost.customTabsProvider`'s
 * Custom Tabs provider-selection policy (see that method's KDoc for the
 * full three-step policy this exercises) — and for [chooseLaunchMode], the
 * choice between Auth Tab and a Custom Tab for the provider it picked. No
 * Android framework classes and no Robolectric: [chooseCustomTabsProvider]'s
 * `answersCustomTabs` parameter stands in for
 * `CustomTabsClient.getPackageName`, and [chooseLaunchMode]'s
 * `authTabSupported` for `CustomTabsClient.isAuthTabSupported`, so every
 * branch is driven with plain fixtures.
 *
 * This harness exists because the policy regressed twice on-device during
 * the auth-session plan with no automated coverage to catch it — most
 * recently the `installedBrowsers` regression commit `31512f38` fixes
 * (`MATCH_DEFAULT_ONLY` hid every non-default browser from the allow-list
 * intersection; `MATCH_ALL` is the fix), guarded by the last test below.
 */
class ProviderSelectionTest {

    @Test
    fun `default browser answers Custom Tabs, is pinned`() {
        val chosen = chooseCustomTabsProvider(
            defaultPackage = "com.android.chrome",
            installedBrowsers = setOf("com.android.chrome"),
        ) { packages, ignoreDefault ->
            if (!ignoreDefault && packages == listOf("com.android.chrome")) "com.android.chrome" else null
        }
        assertEquals("com.android.chrome", chosen)
    }

    @Test
    fun `default does not answer, allow-listed installed browser is pinned in allow-list order`() {
        val chosen = chooseCustomTabsProvider(
            defaultPackage = "com.some.unlisted.browser",
            installedBrowsers = setOf("org.mozilla.firefox", "com.android.chrome"),
        ) { packages, ignoreDefault ->
            when {
                !ignoreDefault -> null // the unlisted default never answers Custom Tabs
                else -> {
                    // Allow-list preference order puts chrome ahead of firefox
                    // regardless of `installedBrowsers`' (unordered) set order.
                    assertEquals(listOf("com.android.chrome", "org.mozilla.firefox"), packages)
                    packages.firstOrNull()
                }
            }
        }
        assertEquals("com.android.chrome", chosen)
    }

    @Test
    fun `a getPackageName answer outside the allow-list is never pinned`() {
        val chosen = chooseCustomTabsProvider(
            defaultPackage = null,
            installedBrowsers = setOf("com.android.chrome"),
        ) { _, ignoreDefault ->
            // A hypothetical future `getPackageName` returning a package this
            // host never allow-listed must still not be pinned.
            if (ignoreDefault) "com.untrusted.browser" else null
        }
        assertNull(chosen)
    }

    @Test
    fun `no browsers installed and none answer, yields null (unpinned)`() {
        val chosen = chooseCustomTabsProvider(
            defaultPackage = null,
            installedBrowsers = emptySet(),
        ) { _, _ -> null }
        assertNull(chosen)
    }

    @Test
    fun `default installed but silent, MATCH_ALL set surfaces a non-default allow-listed browser (31512f38 regression)`() {
        val chosen = chooseCustomTabsProvider(
            defaultPackage = "com.android.chrome",
            installedBrowsers = setOf("com.android.chrome", "org.mozilla.firefox"),
        ) { packages, ignoreDefault ->
            when {
                !ignoreDefault -> null // the default (chrome) is installed but does not answer Custom Tabs
                else -> packages.firstOrNull { it == "org.mozilla.firefox" }
            }
        }
        assertEquals("org.mozilla.firefox", chosen)
    }

    /** Stands in for the launching `ComponentActivity`: the decision only carries it. */
    private val launchingActivity = Any()

    @Test
    fun `provider with Auth Tab and a ComponentActivity uses Auth Tab for both redirect forms`() {
        for (customScheme in listOf("frustplay", null)) {
            val mode = chooseLaunchMode(customScheme, "com.android.chrome", launchingActivity) {
                it == "com.android.chrome"
            }
            assertEquals(LaunchMode.AuthTab("com.android.chrome", launchingActivity), mode)
            // The launch reads the Activity it registers on from the answer itself.
            assertSame(launchingActivity, (mode as LaunchMode.AuthTab).activity)
        }
    }

    @Test
    fun `provider without Auth Tab keeps the Custom Tabs path for a custom scheme`() {
        val mode = chooseLaunchMode("frustplay", "org.mozilla.fennec_fdroid", launchingActivity) { false }
        assertEquals(LaunchMode.CustomTabs("org.mozilla.fennec_fdroid", "frustplay"), mode)
    }

    @Test
    fun `provider without Auth Tab cannot take an https redirect`() {
        val mode = chooseLaunchMode(null, "org.mozilla.fennec_fdroid", launchingActivity) { false }
        assertEquals(LaunchMode.Unsupported, mode)
    }

    @Test
    fun `no provider never asks about Auth Tab and never uses it`() {
        val custom = chooseLaunchMode("frustplay", null, launchingActivity) {
            throw AssertionError("isAuthTabSupported asked without a provider")
        }
        assertEquals(LaunchMode.CustomTabs(null, "frustplay"), custom)
        val https = chooseLaunchMode(null, null, launchingActivity) {
            throw AssertionError("isAuthTabSupported asked without a provider")
        }
        assertEquals(LaunchMode.Unsupported, https)
    }

    @Test
    fun `an Activity with no result registry falls back even when the provider offers Auth Tab`() {
        val none: Any? = null
        assertEquals(
            LaunchMode.CustomTabs("com.android.chrome", "frustplay"),
            chooseLaunchMode("frustplay", "com.android.chrome", none) { true },
        )
        assertEquals(LaunchMode.Unsupported, chooseLaunchMode(null, "com.android.chrome", none) { true })
    }
}
