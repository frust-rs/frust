package dev.frust

import android.app.Activity
import android.content.Context
import android.graphics.Color
import android.os.Handler
import android.os.Looper
import android.view.View
import android.widget.TextView

/**
 * Catalog-local (glyph-catalog only, not part of the template) native-view
 * factory for the "Platform Views" section's Mode B demo slot
 * (`glyphcatalog::pages::platform_views`). Hosts a [TextView] that ticks its
 * own counter at ~20Hz on a plain [Handler] loop — deliberately **not** the
 * template's `TestLabelFactory` (which ticks on [android.view.Choreographer],
 * i.e. the display's own vsync rate): this factory exists specifically to
 * prove the zero-frust-frames self-update property with an update cadence
 * visibly independent of both the display refresh rate and the frust frame
 * loop.
 *
 * `updateParams` (the widget's `params_json` "updateParams" stress toggle)
 * is shown inline in the label so a params bump is visible without disposing
 * the view.
 */
class DemoStreamFactory : FrustPlatformViewFactory {
    override fun createView(activity: Activity, context: Context, paramsJson: String): View {
        return SelfUpdatingStreamLabel(context, paramsJson)
    }

    override fun updateParams(view: View, paramsJson: String) {
        (view as? SelfUpdatingStreamLabel)?.updateParams(paramsJson)
    }

    /**
     * A [TextView] that ticks its own counter every [TICK_INTERVAL_MS] (~20Hz)
     * on a [Handler] bound to the main [Looper] — its own loop, wholly
     * independent of Frust's Choreographer-driven frame loop, so a device
     * trace can confirm this view redraws with ZERO frust frames.
     */
    private class SelfUpdatingStreamLabel(
        context: Context,
        private var paramsJson: String,
    ) : TextView(context), Runnable {
        private var counter = 0L
        private val handler = Handler(Looper.getMainLooper())

        init {
            setBackgroundColor(Color.argb(180, 0, 90, 60))
            setTextColor(Color.WHITE)
            textSize = 13f
            setPadding(12, 12, 12, 12)
            render()
        }

        fun updateParams(paramsJson: String) {
            this.paramsJson = paramsJson
            render()
        }

        private fun render() {
            text = "DemoStream[$counter]\n$paramsJson"
        }

        override fun onAttachedToWindow() {
            super.onAttachedToWindow()
            handler.postDelayed(this, TICK_INTERVAL_MS)
        }

        override fun onDetachedFromWindow() {
            handler.removeCallbacks(this)
            super.onDetachedFromWindow()
        }

        override fun run() {
            if (!isAttachedToWindow) return
            counter++
            render()
            handler.postDelayed(this, TICK_INTERVAL_MS)
        }

        companion object {
            /** ~20Hz (1000ms / 20 = 50ms) — see the class doc. */
            private const val TICK_INTERVAL_MS = 50L
        }
    }
}
