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
// Package `dev.frust` (not a subpackage) is baked into the mangled JNI symbol
// names on the Rust side and therefore fixed once shipped — see the Rust
// module's *Package* note for why v1 sits here. v1 ships this file as an
// app-module source hand-copied by the plugin's consumer (the camera-style
// manual wiring); packaging it as a Gradle module is Phase 3's work.
package dev.frust

import android.app.Activity
import android.content.Context
import android.view.View

class FrustNativeControlFactory : FrustPlatformViewFactory {
    /**
     * Build the control this slot's params name. On success Rust returns the
     * freshly created `View`; on failure (the params carry no known control
     * kind, or construction failed) Rust throws instead of returning null —
     * `FrustViewHost`'s `catch (Throwable)` logs that and marks the slot dead
     * rather than crashing its frame loop.
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
