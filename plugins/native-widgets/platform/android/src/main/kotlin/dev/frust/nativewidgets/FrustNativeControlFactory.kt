// The `frust-native-widgets` plugin's ONE Kotlin factory — the entire Android
// class surface this plugin will ever have, however many controls it grows.
//
// Every method immediately hands off to a Rust JNI export
// (`plugins/native-widgets/src/android/mod.rs`), which builds, mutates and
// tears down the real `android.widget` views itself; which *control* a slot
// means travels in `paramsJson` under the runtime's reserved
// `__frustControl`/`__frustSlot` keys, so a new control never needs a new
// class here (the "no per-control Kotlin, ever" rule).
//
// Package `dev.frust.nativewidgets` is baked into the mangled JNI symbol names
// on the Rust side (`Java_dev_frust_nativewidgets_FrustNativeControlFactory_*`)
// and is therefore **fixed once shipped** — `docs/CODE_STANDARDS.md`'s
// JNI-export-names-are-LAW rule. The fully-qualified name
// `dev.frust.nativewidgets.FrustNativeControlFactory` is also the platform-view
// `viewType` string the api layer publishes (`crate::api::builders`' Android
// `VIEW_TYPE`), which the embedding's `FrustViewHost` resolves reflectively
// through the application classloader — the subpackage keeps the `dev.frust.`
// prefix that host requires while staying OUT of the embedding module's
// exclusive bare `dev.frust` package.
//
// This file ships inside the plugin's own `com.android.library` module
// (`plugins/native-widgets/platform/android`), wired into a consuming app by
// `Contribution::GradleModule` — never copied into an app's source tree.
package dev.frust.nativewidgets

import android.app.Activity
import android.content.Context
import android.view.View
import dev.frust.FrustPlatformViewFactory

class FrustNativeControlFactory : FrustPlatformViewFactory {
    /**
     * Build the control this slot's params name. On success Rust returns the
     * freshly created `View`; on failure (the params carry no known control
     * kind, construction failed, a re-entrant runtime call, an unreadable
     * paramsJson, or a caught Rust panic) Rust throws instead of returning
     * null — `FrustViewHost`'s `catch (Throwable)` logs that and marks the
     * slot dead rather than crashing its frame loop.
     */
    override fun createView(activity: Activity, context: Context, paramsJson: String): View =
        nativeCreateControl(paramsJson, activity, context)

    /**
     * A params change for an already-created control. Rust diffs the decoded
     * props against the ones it last applied and does nothing at all when they
     * are equal, so calling this every frame is free.
     */
    override fun updateParams(view: View, paramsJson: String) {
        nativeUpdateParams(view, paramsJson)
    }

    /**
     * The slot is gone: Rust releases every global reference the control
     * retained (the leak bar — ART aborts the process at 51,200 live global
     * refs). Resolution is by object identity, so a late dispose naming a
     * view that was already replaced is a silent no-op.
     */
    override fun disposeView(view: View) {
        nativeDisposeControl(view)
    }

    private external fun nativeCreateControl(
        paramsJson: String,
        activity: Activity,
        context: Context,
    ): View

    private external fun nativeUpdateParams(view: View, paramsJson: String)

    private external fun nativeDisposeControl(view: View)
}
