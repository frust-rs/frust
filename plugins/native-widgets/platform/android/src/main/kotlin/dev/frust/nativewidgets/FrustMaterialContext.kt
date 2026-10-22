// The Material theming helper the native-widgets plugin's Kotlin shares: the
// segmented control (FrustNativeControlFactory) and the action sheet
// (FrustNativePresenter) both need a context whose theme carries the AppCompat
// marker Material checks for. It lives in its own internal file so the frozen
// JNI-facing factory class keeps its exported surface unchanged; it is called
// from Kotlin only, never over JNI.
package dev.frust.nativewidgets

import android.app.Activity
import android.content.Context
import android.content.res.Configuration
import android.view.ContextThemeWrapper

internal object FrustMaterialContext {
    /**
     * The Activity's manifest theme, with the night/locale configuration
     * of [qualified] when given (else the Activity's), and the night bit
     * forced when [dark] is non-null, as FrustNativeChrome themes its
     * bars. The night-qualified context alone fails Material's AppCompat
     * theme check: it is a bare ContextImpl with the framework theme and
     * no Activity to unwrap; the raw Activity context fails it too (its
     * live theme lacks the AppCompat marker the manifest theme carries).
     */
    fun wrap(activity: Activity, qualified: Context? = null, dark: Boolean? = null): Context {
        val theme = activity.packageManager.getActivityInfo(activity.componentName, 0).themeResource
        // A zero theme resource would leave the wrapper on the framework default
        // theme and fail Material's check later, far from the cause.
        check(theme != 0) { "the Activity declares no theme in its manifest" }
        val wrapper = ContextThemeWrapper(activity, theme)
        val config = Configuration(qualified?.resources?.configuration ?: activity.resources.configuration)
        if (dark != null) {
            val night = if (dark) Configuration.UI_MODE_NIGHT_YES else Configuration.UI_MODE_NIGHT_NO
            config.uiMode = (config.uiMode and Configuration.UI_MODE_NIGHT_MASK.inv()) or night
        }
        wrapper.applyOverrideConfiguration(config)
        return wrapper
    }
}
