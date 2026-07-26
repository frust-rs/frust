// The `frust-native-widgets` plugin's ONE listener class: every platform
// listener interface a control needs is implemented here and funnelled into a
// single native method, dispatched by `(slotId, kind)` — never a class per
// control (the spike's proven shape).
//
// Rust constructs instances itself (`new FrustNativeListener(slotId)` via JNI,
// `NativeCtx::new_listener`) and attaches them with the matching setter; the
// instance is retained alongside the control's view so both are released
// together on dispose.
//
// `detail` carries a primitive payload packed into a `long` — never JSON:
// a value listener can fire at drag rate, and allocating a string per event
// on the main thread is exactly what the "no JSON on the hot path" rule is
// about. Each kind below documents its own packing.
//
// **Native-widget events bypass frust's event pipeline entirely**: this is a
// platform interaction surfacing as a Rust callback (typically a signal
// write, waking exactly one frust frame), not a `RenderRoot::event` pointer
// event — no capture, no focus, none of frust's press semantics apply.
package dev.frust

import android.view.View

class FrustNativeListener(private val slotId: Long) : View.OnClickListener {
    /** `detail` is unused (0) for a click. */
    override fun onClick(v: View) {
        nativeOnEvent(slotId, KIND_CLICK, 0L)
    }

    private external fun nativeOnEvent(slotId: Long, kind: Int, detail: Long)

    private companion object {
        /**
         * `View.OnClickListener.onClick`. The value-carrying kinds
         * (`CompoundButton.OnCheckedChangeListener`,
         * `SeekBar.OnSeekBarChangeListener`) join this table with the Switch
         * and Slider controls; the codes are shared verbatim with the Rust
         * `EVENT_KIND_*` constants (`plugins/native-widgets/src/runtime.rs`),
         * so the two tables must be edited together.
         */
        const val KIND_CLICK = 1
    }
}
