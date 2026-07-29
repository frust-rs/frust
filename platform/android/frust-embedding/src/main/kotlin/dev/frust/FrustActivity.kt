package dev.frust

import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import android.util.Log
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.activity.ComponentActivity
import androidx.activity.OnBackPressedCallback
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

/**
 * The Frust Android host activity — everything a
 * generated app's `MainActivity` used to carry inline, now framework-owned.
 * A generated `MainActivity` is `class MainActivity : FrustActivity()` and
 * nothing else; an app that needs more overrides one of the extension points
 * below rather than re-implementing the wiring.
 *
 * Hosts the Frust render surface inside a [FrameLayout] root — the surface
 * fills the root, and [FrustViewHost] adds any native sibling views
 * (platform-views feature) above or below it — and forwards the lifecycle
 * callbacks the Choreographer render loop needs. An app with no platform views
 * has an inert host (a single render-surface child, no native siblings) and
 * behaves exactly as before.
 *
 * **Extension points:** [nativeLibraryName], [translucentSurface],
 * [surfaceView], and [applySystemUiMode]. Every lifecycle override below is
 * `open` (Kotlin overrides are open unless marked `final` — do not mark any of
 * them `final`); a subclass overriding one MUST call through to `super`, since
 * this class owns the surface's lifecycle forwards.
 *
 * `ComponentActivity` (not plain `Activity`) is the base class purely for
 * [getOnBackPressedDispatcher] — see the back-press callback below; nothing
 * else about the activity's lifecycle behavior changes.
 *
 * Edge-to-edge: `WindowCompat.setDecorFitsSystemWindows(window, false)` draws
 * content under the system bars, which stay **visible**
 * (`SystemUiMode.edgeToEdge` semantics — this is not a bar-hiding fullscreen
 * mode, hence the manifest's `Theme.NoTitleBar`, not `.Fullscreen`).
 * `FrustSurfaceView`'s `OnApplyWindowInsetsListener` reports the
 * resulting system-bar/cutout/IME occlusion back into the framework via
 * `nativeOnInsetsChanged`. `windowSoftInputMode="adjustResize"` (manifest)
 * is intentionally inert once edge-to-edge is on — the IME arrives as an
 * inset push instead of a window resize, matching Flutter's model.
 *
 * Deep links: cold-start delivery forwards `intent?.data` from
 * [onCreate]; a running instance receives a link via [onNewIntent] instead
 * (the app's manifest entry declares `android:launchMode="singleTop"`
 * precisely so a warm relaunch reuses this instance rather than creating a
 * new one). Both funnel into [FrustSurfaceView.onDeepLink], which queues
 * the link until the native handle exists if it arrives first (cold-start
 * routinely races ahead of `nativeInit`, since that only runs once the
 * `SurfaceHolder` callback fires).
 *
 * Back: a single always-enabled
 * `onBackPressedDispatcher` callback offers every press to the framework
 * first ([FrustSurfaceView.dispatchBackPress], wrapping `nativeOnBackPress`).
 * When the framework didn't consume it (no navigator, or already at the root
 * — matches Flutter's `bubble` case), the callback disables itself and
 * re-dispatches so the next-priority callback in the chain runs —
 * `ComponentActivity`'s own default handler, which finishes the activity —
 * then re-enables itself for the next press.
 *
 * System UI / SystemChrome (Flutter `SystemChrome.setEnabledSystemUIMode`
 * parity): this Activity owns the `Window` a `WindowInsetsControllerCompat` needs, so
 * [FrustSurfaceView] (which owns `doFrame` and therefore the per-frame poll)
 * hands a decoded mode to [applySystemUiMode] via
 * [FrustSurfaceView.onSystemUiModeChanged], wired up in [onCreate].
 */
open class FrustActivity : ComponentActivity() {
    /**
     * The hosted render surface, created in [onCreate]. Available to a
     * subclass from `super.onCreate(...)` onwards; reading it earlier throws
     * `UninitializedPropertyAccessException`.
     */
    protected lateinit var surfaceView: FrustSurfaceView
        private set

    /**
     * The `System.loadLibrary` base name of the app's Rust library (i.e.
     * `libfoo.so` is `"foo"`), loaded once in [onCreate] before the surface is
     * constructed.
     *
     * Defaults to the `dev.frust.nativeLibrary` `<meta-data>` value the
     * scaffold writes into the app's `AndroidManifest.xml`; override it to
     * name the library some other way. Read lazily (a `get()`, not a
     * constructor-time value) so the override runs against a live `Context`.
     */
    open val nativeLibraryName: String
        get() = manifestNativeLibraryName()

    /**
     * Host opt-in for a **translucent** (Mode B) render surface — see
     * [FrustSurfaceView.translucentSurface] for the full contract. Override to
     * `true` in an app that composites Frust content over native sibling views,
     * and pair it with the Rust-side `frust::request_translucent_surface`
     * latch; the default `false` keeps the opaque (Mode A) surface.
     */
    open val translucentSurface: Boolean
        get() = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Perf instrumentation: a single fixed
        // marker at activity-create entry, under the same "frust" logcat
        // tag the Rust side's `frust-perf startup ...`/`frust-perf
        // frame ...` lines use, so the Kotlin-side gap (process start ->
        // surfaceCreated -> native init) is measurable from the same
        // logcat stream. No other behavior change.
        Log.i("frust", "frust-perf activity-create")
        WindowCompat.setDecorFitsSystemWindows(window, false)
        // The embedding is app-name-agnostic, so the native library is loaded
        // here (Flutter's `FlutterJNI.loadLibrary(context)` shape) instead of
        // from a static initializer inside FrustSurfaceView — strictly before
        // the view, whose construction can reach a native method.
        FrustSurfaceView.loadNativeLibrary(nativeLibraryName)
        surfaceView = FrustSurfaceView(this, translucentSurface)
        surfaceView.onSystemUiModeChanged = ::applySystemUiMode
        // FrameLayout root: the render surface fills it, and FrustViewHost adds
        // native sibling views (platform-views feature) above (opaque/Mode A) or
        // below (translucent/Mode B) it. Inert when the app hosts no platform
        // views — a single render-surface child, identical to the old
        // `setContentView(surfaceView)` behavior.
        val root = FrameLayout(this)
        root.addView(
            surfaceView,
            FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT,
            ),
        )
        surfaceView.platformViewHost = FrustViewHost(this, root, surfaceView)
        setContentView(root)
        surfaceView.onDeepLink(intent?.data?.toString())

        onBackPressedDispatcher.addCallback(
            this,
            object : OnBackPressedCallback(true) {
                override fun handleOnBackPressed() {
                    if (!surfaceView.dispatchBackPress()) {
                        isEnabled = false
                        onBackPressedDispatcher.onBackPressed()
                        isEnabled = true
                    }
                }
            },
        )
    }

    /**
     * Resolve the app's native library name from the
     * `dev.frust.nativeLibrary` `<meta-data>` entry in its
     * `AndroidManifest.xml` (written by `frust create`). A missing entry is a
     * hard, named error rather than the opaque `UnsatisfiedLinkError` a wrong
     * guess would produce later.
     */
    private fun manifestNativeLibraryName(): String {
        @Suppress("DEPRECATION") // The ApplicationInfoFlags overload is API 33+; minSdk is 24.
        val appInfo = packageManager.getApplicationInfo(packageName, PackageManager.GET_META_DATA)
        val name = appInfo.metaData?.getString(NATIVE_LIBRARY_META_DATA)
        check(!name.isNullOrEmpty()) {
            "Frust: no <meta-data android:name=\"$NATIVE_LIBRARY_META_DATA\" " +
                "android:value=\"<your-crate-name>\"/> inside <application> in " +
                "AndroidManifest.xml. `frust create` writes it; add it back, or " +
                "override FrustActivity.nativeLibraryName."
        }
        return name
    }

    /**
     * Apply a decoded [FrustSurfaceView.SystemUiMode] via
     * `WindowInsetsControllerCompat`, Flutter `SystemChrome.setEnabledSystemUIMode`
     * parity. Mirrors [FrustSurfaceView]'s own `updateSystemBarsAppearance`
     * (same `WindowCompat.getInsetsController` call, driving bar visibility
     * here instead of icon contrast there).
     *
     * `LeanBack` has no `WindowInsetsControllerCompat` tap-to-reveal
     * equivalent — like `Immersive`, it maps to
     * [WindowInsetsControllerCompat.BEHAVIOR_DEFAULT] (any edge swipe
     * reveals the bars); only `ImmersiveSticky` gets the transient-swipe
     * behavior. See `frust_shell_common::system_ui`'s module docs for the
     * full platform-parity notes, including the Android 16/API
     * 36+ forced-edge-to-edge caveat this Activity cannot work around.
     */
    protected open fun applySystemUiMode(mode: FrustSurfaceView.SystemUiMode) {
        val controller = WindowCompat.getInsetsController(window, surfaceView)
        when (mode) {
            is FrustSurfaceView.SystemUiMode.EdgeToEdge -> {
                controller.show(WindowInsetsCompat.Type.systemBars())
            }
            is FrustSurfaceView.SystemUiMode.Immersive -> {
                controller.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_DEFAULT
                controller.hide(WindowInsetsCompat.Type.systemBars())
            }
            is FrustSurfaceView.SystemUiMode.ImmersiveSticky -> {
                controller.systemBarsBehavior =
                    WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
                controller.hide(WindowInsetsCompat.Type.systemBars())
            }
            is FrustSurfaceView.SystemUiMode.LeanBack -> {
                controller.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_DEFAULT
                controller.hide(WindowInsetsCompat.Type.systemBars())
            }
            is FrustSurfaceView.SystemUiMode.Manual -> {
                if (mode.top) {
                    controller.show(WindowInsetsCompat.Type.statusBars())
                } else {
                    controller.hide(WindowInsetsCompat.Type.statusBars())
                }
                if (mode.bottom) {
                    controller.show(WindowInsetsCompat.Type.navigationBars())
                } else {
                    controller.hide(WindowInsetsCompat.Type.navigationBars())
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        surfaceView.onDeepLink(intent.data?.toString())
    }

    override fun onResume() {
        super.onResume()
        surfaceView.onResume()
    }

    override fun onPause() {
        surfaceView.onPause()
        super.onPause()
    }

    override fun onDestroy() {
        surfaceView.onDestroy()
        super.onDestroy()
    }

    companion object {
        /**
         * The `<meta-data>` key carrying the app's native library name — LAW,
         * like the JNI export names: `frust create` writes it into the
         * generated `AndroidManifest.xml` and [manifestNativeLibraryName] reads
         * it back. Do not rename on one side only.
         */
        const val NATIVE_LIBRARY_META_DATA = "dev.frust.nativeLibrary"
    }
}
