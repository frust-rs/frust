// THROWAWAY — native-widgets Phase 0 spike (spikes 1/3/4b Android arm).
// The plugin's ONLY Kotlin factory: `createView` immediately hands off to the
// Rust export `nativeCreateControl`, which builds the real control via JNI and
// returns it. Package `dev.frust` so FrustViewHost's reflection gate resolves
// the FQCN "dev.frust.FrustNativeSpikeFactory".
package dev.frust

import android.app.Activity
import android.content.Context
import android.util.Log
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController
import org.json.JSONObject

class FrustNativeSpikeFactory : FrustPlatformViewFactory {
    override fun createView(activity: Activity, context: Context, paramsJson: String): View {
        lastActivity = activity
        return nativeCreateControl(paramsJson, activity, context)
    }

    override fun updateParams(view: View, paramsJson: String) {
        // Spike 4b piggyback: a "bars":1/0 param drives WindowInsetsController
        // hide/show on the SDK-36 device — pure Kotlin, no Rust involvement
        // (the question is about the OS, not FFI).
        val bars = try { JSONObject(paramsJson).optInt("bars", -1) } catch (_: Exception) { -1 }
        if (bars >= 0) applyBars(view, bars == 1)
        nativeUpdateParams(view, paramsJson)
    }

    override fun disposeView(view: View) {
        nativeDisposeControl(view)
    }

    private fun applyBars(view: View, hide: Boolean) {
        val window = lastActivity?.window ?: return
        val controller = window.insetsController ?: run {
            Log.i("frust", "spike-bars insetsController=null")
            return
        }
        val types = WindowInsets.Type.statusBars() or WindowInsets.Type.navigationBars()
        if (hide) {
            controller.systemBarsBehavior =
                WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            controller.hide(types)
        } else {
            controller.show(types)
        }
        Log.i("frust", "spike-bars requested hide=$hide sdk=${android.os.Build.VERSION.SDK_INT}")
        // The receipt: after the change settles, report what the platform
        // actually did — the 4b verdict line.
        view.postDelayed({
            val insets = view.rootWindowInsets
            val statusVisible = insets?.isVisible(WindowInsets.Type.statusBars())
            val navVisible = insets?.isVisible(WindowInsets.Type.navigationBars())
            Log.i(
                "frust",
                "spike-bars verdict hide=$hide statusVisible=$statusVisible navVisible=$navVisible"
            )
        }, 700)
    }

    private companion object {
        // The factory is instantiated reflectively per view type; the activity
        // is cached only for the 4b bars call (spike-grade, never outlives the
        // spike page's use).
        @Volatile
        private var lastActivity: Activity? = null
    }

    private external fun nativeCreateControl(
        paramsJson: String,
        activity: Activity,
        context: Context,
    ): View

    private external fun nativeUpdateParams(view: View, paramsJson: String)

    private external fun nativeDisposeControl(view: View)
}
