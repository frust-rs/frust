package dev.frust

import android.app.Activity
import android.content.Context
import android.view.View

/**
 * The plugin-facing contract for hosting a native Android [View] inside a
 * Frust app (platform-views feature). This interface is **LAW** — as fixed as
 * the `Java_dev_frust_FrustSurfaceView_native*` JNI names: a plugin author (or
 * app author) implements it as a `dev.frust.<Something>Factory` class, and the
 * fully-qualified class name is exactly the `viewType` string a Frust widget
 * passes to `frust::platform_view(...)`. [FrustViewHost] resolves that string
 * through the application classloader by convention (no registration step) and
 * caches the instance per `viewType`, so the class MUST have a public no-arg
 * constructor.
 *
 * All three methods are invoked on the **main (UI) thread** — [FrustViewHost]
 * drives them from `FrustSurfaceView.doFrame`, which already runs there — and
 * MUST NOT block. A thrown exception is caught by the host: it logs and marks
 * the offending slot dead rather than crashing the Choreographer frame loop.
 *
 * **Input contract (v1):** hosted views receive OS-routed input directly (the
 * platform hit-tests them like any sibling view); Frust does not forward or
 * intercept gestures into or out of a hosted view in v1. Position hosted
 * interactive controls where they will not be occluded by frust-painted
 * content.
 *
 * The iOS side ships the semantically identical `FrustPlatformViewFactory`
 * Swift protocol — one contract, two languages — so plugin docs describe it
 * once.
 */
interface FrustPlatformViewFactory {
    /**
     * Create the native view for a freshly-created slot. `paramsJson` is the
     * raw JSON string the Frust widget supplied (opaque to the host — the
     * factory owns its schema). `activity` is the hosting [Activity];
     * `context` is the same activity as a [Context] (passed separately so a
     * factory that only needs a `Context` need not down-cast). Called on the
     * main thread; must not block.
     */
    fun createView(activity: Activity, context: Context, paramsJson: String): View

    /**
     * Apply an in-place params update to an already-created [view] (the Frust
     * widget changed its `params_json` without the slot being recreated).
     * Default no-op — override only if the view supports live reconfiguration.
     * Called on the main thread; must not block.
     */
    fun updateParams(view: View, paramsJson: String) {}

    /**
     * Release any resources held by [view] after [FrustViewHost] has already
     * removed it from the hierarchy (the slot was disposed). Default no-op.
     * Called on the main thread; must not block.
     */
    fun disposeView(view: View) {}
}
