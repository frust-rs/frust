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
import android.widget.CompoundButton
import android.widget.SeekBar

class FrustNativeListener(private val slotId: Long) :
    View.OnClickListener,
    CompoundButton.OnCheckedChangeListener,
    SeekBar.OnSeekBarChangeListener {

    /** `detail` is unused (0) for a click. */
    override fun onClick(v: View) {
        nativeOnEvent(slotId, KIND_CLICK, 0L)
    }

    /**
     * `detail` packs the checked state as 0/1 (see the Rust side's
     * `pack_bool`/`unpack_bool` in `plugins/native-widgets/src/events.rs`).
     */
    override fun onCheckedChanged(buttonView: CompoundButton, isChecked: Boolean) {
        nativeOnEvent(slotId, KIND_TOGGLED, if (isChecked) 1L else 0L)
    }

    /**
     * `detail` packs `progress` — **platform-space**, zero-based (Rust adds
     * the control's own `min` back; `SeekBar.setMin` needs API 26 and this
     * plugin's floor is 24) — in the low 32 bits, and `fromUser` in bit 32.
     * See the Rust side's `pack_value_changed`/`unpack_value_changed`.
     */
    override fun onProgressChanged(seekBar: SeekBar, progress: Int, fromUser: Boolean) {
        val detail = (progress.toLong() and 0xFFFFFFFFL) or (if (fromUser) 1L shl 32 else 0L)
        nativeOnEvent(slotId, KIND_VALUE_CHANGED, detail)
    }

    /** `detail` is unused (0) — a drag gesture starting. */
    override fun onStartTrackingTouch(seekBar: SeekBar) {
        nativeOnEvent(slotId, KIND_DRAG_START, 0L)
    }

    /** `detail` is unused (0) — a drag gesture ending. */
    override fun onStopTrackingTouch(seekBar: SeekBar) {
        nativeOnEvent(slotId, KIND_DRAG_END, 0L)
    }

    private external fun nativeOnEvent(slotId: Long, kind: Int, detail: Long)

    private companion object {
        /**
         * `View.OnClickListener.onClick`. The codes below are shared
         * verbatim with the Rust `EVENT_KIND_*` constants
         * (`plugins/native-widgets/src/events.rs`), so the two tables must
         * be edited together.
         */
        const val KIND_CLICK = 1
        /** `CompoundButton.OnCheckedChangeListener.onCheckedChanged`. */
        const val KIND_TOGGLED = 2
        /** `SeekBar.OnSeekBarChangeListener.onProgressChanged`. */
        const val KIND_VALUE_CHANGED = 3
        /** `SeekBar.OnSeekBarChangeListener.onStartTrackingTouch`. */
        const val KIND_DRAG_START = 4
        /** `SeekBar.OnSeekBarChangeListener.onStopTrackingTouch`. */
        const val KIND_DRAG_END = 5
    }
}
