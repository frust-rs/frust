// THROWAWAY — native-widgets Phase 0 spike: the ONE generic listener class.
// One native method dispatching by (controlId, eventKind) — the runbook's
// "do not grow a class per control" rule. Rust constructs instances via JNI
// (`new FrustNativeListener(id)`) and attaches them with setOnClickListener.
package dev.frust

import android.view.View

class FrustNativeListener(private val controlId: Long) : View.OnClickListener {
    override fun onClick(v: View) {
        nativeOnEvent(controlId, EVENT_CLICK)
    }

    private external fun nativeOnEvent(controlId: Long, kind: Int)

    private companion object {
        const val EVENT_CLICK = 1
    }
}
