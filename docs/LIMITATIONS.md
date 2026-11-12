# Frust - Known Limitations

A register of accepted, known limitations an app author can actually hit —
not a bug list and not a TODO list. Open bugs and unshipped fixes are tracked
separately; this doc is for a degrade that is **measured, understood, and
deliberately shipped anyway** (accepted by the maintainers, or blocked on a documented
constraint), so it is discoverable from the docs instead of only from a gate
report or an old chat.

**Entry bar**: evidence, not suspicion. Every entry below traces to a device
gate, a review finding, or an on-device measurement — cited at the end of
each entry. Each entry has a stable id (`` `id-like-this` ``) other docs and
code comments can cite. When a limitation is fixed, delete its entry rather
than marking it resolved-in-place — this file describes current-state gaps
only.

---

### `cam-gesture-onset-lead` — platform view leads at the start of a scroll (Android)

**Observed**: over roughly the first 10 display frames (~85 ms at 120 Hz) of
a scroll gesture, a hosted platform view visibly leads frust-painted content
by ~2-3 frames. Steady state (frame 11 onward) is 0 — this is an onset-only
effect.

**Applies to**: Android, a platform view hosted inside a scrolling container
(e.g. the camera preview inside playground's `ScrollView`); measured on
a Xiaomi 12. Does not apply in texture mode: the camera's texture preview is scene content, not a hosted view.

**Why not fixed**: the shape-aware sync tail's regime detector keys off an
acquire-wait EWMA that cannot rise until the GPU queue actually builds under
fresh load — so shortening the confirm window or pre-seeding the depth (the
two cheaper ladder rungs) measurably do **not** shorten onset: an A/B showed
no resolved improvement (bootstrap difference +0.43 frames, 95% CI
[-0.56, +1.46], spanning zero). The one remaining lever — a gesture-primed
arm that speculatively latches the regime on touch-down via a new
Kotlin→Rust signal — is more invasive and awaits a ruling; it is not
automatic.

**Evidence**: on-device A/B measurement (Android, Xiaomi 12) — see Why not fixed
above for the result.

---

### `cam-tail-p90-residual` — shape-aware sync tail does not clear its own bar (Android)

**Observed**: with the shape-aware sync tail active, the Xiaomi 12's steady-state
platform-view lead has a p90 of 1.78-1.98 display frames (~15-17 ms) against
a ≤0.25-frame p90 target that it does not clear. Median lead is 0 at every
measured velocity and overshoot is 0 everywhere.

**Applies to**: Android, acquire-bound devices in the same class as the Xiaomi 12
(the regime the tail activates for at all).

**Why accepted**: strictly better than the gate-only control on the same
device/session (p90 4.8-4.9 frames), zero overshoot at every velocity.
Accepted by the maintainers on 2026-07-26 partly because a camera preview is expected to
be placed on a static surface rather than inside a scroll view, which makes
this residual unobservable for that use case — but the sync machinery itself
stays load-bearing regardless: playground's own camera page deliberately
scrolls the slot to exercise the dispose/revive cycle, and `platform_views`
remains a scrolling Mode B testbed.

**Evidence**: on-device measurement (Xiaomi 12); closed and accepted per the maintainers'
ruling above.

---

### `cam-single-capture-in-flight` — a second concurrent capture times out (Android)

**Observed**: calling `take_picture` a second time on the same session while
an earlier call is still in flight (e.g. a double-tap on a capture button)
makes the **first** call time out at its 15 s deadline and report failure —
even though its photo may already have been written to disk. The second call
proceeds normally.

**Applies to**: Android only. Apple is immune — each capture is bound to its
own delegate and reply channel, so a second call cannot collide with the
first.

**Why not fixed**: deferred by an explicit maintainer ruling, not by oversight. Two fixes are on the table but the choice between them
is a contract decision, not a cleanup: (a) `take_picture` typed-refuses a
call while one is already in flight on the same session, or (b) an app
disables its capture control while a request is outstanding, leaving the
crate itself permissive.

**Evidence**: on-device reproduction (Android); deferral ruling recorded
above.

---

### `cam-bgra-apple-only` — `ImageFormat::Bgra` is refused on Android

**Observed**: requesting the image stream with `ImageFormat::Bgra` on
Android returns a typed error instead of frames.

**Applies to**: Android. CameraX's only packed-32-bit `ImageAnalysis` output
is `RGBA_8888` — there is no zero-copy BGRA buffer to hand over, and neither
mislabelling RGBA as BGRA nor a per-frame swizzle in the zero-copy path was
acceptable.

**Why accepted**: `Yuv420` is the format that actually exists zero-copy on
both platforms and is the cross-platform choice for an app that must run on
both; Flutter's own `camera` plugin draws the identical line (`bgra8888` is
iOS-only there too). `Bgra` remains available and works correctly on Apple.

**Evidence**: CameraX capability survey (Android); crate rustdoc/README
caveats.

---

### `barcode-qr-only-v1` — the barcode decoder decodes QR only

**Observed**: `frust_camera::barcode`'s public API is formats-general —
`BarcodeFormat` is `#[non_exhaustive]` and `Barcode`/`decode_luma`/
`decode_frame` all take a symbology filter — but the v1 engine behind that
API (`rqrr`, a pure-Rust QR detector/decoder) only ever produces
`BarcodeFormat::QrCode`. A white-on-black (inverted-contrast) QR code goes
undecoded: `rqrr` inherits this from its `quirc` lineage, the same
detector family MLKit's own barcode scanning draws its line at too.

**Applies to**: every platform this crate targets (the decoder is
platform-independent) — any app relying on a non-QR symbology, or on an
inverted-contrast QR code, gets an empty detection list rather than an
error.

**Why accepted**: the engine seam (`barcode::engine::BarcodeEngine`) is
private and swappable by design — a later engine adds symbology coverage
without changing `decode_luma`/`decode_frame`'s public signatures. Widening
past QR was deliberately deferred to a real second engine rather than
speculatively enumerating a wider `BarcodeFormat` vocabulary ahead of one.
A 1D symbology would also need live rotation tracking: QR is rotation-invariant,
but the Apple backend pins its stream to portrait 90°.

**Evidence**: `plugins/camera/src/barcode/engine.rs` (the private
`RqrrEngine`, `rqrr` the sole v1 implementation); `rqrr`'s own
`quirc`-derived detection algorithm; `plugins/camera/src/apple.rs`
(`STREAM_ROTATION_DEGREES`, hardcoded 90°).

---

### `barcode-analysis-resolution-unpinned` — Android barcode decode runs at whatever resolution CameraX's analysis stream picks

**Observed**: `CameraSession::start_barcode_stream` shares
`start_image_stream`'s underlying Android image stream, and that stream's
`openCamera(int lensFacing)` JNI contract carries no resolution parameter
(see `cam-bgra-apple-only` above, and `AndroidSession::open`'s own doc) — a
barcode decode on Android therefore runs against whatever `ImageAnalysis`
resolution CameraX's own default selection strategy picks, with no way for
an app to request a specific analysis resolution for detection accuracy or
decode cost.

**Applies to**: Android only. iOS's stream honors `Resolution` via
`AVCaptureSessionPreset`/`AVCaptureSession` config, so this is an
Android-specific gap in the same frozen-contract neighborhood as
`cam-bgra-apple-only`.

**Why accepted**: the `openCamera` JNI contract is a frozen v1 surface (see
the module doc's *Backends*); threading a resolution parameter through it is
a contract change, not a barcode-specific fix, and was out of scope for the
barcode feature itself.

**Evidence**: `plugins/camera/src/android.rs`'s `AndroidSession::open`
(`resolution` accepted and not forwarded — "the frozen `openCamera(int
lensFacing)` contract carries no resolution parameter").

---

### `barcode-nodup-full-rate-decode` — NoDuplicates and Unrestricted policies decode every frame

**Observed**: `DetectionPolicy::NoDuplicates` and `Unrestricted` decode every
delivered camera frame (~30 fps) by design — only `Throttled` gates decode on
time. The absence counter in `NoDuplicates` (`ABSENCE_FRAMES`, set to 30 frames)
counts processed frames and is calibrated to camera rate, making the counter's
cadence inseparable from the decode cadence.

**Applies to**: every platform this crate targets — the barcode stream's
`on_detect` callback fires at full camera rate for both policies, consuming CPU
on every frame.

**Why accepted**: both policies filter on the decode result — a duplicate can
only be recognised by decoding the frame — and `Throttled` is the time-gated
choice for an app that needs a lower decode rate. The CPU and thermal cost on
ARM mobile has not been benchmarked; playground's scan strip has a timing mode
that measures it.

**Future lever (documented option only, NOT implemented)**: advance the
absence counter on skipped/undecoded frames too, decoupling counter cadence
from decode cadence and allowing a slower-than-camera-rate re-arm delay.

**Evidence**: `plugins/camera/src/barcode/stream.rs`'s `should_decode`
(NoDuplicates and Unrestricted always return `true`).

---

### `clip-desktop-sensitivity-noop` — `set_text_sensitive` applies no sensitivity marking on desktop

**Observed**: `Clipboard::set_text_sensitive` on macOS, Linux, and Windows (the
`arboard`-backed desktop backend) is a plain alias for `set_text` — no OS-level
sensitivity marking is applied on any of the three desktop targets, unlike
Android (`ClipDescription.EXTRA_IS_SENSITIVE`, API 33+) and iOS
(`UIPasteboard.setItems(_:options:)` with `localOnly`).

**Applies to**: Desktop only (macOS, Linux, Windows); mobile already applies
its own marking (see `plugins/clipboard/src/android.rs`/`apple.rs`).

**Why not fixed**: unlike Linux (X11/Wayland have no equivalent mechanism at
all), Windows and macOS each document a real primitive `arboard` doesn't
expose. Windows: the clipboard-format trio `CanIncludeInClipboardHistory`,
`CanUploadToCloudClipboard`, `ExcludeClipboardContentFromMonitorProcessing`
(suppresses Win+V clipboard history and Cloud Clipboard sync for a format
marked with them). macOS: the community-convention format
`org.nspasteboard.ConcealedType` (no first-party API; an opt-in convention a
number of pasteboard-aware apps honor). Both require writing a custom
clipboard format rather than plain text, which `arboard`'s API doesn't expose
— reaching Windows' trio in particular would need `clipboard-win`'s raw
format surface instead of (or alongside) `arboard`. Deliberately deferred,
not attempted, to keep the desktop backend on one dependency.

**Evidence**: Windows Clipboard History/Cloud Clipboard format documentation;
`org.nspasteboard.org`'s `ConcealedType` convention; `arboard`'s public API
surveyed for a custom-format write (none). See `plugins/clipboard/README.md`'s `## 2.
Security` section for the crate-level writeup.

---

### `android-clipboard-read-focus-gate-and-toast` — an Android paste needs window focus, and a cross-app read raises a system toast

**Observed**: the Android shell drains the framework's paste request each
frame and reads the primary clip through `ClipboardManager`, inheriting two
platform behaviours it does not try to hide. (1) **The focus gate (Android
10+)**: `getPrimaryClip` returns nothing unless the app currently has input
focus, so a paste attempted while the window is unfocused yields nothing and
is deliberately indistinguishable here from an empty clipboard — no error
reaches the tree or the user. (2) **The read toast (Android 12+)**: the
system shows an "<app> pasted from <app>" toast the first time the app reads
another app's clip. The emptiness check runs before the real read precisely
because `hasPrimaryClip` raises no toast, so only a read a user actually
asked for ever crosses that line. A `SecurityException` from a device policy
or profile restriction is caught and logged: the paste does not happen and
the frame loop survives. The write side has its own platform note — Android
13+ shows the system's own "copied" confirmation, which nothing here
suppresses or duplicates. (2026-09-12)

**Applies to**: Android only, every field the framework's `EditCommand::Paste`
route reaches — the framework selection toolbar's Paste, a hardware `Ctrl+V`,
and `performContextMenuAction`. iOS is unaffected (its reads go through the
system edit menu); desktop and web have their own gates.

**Why accepted**: both are platform contracts rather than frust behaviour, and
neither has a route around it that keeps the read honest — focus is a
precondition the OS enforces, and the toast is the OS telling the user what
the app just did. Reporting a null read as an error would put a failure in
front of the user for the ordinary case of an empty clipboard.

**Evidence**:
`crates/frust-shell-android/platform/android/frust-embedding/src/main/kotlin/dev/frust/FrustSurfaceView.kt`'s
`readClipboardText` doc comment, which names all three behaviours in place,
and its `writeClipboardText` neighbour for the copy confirmation.

**Trigger for removal**: none — this describes the platform, and is kept so a
paste that silently does nothing is diagnosable rather than mysterious. The
behaviours themselves are read from the platform contract and from the shipped
source, not from an on-device observation of each one. A debug build of
`examples/glyph-catalog` was run on a Pixel 5 (redfin) on 2026-09-12 and the
operator reported the clipboard legs working; the APK was confirmed to carry
this code (`nativeFocusGeneration`, `readClipboardTextAsync` and
`clipboardResolutionEpoch` all present in the shipped dex and arm64-v8a `.so`,
against positive controls). That run is not enumerated leg-by-leg here, and in
particular did not confirm the URI-backed path — see
`android-uri-clipboard-paste-is-async-and-may-be-dropped`.

---

### `android-uri-clipboard-paste-is-async-and-may-be-dropped` — a paste from a URI-backed clip resolves off the UI thread and is discarded if the focus session moved or it took too long

**Observed**: `ClipData.Item.coerceToText` performs a synchronous
`ContentResolver` round-trip into the clip owner's process when the clip is
URI-backed (a photo, file, or contact copied from another app), so the Android
shell resolves that case on a background thread rather than blocking the frame
loop on another app's `ContentProvider`. A clip that already carries text —
the overwhelmingly common case, including everything frust itself writes — is
still read and pasted synchronously in the same event pass. The asynchronous
half is dispatched only if the framework's focus session is still the one that
asked for it, the native handle is still live, the view has not torn down, and
the answer arrived within two seconds. Any of those failing discards the paste
silently. (2026-09-13)

**Applies to**: Android only, and only a primary clip whose first item has a
`Uri` and no direct text.

**Why accepted**: `EditCommand::Paste` is focus-routed and carries no identity
of its own, so a late answer has to be checked against something, and a paste
landing in the wrong field cannot be taken back. What it is checked against is
the framework's focus *session identity* (`RenderRoot::focus_epoch`, which
moves on every honoured focus claim and every session release, by no fixed
step: a claim moves it one step, a session ending inside a dispatch two, and
a Tab traversal grant, which releases and then claims, two),
so the check is an identity rather than a proxy for one: focus cannot move from one field to
another without moving it, whether or not the field taking focus publishes an
IME surface, and whether or not two fields publish structurally identical
state.

The residue is what remains once misdelivery is closed: **a paste can still be
dropped**, and silently.

It is conservative in one narrow way: any press that re-claims the field's
focus session while the provider is still answering is read as a new session
and discards the paste. Tapping back into the field that already had focus is
one such press. So is a verb taken from the selection toolbar, which re-claims
on the user's behalf so that a copy or a select-all leaves the field exactly as
focused as it found it — that re-claim is indistinguishable from any other, so
a *Copy* or *Select all* tapped during the wait drops the pending paste with
nothing behind it. (Tapping *Paste* again merely replaces one pending answer
with another.) That is the safe direction and costs at worst a paste the user
can ask for again, on the terms below — which are weaker while a provider is
hanging. Nothing else inside one session discards it: editing, moving the
caret, and the field being repositioned — a reflow, the soft keyboard
animating in, a programmatic scroll — all leave the session exactly where it
was, so the paste still lands, in the field that asked for it and at whatever
the caret has since become. A *touch* scroll is a press first, so it is the
press that decides there too: one landing in the field re-claims, one landing
on nothing focusable blurs, and the movement itself decides nothing either
way.

The two-second bound exists because `shutdownNow()` cannot interrupt a thread
already blocked inside another process, so a deadline is the only thing that
can stop a very late answer arriving as a surprise paste. What asking again
buys depends on where the paste was lost. A paste dropped by the arrival
checks — wrong session, past deadline, empty text — is one whose resolution
came back, and the record is released before those checks run, so the resolver
is free and the next press submits a read of its own. While a provider is
still hanging, the next press is neither queued behind that read nor already
past its own deadline when it runs: one resolution is outstanding at a time,
so the press re-points the read already out at itself, replacing the session
that answer is addressed to and re-stamping the deadline from its own press,
and lands if the provider returns inside those fresh two seconds. What it does
not get is a read of its own — the single resolver thread stays parked in the
call that hung it, unreachable until that provider returns or a teardown
(`onPause`, `surfaceDestroyed`, `onDestroy`) discards the executor.

Re-pointing costs staleness: a folded press is answered with the item the
outstanding read captured, so a clip replaced during the wait pastes the one
that was on the clipboard when the first press asked.

**Evidence**:
`crates/frust-shell-android/platform/android/frust-embedding/src/main/kotlin/dev/frust/FrustSurfaceView.kt`'s
`readClipboardText`/`readClipboardTextAsync` doc comments and
`CLIPBOARD_RESOLUTION_TIMEOUT_MS`; the identity it compares crosses JNI as
`nativeFocusEpoch` (`crates/frust-shell-android/src/jni_glue.rs`) from
`AppTree::focus_epoch`. What that counter does where the older surface-change
counter beside it stands still is pinned by `crates/frust-core/src/app/tests/focus_ime.rs`'s
`focus_epoch_moves_when_focus_crosses_two_fields_publishing_alike`,
`focus_epoch_moves_when_the_field_taking_focus_publishes_nothing`, and
`focus_epoch_ignores_an_edit_inside_one_session`, each of which asserts what
*both* counters did at the same moment.

**Trigger for removal**: a resolver that cannot starve — half met. A retry
cannot be starved behind a read that will not drain, but the one resolver
thread can still be parked by a hung provider until it returns or a teardown
discards the executor, so removing this entry needs a resolution path that can
reclaim or replace that thread — a per-request cancel, or a bounded pool —
plus a device run that actually watches a URI-backed paste land. The
misdelivery half of this entry's earlier trigger is done: the per-session
identity it asked for is exposed through `AppTree` and is what the guard
compares.

**NOT DEVICE-VERIFIED.** The 2026-09-12 Pixel 5 run exercised the clipboard
legs but not this path: reaching it needs a clip whose first item has a `Uri`
and no direct text — a photo, file or contact copied from another app, not
text. Everything specific to this entry (the off-thread resolve, the
focus-session guard, the two-second deadline) is therefore argued from the
call graph and from the desktop shell's identical worker-thread precedent, and
has never been watched happen. That run predates `nativeFocusEpoch`, so it
carries no evidence about the export the guard now depends on either: this
entry currently has **no** hardware evidence behind any part of it, and a
device run should confirm the export resolves before reading anything else
from the behaviour.

---

### `pbxproj-id-budget` — Xcode object-id minting is capped at 255 ids per prefix

**Observed**: the scheme frust mints new Xcode project object ids under
(`ABCDABCDABCDABCDABCD00NN`) allocates a two-hex-digit suffix — 255 ids,
shared across **every** plugin's contributions in one generated app. Running
out fails a plugin apply loudly (a named error) rather than silently
corrupting or overwriting an id.

**Applies to**: iOS app scaffolding via `frust create`/`frust tui`'s Add
Plugin flow, specifically any `Contribution::SwiftPackageRef` (the six-site
pbxproj applier a plugin's local Swift package reference goes through) — a
project accumulating many plugin Swift packages over its lifetime.

**Why accepted**: ample for the app template's own ids (~30) plus a handful
of plugins in realistic use; failing loudly and leaving the file untouched
was chosen over silently guessing at a wider scheme.

**Evidence**: flagged during the camera plugin's implementation and review;
documented as a known constraint, not a regression.

---

### `ime-ios-content-type-unverified` — iOS secure-entry IME path is device-unverified

**Observed**: `FrustViewController.swift`'s IME reconcile path — the
content-type application (suppressing the QuickType
suggestion bar and keyboard learning on a `"password"`-classified field),
the **mirror re-seed/reconcile** that keeps `FrustView.mirror` from carrying
a previous field's text across a focus move (`syncImeFocus`'s `seedMirror`
on the content-type branch and `FrustView.reconcileMirror(to:)` on the
steady-active branch), and the **per-frame `syncImeFocus()` call from
`renderFrame`** that makes non-touch focus moves (Return-to-next-field,
programmatic focus) reach any of it — has been run on a device only in
part. `cargo` cannot compile Swift, so no ordinary workspace gate touches
this path. One device run has occurred (2026-11-12, iPhone 13 mini, iOS
27.2, the playground built with Xcode 27.0 from `355d6f1a`, a third-party
keyboard active): the path builds, links and runs, and typing, autocorrect
and dictation reach the field. That run had no password-classified field,
no page with two fields, and no Return-to-next-field move, so it verified
none of the three behaviours named above in full: the password traits were
not exercised at all, the mirror re-seed was seen only across a page change
with a blur in between (the second field held only its own text), and the
non-touch focus move was not exercised.
A second run (2026-11-12, same phone, built from `f36a8cbd`, the Apple
keyboard, XCUITest-driven) used a page with two fields and a password
field: text typed in one field and then one character in the next left the
second holding only that character, with no blur in between; on the
password field typed text is drawn as dots, Dictate is disabled and the
keyboard is left out of screenshots. Still not seen: whether the suggestion
bar is empty on the password field, and any non-touch focus move.
A separate channel was open on iOS: the view that is the `UITextInput`
responder reported the focused field's real text as its accessibility value,
a password field's included, although frust's own node for the field was
masked (seen on the phone, 2026-11-12). `FrustView.swift` now overrides
`accessibilityValue` (nil) and drops the keyboard-key trait, so the view
reports no text. The fix is in; it is unverified on a device until an
accessibility snapshot taken after it is recorded.

**Also covers**: `syncImeFocus`'s per-frame
`becomeFirstResponder()` retry is **bounded** (`imeFocusSatisfied`)
rather than fighting an intentional UIKit-originated resign (user swipe-
dismiss, a sibling native control taking first responder) forever, because
nothing on the Swift side can observe *why* first responder was resigned.
A user touch on the Frust surface (`onTouch`, user-initiated) re-arms the
bound — **but only while the surface is not already first responder**.
That is what lets a user bring back a keyboard they dismissed
themselves by re-tapping the field, since re-tapping an already-active field
changes neither Rust's `active` nor `contentType` signal. The
not-already-first-responder half is load-bearing: `imeFocusSatisfied` is set
back to `true` only in the `!isFirstResponder` branch, so re-arming during a
touch inside a *still-focused* field would leave it dangling at `false` and
reassert the keyboard on the next UIKit-originated dismissal — reopening the
very pop-back the bound exists to prevent. A touch inside a Mode B hosted slot never reaches `onTouch`
(`FrustView.hitTest` returns `nil` there), so a sibling native control
retaining first responder is unaffected by the per-frame tick. This
mechanism is Swift-only. The 2026-11-12 device run saw one part of it: a
keyboard dismissed by a tap outside the field came back on a re-tap of the
field. Swipe-dismiss, the stay-down half and the Mode B sibling case were
not run.

**Residual limitation, accepted** — the not-already-first-responder
gating does **not** remove it: when a Mode B sibling holds first responder,
`forgeView.isFirstResponder` is already `false`, so the
not-already-first-responder condition is satisfied and a touch **elsewhere**
on the Frust surface (e.g. scrolling non-editable content, outside the
sibling's interactive slot) still re-arms the bound. Rust has no way to learn the sibling took first
responder, so it still reports its own field as active, and the reconciler
will attempt to take focus back. This is inherent to the "any surface touch
re-arms" design, not an oversight; it is strictly better than the unbounded
per-frame fight it replaces, and it only manifests when the Mode B
native-widgets path is in play.

**Applies to**: iOS only — the entire `FrustEmbedding` IME surface
(`FrustViewController.swift`, `FrustView.swift`, `FrustTextInput.swift`).
Android's mirror path (`FrustSurfaceView.pollImeAfterDispatch` +
`FrustInputConnection.reconcileTo`, the model this path follows
property-for-property) is likewise device-unverified, but is a materially different code path (`EditorInfo`/
`restartInput`/`Editable` vs. UIKit `UITextInputTraits`/resign-become/
`NSMutableString`) and this entry makes no claim about it. Android's
`applyImeContentType` inputType/imeOptions flags have cleared `compileDebugKotlin`,
and the iOS Swift side has been built with `xcodebuild` and run once (above).
Neither is device verification of the content-type behaviour.

**Why not closed**: the only proof is on-device verification, and the one
run so far did not reach a password field. **The
leak (the accessibility channel above included) is therefore not confirmed closed on any platform** — the Rust-side
wire encoding (`content_type_wire`) has unit coverage, but nothing has
exercised the UIKit trait application for a password field, the mirror
re-seed on a direct field-to-field move, the per-frame reconcile on a
non-touch focus move, or the swipe-dismiss half of the bounded retry
against a real keyboard. The
per-frame reconcile in particular has an unmeasured cost (one
`frust_ime_state_json` FFI call plus one `JSONSerialization` parse per
`CADisplayLink` tick, matching Android's per-frame `nativeImeState` budget)
and an unobserved interaction with UIKit autocorrect/composition, both
reasoned about but not instrumented. Treat the current implementation as
the best-reasoned fix available, not as a verified fix.

**Device-gate observables, for whoever runs it** — five separate runs:
1. *Traits across a focus move.* Focus a `"normal"` field, type a few
   characters, then move focus directly to an obscured (`"password"`)
   field **without dismissing the keyboard in between**. Confirm (a) no
   QuickType suggestion bar appears while typing into the second field, and
   (b) nothing typed there is echoed anywhere (suggestion strip, autofill
   preview).
2. *Mirror staleness.* Same two-field form, both fields **normal** so
   no content-type change fires. Type `hello` into A, move focus to empty
   field B without dismissing the keyboard, type one character. B must
   contain exactly that one character — **not** `hello` + the character.
   Repeat with the second field obscured to cover the content-type branch's
   re-seed ordering.
3. *Non-touch trigger.* Same as (1) and (2), but move focus with the
   keyboard's **Return/Next key** rather than a tap, so no touch reaches the
   surface. Both observables must still hold.
4. *Bounded retry + touch re-arm.* Focus field A, let the keyboard
   appear, then swipe-dismiss it — the keyboard must **stay down** (no
   per-frame pop-back). Tap field A again — the keyboard must **reappear**.
   With a Mode B sibling native control focused, confirm the per-frame tick
   does not steal focus back from it. Dismiss field A by swipe, tap a
   *different non-editable* part of the surface, then tap field A again — it
   must still recover (exercises the clear-on-touch design point, and its
   residual limitation above).
5. *Touch inside an already-focused field, then dismiss.* Focus field
   A, then **tap or drag inside A while it still holds the keyboard** —
   caret repositioning or a selection drag. Now swipe-dismiss the keyboard
   with **no further touch on the Frust surface**. It must **stay down**.
   This is a distinct run from (4): the touch there happens *after* the
   dismissal, this one *before* it. The re-arm is gated on `!isFirstResponder`,
   but **that gating is reasoned, not device-measured** — this run is what
   would catch a regression.

**Evidence**: `swiftc -typecheck` exit 0; one partial device run, 2026-11-12, recorded in
[SHELLS_DEVELOPMENT.md](SHELLS_DEVELOPMENT.md) under the safe-area / keyboard / back gate. Runs 1 to 5
above have not been done as written: the playground lacks the two-field and password fixtures.

---

### `keyboard-avoidance-opt-in-vertical-scrollview-only` — keeping a focused field above the soft keyboard is opt-in and covers a vertical `ScrollView` only

**Observed**: a text field at the bottom of a scrolling page can stay under
the soft keyboard. The mechanism is in: a `safe_area(..).keyboard(true)`
holds the subtree's bottom edge at the top of the keyboard, and a vertical
`ScrollView` containing the focused field scrolls its caret into view
(`reveal_focused_caret`, on by default). Its device verification is
pending: the only phone runs (2026-11-12, Android 16 on a Pixel 4a and iOS
27.2 on an iPhone 13 mini) predate it and recorded the failure, a field at
the bottom of a scrolling page left under the keyboard.

**Applies to**: Android and iOS. Not covered: a `ListView`, a
`CustomScrollView`, horizontal scrolling, and an app that does not opt in
with `keyboard(true)` (without it the keyboard overlaps the page and the
reveal has no shrunken viewport to react to).

**Why not closed**: the mechanism has not been run on a device since it
merged. A field in an uncovered container needs the app to scroll it
explicitly (a `ScrollController`).

**Evidence**: the two 2026-11-12 device runs above; the `Caret reveal` module
docs in `scroll.rs` and the `Keyboard edge` docs in `safe_area.rs`.

---

### `focus-wrapper-erasure-swap-blind` — a type swap inside a wrapper view with `Element = Box<dyn Widget>` is invisible to the reconciler

**Observed**: `any(Wrap(any(view)))` — where `Wrap` is a custom view type with
`type Element = Box<dyn Widget>` that forwards `build`/`rebuild`/`teardown` to
an inner `AnyView` — produces a `ChildPod` whose recorded element has the
concrete type `Box<dyn Widget>` regardless of the inner view. When the inner
view swaps concrete type (e.g. `Text` → `Padding`), the swap-detection funnel
in `authoring::rebuild_child_tracked` compares the erased element's `TypeId`
before/after the rebuild and reports `swapped == false`: both are
`Box<dyn Widget>`, though the inner rebuild succeeded. The pod's `active` and
`focused` flags are neither cleared nor reported, and any live focus session
routed into the replaced inner widget continues routing into a fresh widget
that never claimed focus — exactly the failure the swap-detection arms exist to
prevent.

**Applies to**: any `ChildPod` built from a wrapper view that re-boxes an inner
`AnyView` under its own `Element = Box<dyn Widget>`. Pre-existing and in-tree
only: `ReorderableListView` (`crates/frust-widgets/src/drag/reorderable.rs`'s `View` impl,
`self.inner: AnyView<State>`). The single-erasure case — `any(Wrap(concrete_view))`
and every in-crate container's own child list — detects swaps correctly and is
unaffected. `any(any(view))` is CLOSED by idempotent erasure (`AnyView::new`
unwraps an `AnyView` argument), so the residual is only wrapper views.
`frust_widgets::Either` and `EitherOf3`..`EitherOf8` are not this shape: their
widgets are enums over statically typed arms (no `AnyView` field), and an arm
swap is reported to the enclosing reconciler (`BuildCtx::mark_element_replaced`,
collected by `rebuild_child_tracked` and `ComponentView::rebuild`), which drops
the pod's capture and focus link exactly like an `AnyView` swap, so they are
not subject to this limitation.

**Why not fixed**: closing it needs shared `TypeId` reporting through
`ErasedView` so nesting composes. Each reconciler today probes whatever boxed
element it holds; `ErasedView` would need to report the element's concrete
`TypeId` through the erasure boundary so `Wrap`'s element type is transparent to
the swap check. That is a `frust-core` trait-surface change landing on every
reconciler at once, and was deliberately not attempted.

**Evidence**: source inspection of `crates/frust-core/src/view.rs` (AnyView's
idempotent `new`), `crates/frust-widgets/src/drag/reorderable.rs` (the only
in-tree wrapper; the `inner: AnyView<State>` field, and the `type Element = Box<dyn Widget>` in its
`impl View<State> for ReorderableListView<State>` block),
`crates/frust-widgets/tests/double_erasure_swap.rs` (tripwire test
`wrapper_view_erasure_swap_blind`), `crates/frust-widgets/tests/wrapper_view_guard.rs`
(source-scan guard failing on any new in-tree view of this shape), and `crates/frust-widgets/src/authoring.rs` (swap detection).

---

### `tuple-children-positional-only` — a tuple child sequence reconciles by position only

**Observed**: `Column((a, b, c))`, `.children((a, b))` and every catalog list
parameter typed `impl ViewSeq<State, M>` erase a tuple's elements into the
container's `Vec<AnyView<State>>` in order, so reconciliation matches children
by slot (plus the element `TypeId` check). Moving an element from one slot to
another — or conditionally omitting one via an `Option` element, which drops
its slot — rebuilds the widgets after it rather than relocating them; their
retained state (scroll offset, text-input buffer, animation) is lost.

**Applies to**: any tuple, array or `Option` child sequence. Identity-based
reconciliation (`keyed(key, view)`) is a `Vec` affair and is unaffected.

**Why not fixed**: a tuple has no per-element key to carry; adding one would
mean a second sequence trait or a keyed tuple wrapper, and the documented
answer (`Vec` + `keyed()` for identity, tuples for fixed shapes) covers the
cases observed in the tree.

**Workaround**: put a dynamically ordered or optional child that must keep
state in a `Vec` with `keyed(..)`, or keep it last in the tuple so no sibling
shifts.

**Evidence**: `crates/frust-core/src/view.rs` (`ViewSeq` rustdoc and the
tuple impls), `crates/frust-widgets/tests/builders.rs` (tuple == `vec![any(..)]`
== fluent equivalence, `Option` slot drop).

---

### `viewseq-arity-12` — a flat tuple of more than 12 views is not a child sequence

**Observed**: `ViewSeq<State, M>` is implemented for tuples of 1 to 12
elements. A 13-element tuple fails to compile with the trait's
`#[diagnostic::on_unimplemented]` note naming the accepted shapes (a `View`, a
tuple of up to 12 views, an array, a `Vec`, an `Option`, or `views(iter)`);
nesting — `((a, b, c), (d, ..))` — flattens in order and has no limit.

**Applies to**: hand-written tuple children and the codemod's rule T6, which
leaves a fully erased list of more than 12 elements as a `vec![any(..), ..]`
(listed as `T6-arity`, not rewritten). Two such lists exist in
`examples/shadcn-demo` (`pages/primitives.rs`, `pages/data_table.rs`).

**Why not fixed**: the impls are macro-generated per arity; 12 matches the
standard library's tuple-trait convention and keeps compile time flat, while
nesting already gives unbounded width.

**Workaround**: nest tuples, or use a `Vec<V>` when the elements share a type.

**Evidence**: `crates/frust-core/src/view.rs` (`compile_fail,E0277` doc test
for the 13-tuple), `scripts/codemod/frust_any_codemod.py` (`T6-arity` bucket).

---

### `keyed-list-all-or-nothing-debug-only` — a mixed keyed/unkeyed list is caught by a `debug_assert` only, and the erasure codemod skips some sites

**Observed**: a `FlexView` / `column()` / `row()` child list must be all-keyed
or all-unkeyed. Mixing `.keyed(..)` with `.child(..)` / `.flex(..)` is detected
by a `debug_assert!` on rebuild only
(`crates/frust-widgets/src/authoring.rs`'s `rebuild_children` documents the tripwire; like the
`debug_assert` noted in `list_view.rs`'s module doc it is inert in release builds), so in a release build a mixed list silently falls back to
positional reconciliation: keyed children lose identity across a reorder and
are rebuilt in place rather than relocated, with no diagnostic. The same
applies to a duplicate key set.

**Applies to**: any `FlexView` child list built with a mix of keyed and unkeyed
children, in release builds only. All-keyed and all-unkeyed lists are
unaffected. The erasure codemod also has documented blind spots: it skips,
with a note, a site where a local binding or fn shadows a builder name, and a
mixed keyed list; and a closure parameter typed `AnyView` whose `any()` call
the tool would drop is not rewritten safely, which the compiler catches as a
type error rather than a silent change.

**Why not fixed**: the rule is a documented contract (CODE_STANDARDS.md's
"Keyed lists are all-or-nothing, and keys must be unique": a mixed or
duplicate key set `debug_assert!`s and falls back to positional matching in
release, never panicking live), chosen so a live app never panics on a list
shape error. Turning it into a compile-time or always-on check needs a
typestate on the builder or a release-mode diagnostic channel, neither of
which is in scope. The codemod's skipped sites are reported (exit status 2) and
fixed by hand, and its one silent-risk case is compiler-caught.

**Evidence**: `docs/WIDGETS_CODE_STANDARDS.md` (keyed-list rule),
`crates/frust-widgets/src/authoring.rs` (`rebuild_children`'s `debug_assert`), and
`scripts/codemod/frust_any_codemod.py` (its skipped sites).

---

### `focus-ime-edge-paced-deferral` — a focus/IME edge on a paced-only tick waits for the loop's next frame

**Observed**: `FrameInputs::focus_or_ime_changed` is deliberately **not** an
`is_paced_only_frame` disqualifier (every other wake input is). So when a
focus/IME edge — focus gained or lost, an IME surface published or released —
lands on a tick whose *only* other dirtiness is a paced
`TickClass::CosmeticLoop` frame request, the edge is absorbed into the pacing
decision instead of forcing the frame immediately: the repaint it would have
caused rides along with the loop's next paced frame. The deferral is bounded by
one `MotionScheme::cosmetic_loop_rate` interval — 33 ms at the 30 Hz framework
default, and at most **100 ms** at `CosmeticLoopRate::FLOOR_HZ` (10 Hz), the
lowest rate a theme can express (the constructor clamps up to the floor, so no
slower value is representable). Per-request paced intervals — slower cadences
named by `PaintCtx::request_frame_paced_at(Duration)` (e.g. a 500 ms caret
blink) — never widen this bound: `FrameGate::decide_paced` deliberately tightens
any tick carrying `focus_or_ime_changed` back to the theme's own cap, ignoring
longer per-request intervals, so a focus/IME transition never waits for a slow
loop's cadence.

**Applies to**: Android and iOS — the two shells that feed `FrameInputs` into
`FrameGate::decide_paced`. Desktop is unaffected: it runs no skip gate and its
own paced wake (`app_handler`'s `next_paced_wake`) never consults a focus/IME
input. Within those two shells it needs a paced decorative loop to be running
with nothing else dirty at the instant of the edge; any concurrent
input/signal/change-flag/transition dirtiness disqualifies pacing and the frame
runs immediately.

**Why accepted**: the alternative is what this replaced. `focus_or_ime_changed`
was a *level* input (`is_focus_active || ime_state().is_some()`) and a pacing
disqualifier, which meant any screen merely *holding* focus rendered every
vsync for the whole session — measured at 62-120 fps on a static screen whose
only live input was focus (Xiaomi 12) — and made caret pacing structurally
unreachable, since a blinking caret in a focused field is precisely the
cosmetic loop the gate exists to throttle. What is deferred is only a
*cosmetic* repaint: the platform's own IME reconcile polls the published Rust
state on its own cadence (Kotlin `doFrame` / Swift `renderFrame`), independent
of whether Rust produced a frame, and every IME event the platform delivers
also trips `events_since_last_frame`, which **is** a disqualifier — so the
keyboard's view of the session is never the thing being delayed.

**Evidence**: the contracts and device measurement recorded on
`FrameInputs::focus_or_ime_changed` / `FrameInputs::is_paced_only_frame`
(`crates/frust-shell-common/src/frame_gate.rs`, Xiaomi 12); the 100 ms bound
from `CosmeticLoopRate::FLOOR_HZ` (`crates/frust-theme/src/motion.rs`).

---

### `paced-starvation-min-lattice` — a fast paced loop repaints a slow loop at the fast rate

**Observed**: multiple paced frame requests in the same paint pass fold to a
MIN-lattice — the tightest interval wins. When a 30 Hz decorative shimmer runs
beside a 2 Hz caret blink, the caret repaints at 30 Hz for as long as the
shimmer runs. The caret sees 15 repaints for every frame it could display, all
visually indistinguishable from its requested 2 Hz cadence and bearing no
visible glitch or missed frame.

**Why by design**: the core-side MIN fold guarantees a *slow* request can never
starve a fast one — every paced requester is repainted at least as often as it
asked. A fast loop's tighter interval sets the frame rate for the whole tick,
and slower cadences riding along cost no frame the fast loop wasn't already
forcing. Motion that genuinely must run every vsync is a `TickClass::Transition`
request, not paced at all.

**Applies to**: Android, iOS, and desktop — all three shells support paced frame
requests. The MIN-lattice fold happens core-side
(`PaintCtx::request_frame_paced_at`, crates/frust-core/src/widget.rs), before
any shell sees the aggregated `PaintOutcome::paced_interval`, so the starvation
shape is identical everywhere — only the resolution mechanism differs. Android
and iOS resolve the requested interval through the frame gate's pre-paint skip
decision (`FrameGate::decide_paced` / `FramePacing::effective_interval`,
crates/frust-shell-common/src/frame_gate.rs); desktop resolves it post-paint
through delayed-redraw scheduling in `next_paced_wake`
(crates/frust-shell-desktop/src/paced_wake.rs), the identical
`max(cap, requested)` resolution, and pacing is on by default there too.

**Bound**: the repainting at the fast rate lasts exactly as long as the tighter
paced loop is active. A static screen with a 2 Hz caret and no concurrent
motion reverts to 2 Hz caret repaints.

**Evidence**: the MIN-lattice contract is stated in
`crates/frust-core/src/widget.rs`, `PaintCtx::request_frame_paced_at` rustdoc
(the two contracts on `request_frame_paced_at`, especially "MIN-lattice
aggregation" and "a *slow* request can never starve a fast one"), and resolved
by the shell at `crates/frust-shell-common/src/frame_gate.rs`,
`FramePacing::effective_interval` (the `effective_interval` method and its doc,
especially the "MIN fold already happened in core" bullet) and, on desktop, at
`crates/frust-shell-desktop/src/paced_wake.rs`, `next_paced_wake` (the same
`max(cap, requested)` rule, delayed-redraw side).

---

### `tui-raw-mode-signing-scrub` — plaintext signing scrub unavailable from TUI session keyboard signal

**Observed**: a TUI session keeps the terminal in raw mode (via `cfmakeraw`,
clearing termios `ISIG`), so keyboard Ctrl-C is delivered as an input byte via
crossterm — it never raises SIGINT. When `frust tui` launches an Android
**release** build (via `frust-drive::android_build::signing::write_resolved` →
`interrupt::install()`), the signal handler installs without error, but is
inert for keyboard Ctrl-C. The plaintext `android/.frust-signing.properties`
scrub cannot fire via the signal path; the `Drop` guard still covers normal
returns, so the gap is signal-path-only. Bare `frust` defaults into the TUI,
which makes Android **release** builds more likely to be launched from a raw-mode session — the
TUI's build launcher drives `frust-drive::android_build::build` in-process
(`spawn_blocking`).

`kill <pid>` (SIGTERM) and terminal-close (SIGHUP) still scrub correctly in a
TUI process — the same `ctrlc` `termination` feature + `frust-drive::interrupt`
three-signal handler that covers the CLI; only the raw-mode-delivered keyboard
Ctrl-C byte is inert. That signal path exits via `std::process::exit`, which
bypasses the TUI's `ratatui::restore()`/mouse-capture teardown, so a
SIGTERM'd TUI scrubs the plaintext file correctly but leaves the terminal in
raw mode/alternate screen afterward (a shell `reset` recovers it).

See also `tui-both-streams-tty-gate` below for a related, deliberate TTY-gate
narrowing.

**Applies to**: every Android release build launched from a TUI session.
Keyboard Ctrl-C neither scrubs nor cancels the build, so the plaintext window
lasts until the build returns on its own; actual leakage still requires
SIGKILL or a machine crash — unchanged from the CLI. Desktop and iOS are
unaffected — only the Android release path materialises plaintext signing
material (`android_build::signing::write_resolved`, reached from the
android_build and android_run release pipelines alone); desktop and iOS
builds write no secret file, so a missed signal has nothing to leave behind.
Builds from the CLI (`frust build --release`, `frust run --release`) continue
to scrub via the normal signal path regardless.

**Why accepted**: the raw-mode/ISIG interaction has always existed when a TUI
terminal is in raw mode, and closing it was deferred by the maintainers (2026-08-04). The
exposure is real: many new users encounter the TUI first. Mitigation exists
(signal-based scrub is one of two layers; the `Drop` guard still covers normal
completion; SIGTERM/SIGHUP still scrub as above; actual leakage still
requires SIGKILL or a machine crash, unchanged from the CLI), but closing the
signal-path gap requires either breaking raw-mode semantics or introducing
OS-specific platform code to detect the shell's raw-mode state and install an
alternative cleanup mechanism — both deferred. The TUI-specific delta:
keyboard Ctrl-C (a) raises no SIGINT, and (b) for build sessions cancels
nothing — Ctrl-C maps to a stop-session effect that is a no-op for builds
because the build launcher never registers with `Supervisor::sessions`, so the
user believes the build was cancelled while it (and the plaintext file's open
window) runs to completion. This false-cancellation UX plausibly steers a
user toward a forceful kill (closing the terminal, `kill -9`) as the next
step — the one documented leak vector; this is plausible, not measured.

**Evidence**: manual testing on Darwin and Linux (2026-08-04) confirmed crossterm's
raw-mode ISIG clearing and raw-mode SIGINT immunity, that the CLI path scrubs via
signal and the TUI path does not, and that Ctrl-C is a no-op for the build launcher.

---

### `tui-both-streams-tty-gate` — the TUI's default-entry TTY gate requires both stdin and stdout

**Observed**: bare `frust`'s TTY-gated default (see `docs/CLI_ARCHITECTURE.md`'s
Data Flow) requires **both** stdin and stdout to be real terminals before it
resolves to `Command::Tui`. stdout is the hard requirement — frames and help
text render there; stdin is a deliberate conservative narrowing.
`frust tui </dev/null` (stdin redirected, stdout a real terminal) previously
worked via crossterm's `/dev/tty` fallback and is now refused by design. A
pty-allocating automation harness (`ssh -t`, `docker run -it`, an `expect`
script) still passes the gate by design — its stdin *is* a terminal — so a
scripted bare `frust` invocation under one of those opens the interactive TUI
rather than exiting 2.

**Applies to**: any non-interactive or scripted invocation of bare `frust`
that allocates a pty for stdin (the automation cases above), and any script
relying on the old `frust tui </dev/null` behavior.

**Why accepted**: chosen as the conservative both-streams gate over attempting
to support mixed-stream scenarios (see `tui-raw-mode-signing-scrub` above for
the related signal-handling gap). There is
deliberately no environment escape valve: `FRUST_NO_TUI` was considered and
rejected by the maintainers (2026-08-04) — "frust --args for CLI and frust for TUI should
cover everything."

**Evidence**: `crates/frust-cli/src/main.rs`'s `default_command` (`stdin_tty && stdout_tty`).

---

### `iap-desktop-unavailable-v1` — every `frust-iap` call errors on desktop, by deferral not by absence

**Observed**: `Iap`'s desktop (macOS/Linux/Windows) backend reports
`IapError::NotAvailable(Unavailability::UnsupportedPlatform)` for every call —
there is no in-app-purchase backend on any desktop target in v1.

**Applies to**: macOS, Linux, and Windows, but not uniformly in nature. macOS
ships the Mac App Store's own StoreKit and Windows ships the Microsoft
Store's API, so a real desktop backend on either is a buildable future
addition kept out only by mobile-first v1 scope — a **deferral**. Linux
alone has no store API to route to at all, the same genuine capability gap
`frust-haptics`'s desktop arm documents.

**Why accepted**: mobile-first v1 scope. The desktop arm is dependency-free
by design (`plugins/iap/Cargo.toml`'s desktop-stanza comment) rather than a
stub around an unfinished backend, so an app's desktop preview build runs
its real store-code path and simply gets nothing back — the same fail-soft
shape every sibling plugin uses.

**Evidence**: `plugins/iap/src/desktop.rs`'s module doc; `plugins/iap/Cargo.toml`'s
desktop-stanza comment; IAP plugin implementation.

---

### `db-engine-dialect-divergence` — sqlite and turso diverge outside the shared conformance surface

**Observed**: two SQL-dialect gaps between `frust-database`'s engines are carved out of
the shared conformance suite as sqlite-only tests rather than both-engine assertions.
`CREATE INDEX` support is experimental and off-by-default upstream in turso `0.7.2`.
Under-supplied positional params (`?1`/`?2` with fewer bound values than placeholders)
error as `DatabaseError::Sql` on sqlite (`rusqlite`'s own client-side count check) but
silently bind the missing trailing placeholders as `NULL` and succeed on turso, which has
no equivalent check or parameter-count API to build one on.

**Applies to**: any app issuing `CREATE INDEX` or relying on param-count strictness
against a `turso`-backed `Database`; both are absent from the shared suite an app author
might otherwise assume covers the full SQL surface both engines accept.

**Why accepted**: neither gap is fixable from this crate's side without either
hand-parsing SQL for placeholders in the bridge (rejected — keeps the turso bridge "tiny
and boring") or waiting on upstream turso index/param-count work. The shared suite sticks
to the both-engine surface; each divergence is a named `sqlite_conformance`-only test
instead of a silently weakened shared assertion.

**Evidence**: `plugins/database/src/conformance.rs`'s module doc (`create_index`,
`param_count_mismatch` carve-outs); `plugins/database/README.md` §7.

---

### `db-cross-engine-interop-subset` — a shared file is portable only inside the WAL/unencrypted subset

**Observed**: a database file written by one engine and opened by the other stays
correct only if both sides stick to WAL journal mode, no encryption, and no
engine-specific pragma (`mvcc`, `cipher`, `hexkey`). Both backends enforce this
themselves (sqlite sets WAL at open; turso asserts it and refuses anything else), but
nothing stops an app from reaching around `Database` with raw SQL that breaks it.

**Applies to**: any app opening the same file with both `engine-sqlite` and
`engine-turso` builds, or sharing a file with an external SQLite tool that changes
journal mode or applies encryption.

**Why accepted**: this is the verified intersection of what both engines' SQLite builds
(bundled `rusqlite` 3.53.2; turso self-reports `sqlite_version()` 3.50.4) actually
support in common — not a narrower promise than necessary, but not a general SQLite-file
compatibility guarantee either.

**Evidence**: `plugins/database/src/conformance.rs`'s cross-engine round-trip tests and
module doc; `plugins/database/README.md` §5.3.

---

### `db-ui-thread-docs-only` — no typed guard against calling `Database` on the platform UI thread

**Observed**: unlike the boundary the `AsyncContext` error covers for turso, there is no
guard of any kind — typed or otherwise — against calling `Database::execute`/`query`/
`transaction` synchronously from the platform UI thread. It blocks the UI thread exactly
like any other blocking call would.

**Applies to**: both engines, on every platform. The documented mitigation is calling
through `frust_reactive::spawn_blocking`, never directly from `build()`/an event handler.

**Why accepted**: matches the `secure-storage` precedent already established in this
tier — a code guard would need FFI/thread-identity dependencies this pure-Rust plugin
deliberately avoids pulling in just to detect a caller mistake the docs already state.

**Evidence**: `plugins/database/src/lib.rs`'s module doc (Threading model); `plugins/database/README.md`'s Threading section.

---

### `db-asynccontext-partial-guard` — the turso `AsyncContext` guard cannot see a call from inside a spawned async task

**Observed**: `DatabaseError::AsyncContext` is reported only for the provable subset of
wrong-context callers — inside a tokio runtime's own `block_on` body, not inside a task
(`Handle::try_current().is_ok() && task::try_id().is_none()`). A call made from inside a
spawned async task looks identical to a `spawn_blocking` closure through every public
tokio API (same handle, same task id), so the guard cannot reject it. Such a call
degrades to a blocked worker thread for the duration of the query — never a panic, never
a deadlock, but also never a typed error.

**Applies to**: `engine-turso` builds only; an app that calls `Database` from inside
`tokio::spawn`-ed code rather than via `frust_reactive::spawn_blocking`.

**Why accepted**: tokio publishes no API that distinguishes "inside a spawned task" from
"inside a blocking-pool closure" from the caller's side — the measured four-context table
backing this is in the module doc. Detecting it would need parsing tokio's internal
thread-naming or task-local state, which is out of reach from a public dependency.

**Evidence**: `plugins/database/src/turso.rs`'s module doc (the four-context
`Handle::try_current()`/`task::try_id()`/`block_on` measurement table and the two guard
tests).

---

### `db-turso-bridge-serializes-handles` — turso handles do not get sqlite's read parallelism

**Observed**: `plugins/database/src/lib.rs`'s module doc documents an `engine-sqlite`
guarantee — "open multiple handles for concurrent readers" — that opening more than one
`Database` handle onto the same file buys real, wall-clock read parallelism, because
`rusqlite`'s bundled SQLite runs separate connections on separate OS threads.
`engine-turso` does not honor this: every `TursoConn`, however many `Database` handles an
app opens onto the same file, routes its operations through one process-wide,
single-threaded bridge (`plugins/database/src/turso.rs`'s bridge thread and `run()`).
Additional turso handles still buy correctness and cross-handle write visibility — each is
an independent connection, each sees the others' commits — but their calls
serialize/interleave on that one bridge thread rather than execute in parallel.

**Applies to**: `engine-turso` builds only. `engine-sqlite` is unaffected — its concurrent-
readers guarantee is real. An app that opens multiple turso handles expecting the
sqlite-style read-parallelism speedup gets correct results at sqlite's single-connection
throughput, not sqlite's multi-connection throughput.

**Why accepted**: the bridge is deliberately minimal by charter — one process-wide,
single-thread runtime, no worker pool, no timer, no socket of its own
(`turso.rs`'s module doc, *The bridge*) — kept that way so the async-to-sync seam stays a
small, auditable piece rather than growing its own scheduler. A per-handle or worker-pool
bridge (one bridge thread per `Database`, or a small thread pool dispatching turso futures
round-robin) would recover real parallelism, but is a materially bigger architectural
change — connection-to-thread affinity, pool sizing, and a second place this crate would
own concurrency policy — and was explicitly ruled out of v1 scope rather than overlooked.

**Evidence**: `plugins/database/src/turso.rs`'s bridge/`run()` (the single
`OnceLock<Result<Handle, String>>` and the one spawned bridge thread);
`plugins/database/README.md` §4's `engine-turso` qualifier beside the `tokio::join!`
example.

---

### `db-rollback-failure-residual` — a failed recovery ROLLBACK can leave a handle silently mid-transaction

**Observed**: `Database::transaction`'s `RollbackGuard` rolls the transaction back on
every abnormal exit (closure `Err`, failing `COMMIT`, panicking closure), but that
recovery `ROLLBACK` is itself issued best-effort (`let _ =`). If it also fails — an I/O
error mid-WAL-rollback, disk full — the connection is handed back with the transaction
still open and no taint recorded: the next `transaction()` on the handle fails at
`BEGIN`, and bare `execute`/`query` calls silently join the orphaned transaction, whose
writes are discarded when the handle drops.

**Applies to**: both engines; only reachable when a `ROLLBACK` statement fails
immediately after another failure on the same connection (a second-order fault).

**Why accepted**: the primary failure paths are closed — the
guard makes "no exit without a rollback attempt" structural, and the double-fault
window is narrow and requires storage-level failure. A `tainted`-handle flag with a
typed error on subsequent use is the known remedy if this residual is later promoted.
Apps needing robustness
against this class drop the handle on any `transaction` error and reopen (README §4a).

**Evidence**: `plugins/database/src/lib.rs` `RollbackGuard::drop` (best-effort
`ROLLBACK`), the qualified poison-policy doc on `lock_conn`, README §4a's closing
caveat.

---

### `tui-shimmer-ansi16-degrade` — the build/install phase-line shimmer degrades to flat+BOLD at Ansi16

**Observed**: `themed_shimmer_spans` (`crates/frust-tui/src/ui/anim/shimmer.rs`) sweeps a
real color ramp at TrueColor (a live RGB lerp) and at Xterm256 (`Theme::SHIMMER_RAMP_X256`,
a fixed 5-entry palette-index table bucketed per character), but at `ColorDepth::Ansi16`
there are only the theme's two named ANSI colors to choose from — too coarse a palette for
a convincing color sweep between them. The head-tracking motion still plays: characters
near the sweep head get `Modifier::BOLD` against a flat `theme.muted()` foreground
everywhere else, rather than any color change.

**Applies to**: any Ansi16-depth terminal (`ColorDepth::detect()`'s conservative floor for
an unrecognized `$TERM`, `TERM=dumb`, or no `TERM` at all) rendering the build/install
phase-line shimmer.

**Why accepted**: Ansi16 has no intermediate hues between the theme's muted and accent
tokens to sweep through — a real color ramp needs a palette this depth doesn't have.
BOLD-only motion is still a visible, cheap sweep at any depth, and keeps the degrade
decision inside `themed_shimmer_spans` itself (dispatched on `theme.depth()`) rather than
pushed out to call sites.

**Evidence**: `crates/frust-tui/src/ui/anim/shimmer.rs`'s `themed_shimmer_spans`/
`shimmer_spans_flat_bold`; `crates/frust-tui/src/ui/theme.rs`'s `ColorDepth`/
`ThemeColor::resolve`.

---

### `devtools-ios-physical-forward-deferred` — the devtools client can't reach a physical iOS device

**Observed**: `frust-drive`'s devtools client connects directly over localhost (desktop, and
iOS Simulator, which shares the host's loopback) and forwards through `adb` for a physical
Android device. There is no equivalent for a physical iOS device — no forwarding helper
exists, so a client cannot reach the in-app debug service on one.

**Applies to**: `frust drive`/`frust-tui` devtools sessions targeting a physical iOS device.
Desktop and both simulators/emulators are unaffected.

**Why accepted**: physical-iOS forwarding needs `usbmuxd` (or an equivalent), which was left
out of the first version by decision rather than discovered as a gap.

**Evidence**: `frust-drive` has no `usbmuxd` or `iproxy` path; its devtools client connects over
localhost or through `adb` only.

---

### `devtools-screenshot-not-supported-v1` — the devtools `screenshot` method is unimplemented

**Observed**: the protocol declares a `screenshot` method (`Capability::Screenshot`,
result `{ png_base64 }`), but `DevtoolsBackend::screenshot`'s default — which every shell's
backend implementation inherits — returns `BackendError::NotSupported`, which the wire
reports as `RpcError::NOT_SUPPORTED`.

**Applies to**: every shell in v1; no backend overrides the default.

**Why accepted**: v1 scoped the wire shape for a future capability without shipping the
capture path; the method and capability flag exist precisely so a client can detect support
per-app rather than guessing, once a backend does implement it.

**Evidence**: `crates/frust-devtools/src/backend.rs`'s `DevtoolsBackend::screenshot` default;
`crates/frust-devtools-protocol/src/method.rs`'s `screenshot` doc comment.

---

### `hotpatch-windows-fixed-base-no-aslr` — Windows x64 hot runs load the app and every patch DLL at fixed bases without ASLR

**Observed**: a debug hot run on an x64 MSVC target links the fat exe with `/DYNAMICBASE:NO
/BASE:0x10000000` and patch n's DLL with `/DYNAMICBASE:NO` at its own base, 0x20000000 +
(n-1)*0x10000000, so the app under a hot run and every patch it loads sit at predictable addresses
with ASLR off. Two constraints force it: the stub turns each base data symbol a patch references
into a COFF absolute symbol, which holds 32 bits, so base data must sit below 4 GiB; and the runtime
reads a Windows image's slide as its load address minus the in-memory `ImageBase`, which the loader
rewrites when it relocates an image, so the slide always reads 0 and only an image loaded at its
link-time base can be checked. A patch DLL the loader relocates anyway (its base already taken)
fails the patch-anchor check: the patch is refused and the app restarts. lld-link refuses
`/DYNAMICBASE:NO` on ARM machines (ARM64, ARM64EC, ARM64X, ARMNT), so an ARM line keeps ASLR and
its patches are refused: ARM64 Windows never hot-patches. Bases above 4 GiB (patch 15 on) were not
exercised.

**Applies to**: Windows debug hot runs only: the fixed-base flags are added only by a hot run's fat
link and its thin patch links, so a plain `frust run`/`frust build`, and every release build, keep
ASLR.

**Why accepted**: the exposure is a debug developer build on the developer's own machine, for the
length of one hot run, and a collision fails closed (restart, never a patch applied at the wrong
slide). Reading a relocated image's slide from its base-relocation data instead of `ImageBase`, and
resolving base data without 32-bit absolute symbols, are the reopen path.

**Evidence**: `crates/frust-drive/src/hotpatch/fat_link.rs` (`DYNAMIC_BASE_OFF`, `FAT_IMAGE_BASE`,
`PATCH_IMAGE_BASE_FIRST`, `PATCH_IMAGE_STRIDE`, `ASLR_ONLY_MACHINES`, `fixed_base_args_at`);
`crates/frust-drive/src/hotpatch/stub.rs` module doc; `crates/frust-hotpatch/src/anchor.rs`'s
Windows `image_containing` and `crates/frust-hotpatch/src/patch.rs`'s patch-slide check;
`examples/hotpatch-spike/RESULTS.md` `## Stage 3: Windows gate (H3-03)` (Known limits seen, row G's
base addresses: no relocation in 41 patch loads) and `### Stage 3 re-run (H3-03b)` (30 patched
runs).

---

### `hotpatch-windows-patch-file-acl` — on Windows the patch DLL lands with an inherited ACL, is loaded by path, and is never removed

**Observed**: the app-side hardening of a written patch is unix-only. On unix `write_patch_file`
creates the patch directory `0700` and each patch file `0600` (reset exactly after the umask), and
the `patch_file` hand-off checks owner, mode, size and SHA-256. On Windows each patch DLL is
written fresh (`create_new`, after removing any existing entry) to
`%LOCALAPPDATA%\frust-hotpatch\patch-<pid>-<id>.dll` with the ACL that per-user directory
inherits, closed, then `LoadLibrary`'d by path, with no deny-write share held across the
close-to-load window, so a process able to write that directory could swap the file before it
loads. The `patch_file` hand-off is not offered, so every patch travels as base64 `patch_chunk`s
even over loopback (~600 ms per 1.28 MB patch, load included). The app's removal after the load
fails for a loaded DLL (`Access is denied. (os error 5)`), so the copies accumulate: 45 files,
57,368,064 bytes after the Stage 3 gate, and nothing deletes them when the process exits.

**Applies to**: Windows debug hot runs only.

**Why accepted**: a default `%LOCALAPPDATA%` grants only the user, SYSTEM and Administrators, and a
process already running as the same user could attach a debugger to the app anyway, so the window
reaches nothing new unless `LOCALAPPDATA` is redirected somewhere with a looser ACL. Hardening the
written file and its close-to-load window, removing the accumulating copies and moving off the
chunk-only transport are follow-up work.

**Evidence**: `crates/frust-shell-common/src/devtools.rs` (`write_patch_file`'s `cfg(unix)` arms,
`HotState::attempt`, the unix-only `patch_file`); `crates/frust-paths/src/lib.rs`'s `cache_dir`
(`%LOCALAPPDATA%` on Windows); `examples/hotpatch-spike/RESULTS.md` `## Stage 3: Windows gate
(H3-03)`, Known limits seen (patch copies, transport).

---

### `hotpatch-windows-pdb-layout-align-blind` — the Windows layout gate admits an alignment-only type edit

**Observed**: on Windows the hot run's layout gate (`hotpatch::pdb_layout`) reads type layouts from
the PDB's CodeView type records, which state no alignment, so every entry records alignment 0 and
its hash covers only the record kind, size and field list. An edit that changes only a type's
alignment, such as adding `#[repr(align(16))]` to a struct whose size is already a multiple of 16
and whose field offsets do not move, leaves the entry unchanged and the patch is admitted, although
the patched code may then assume an alignment the running process's existing values do not have.
The DWARF gate on every other target hashes `DW_AT_alignment` where the DWARF states one, so it
sees the same edit.

**Applies to**: Windows (MSVC) hot runs only.

**Why accepted**: an alignment-only edit to a live type is rare in a hot-reload loop, every other
layout change (size, offsets, field types, variants) is still refused, and CodeView carries no
alignment for the gate to read. An alignment source beside the PDB is the reopen path.

**Evidence**: `crates/frust-drive/src/hotpatch/pdb_layout.rs` module doc (alignment 0) and
`crates/frust-drive/src/hotpatch/layout.rs` (`DW_AT_alignment` in the DWARF hash);
`examples/hotpatch-spike/RESULTS.md` `## Stage 3: Windows gate
(H3-03)`, Known limits seen (not exercised by the gate's rows).

---

### `tui-devtools-desktop-metrics-unavailable` — system-metrics sampling never starts on desktop or iOS

**Observed**: `frust-tui`'s DevTools System/Network tabs, and `frust-mcp`'s `metrics` tool, sample
process metrics for Android sessions only. `frust-drive::process::StreamHandle` never exposes a
spawned child's pid, and extending it is out of scope for either feature, so a desktop or iOS
session's identity (`MetricsIdentity` in `frust-tui`; `Session::metrics_sampling` in `frust-mcp`)
never resolves past "not Android" — `frust-tui`'s tabs render a permanent "sampling unavailable"
state, and `frust-mcp`'s `metrics` tool reports `system.available: false` with a reason, never a
zeroed reading, for the life of the session.

**Applies to**: desktop and iOS sessions in the DevTools System/Network tabs and in `frust-mcp`
sessions. Android sessions are unaffected — their identity resolves from lines the session's own
log already carries (`Launching {pkg}…` / `Streaming logs (pid {pid})`).

**Why accepted**: threading a pid out of `StreamHandle` is a `frust-drive` process-plumbing change
unrelated to the System/Network tabs feature itself; deferred as a named follow-up rather than
folded in here.

**Evidence**: `crates/frust-tui/src/engine/devtools.rs`'s `MetricsIdentity` doc (desktop honesty
note); `crates/frust-mcp/src/engine/metrics.rs`'s module doc ("Android only, and why");
desktop/iOS sampling is unavailable because the pid is not exposed.

---

### `metrics-macos-desktop-unimplemented` — `frust-drive::metrics`'s desktop collector is Linux-only

**Observed**: the desktop metrics collector reads `/proc`/`/sys` directly and only compiles a real
implementation for Linux; on every other desktop OS, macOS and Windows included, every fetch returns
`MetricsError::Unsupported` at runtime rather than failing to build.

**Applies to**: every non-Linux desktop session. Linux desktop (the dev/CI platform) and Android
are both fully supported.

**Why accepted**: a deliberate no-new-dependency decision — `sysinfo` was the obvious option and
was dropped to keep `frust-drive` free of the added dependency; a native source for macOS
(e.g. `host_statistics`/IOKit) or Windows is deferred.

**Evidence**: `crates/frust-drive/src/metrics/desktop.rs`'s module doc ("Linux-only... the macOS
gap is tracked as a `docs/LIMITATIONS.md` entry"); its `cfg(not(target_os = "linux"))` arm returns
`MetricsError::Unsupported`.

---

### `devtools-android-per-app-network-toggle` — the in-app service silently fails to bind when an app's network access is off

**Observed**: on Android (notably MIUI/HyperOS and other vendor skins with a per-app network
toggle), if an app's "network access" is disabled in App info, its uid is firewalled at the kernel
(eBPF owner-match) level. Every socket operation — **including a `127.0.0.1` loopback bind** —
returns `ECONNREFUSED (os error 111)`, so the devtools `Service::start` fails and no discovery
line is ever logged. The manifest's `INTERNET` permission still reads as *granted*: the vendor
toggle is a separate runtime layer that overrides it, and there is **no SELinux denial** to point
at. The symptom in the workbench is DevTools sitting on "waiting for a discovery line…" forever.

**Applies to**: any Android device whose per-app network access has been turned off for the app
under inspection; independent of build flavor and of the granted `INTERNET` permission.

**Why accepted**: it is a device/OS policy, not something the framework can or should override —
an app that the user has firewalled must stay firewalled. The service already logs the failure
(`frust-devtools: service did not start: <reason>`), and the TUI now surfaces that reason on the
DevTools screen (see below) instead of an eternal wait. The fix is operational: enable the app's
network access, then relaunch.

**Evidence**: reproduced on a Xiaomi 12 (2026-08-10) — `Service::start` returned `ECONNREFUSED`
and a bare `toybox nc` loopback listener as the app's uid failed identically while a shell-uid
listener succeeded, with no `avc: denied` for any socket/bind class; enabling network access made
the discovery line appear immediately. Surfacing path: `frust-devtools-protocol`'s
`FAILURE_PREFIX`/`parse_failure_line`, `frust-shell-common::devtools::start`'s failure log, and
`frust-tui`'s `DevtoolsState::start_error` (rendered on the Discovering screen).

---

### `mcp-screenshot-desktop-unsupported-v1` — the `screenshot` MCP tool has no desktop or iOS Simulator path

**Observed**: `frust-mcp`'s `screenshot` tool tries the app's own devtools `screenshot` capability
first, falls back to `adb screencap` for an Android session, and otherwise returns a clear
in-band refusal. No shell backend declares `Capability::Screenshot` yet (see
`devtools-screenshot-not-supported-v1`), so today the devtools path never fires and desktop/iOS
Simulator sessions have no fallback at all — only Android sessions can be screenshotted.

**Applies to**: `frust-mcp`'s `screenshot` tool for desktop and iOS Simulator sessions.

**Why accepted**: the root cause (`devtools-screenshot-not-supported-v1`) is out of scope for this
crate; the tool is written to pick up the devtools capability automatically once a backend
implements it, with no MCP-side change needed.

**Evidence**: `crates/frust-mcp/src/tools/diagnosis.rs`'s `screenshot` decision chain; the shared
root cause is `devtools-screenshot-not-supported-v1`.

---

### `mcp-server-unauthenticated-v1` — the MCP server has no authentication beyond the loopback bind

**Observed**: `frust-mcp` binds `127.0.0.1` only and relies on rmcp's default Host-header guard
against DNS rebinding, but the MCP protocol layer itself has no login, token, or capability check
— any local process that can reach the port can list, launch, drive, and stop sessions of the
server's configured project. There is no per-call project override: that argument was removed
after review, so a session can never be pointed at an arbitrary filesystem path.

**Applies to**: every `frust-mcp` session, on every platform.

**Why accepted**: a deliberate v1 stance matched to the threat model of a
loopback developer tool: the bind is not reachable off-host, and the devtools handshake token
still protects the app-side service itself. Server-side auth (e.g. a bearer token) is a named
follow-up, not a v1 requirement.

**Evidence**: `crates/frust-mcp/src/config.rs`'s `McpConfig` doc comment (bind address is never
configurable); `crates/frust-mcp/src/server.rs`'s module doc (Host-header guard, no other auth);
`crates/frust-mcp/src/engine.rs`'s `run_app` doc comment (no per-call project override).

---

### `mcp-no-headless-entry-point` — the MCP server has no headless/CI entry point

**Observed**: `frust-cli`'s `mcp` subcommand (and its `frust-drive`→`frust-mcp` dependency) was
removed; `frust_mcp::run`/`serve_embedded` remain library-only entry points, with `frust-tui`'s
embedded server as their only caller in this repo. Reaching an MCP server therefore means starting
an interactive `frust-tui` session and toggling its server on — there is no way to run one headless,
so CI or a remote/headless automation pipeline cannot attach an MCP agent to a Frust app.

**Applies to**: any workflow wanting MCP access to a Frust app outside an interactive `frust-tui`
session.

**Why accepted**: the v1 design chose one session world — the workbench is the single source of
truth for what is running, and a second headless engine driving the same project would either
diverge from it or race it. A standalone headless mode is a named follow-up, not a v1 requirement.

**Evidence**: `crates/frust-cli/src/commands/mod.rs` and `Cargo.toml` carry no `mcp` command or
`frust-mcp` dependency; `crates/frust-mcp/src/lib.rs`'s `run`/`serve_embedded` are `pub` with no
binary consumer in this repo besides `crates/frust-tui/src/runner.rs`.

---

### `mcp-embedded-client-registry-coarse` — connected-MCP-client tracking is coarse and can go stale

**Observed**: `frust-mcp`'s `ClientRegistry` (the TUI's MCP panel reads it live) tracks only
a count, a mint-order opaque `id`, and a connect time per session — no client name, version, or
capabilities. Disconnection has exactly one signal: the per-session `ClientGuard`'s `Drop`, fired
when rmcp tears the session down. The Streamable-HTTP transport exposes nothing lower-level to
build a better signal from, so a client that vanishes without a clean teardown (a killed process, a
network drop rmcp doesn't notice) leaves its entry — and the panel showing it as connected — until
the server itself stops.

**Applies to**: `frust-mcp`'s `ClientRegistry` in both `serve` and `serve_embedded` modes; the TUI's
MCP panel client list.

**Why accepted**: the transport offers no better signal to build on. Per-client name/version display is a named follow-up, gated on confirming rmcp actually exposes an
`Implementation` the registry could read.

**Evidence**: `crates/frust-mcp/src/clients.rs` module doc ("the guard's Drop is consequently the
*only* disconnect signal this crate has... a documented limitation, not a bug to chase here");
`crates/frust-tui/src/ui/views/mcp/mod.rs`.

---

### `mcp-embedded-devtools-unavailable` — embedded-mode driving/diagnosis tools needing a devtools client are always refused

**Observed**: `frust-tui`'s `TuiSessionBackend` (the embedded server's `SessionBackend`) always
reports a session's `devtools_client` as absent — the devtools socket lives inside
`supervise::DevtoolsBridge`'s own thread inside the workbench, with no shareable handle to give an
MCP tool call. Every driving tool (`tap`/`scroll`/`enter_text`) and every diagnosis tool needing a
live devtools request (`widget_tree`/`widget_props`/`find_widgets`; `screenshot`'s devtools-first
path) returns a typed in-band refusal rather than attempting the call. Tools reading data the
workbench already retains (`app_logs`, `list_sessions`, `run_app`/`stop_app`/`restart_app`,
`performance`, `metrics`) are unaffected.

**Applies to**: every MCP session run through `frust-tui`'s embedded server — not the standalone
`SessionEngine` backend, which owns its devtools connection directly.

**Why accepted**: handing the tool layer a foreign thread's socket would need its own lock/hop
protocol, or a second connection to the same app's devtools service; neither exists yet, and an
honest refusal is the interim contract rather than a silent no-op or an invented reading. Session
snapshots still report `devtools_port` deliberately (an agent may want it for its own tooling) —
this is safe because the retained log ring redacts the discovery line's handshake token at the push
edge (`SessionView::push_line_at`), so the token needed to actually use that port is unobtainable
via `app_logs`; only the port number itself is exposed.

**Evidence**: `crates/frust-tui/src/supervise/mcp_backend.rs` module doc ("What this backend
deliberately cannot do", `DEVTOOLS_OWNED_BY_WORKBENCH`); `crates/frust-tui/tests/mcp_embedded.rs`'s
`devtools_backed_tools_report_the_embedded_mode_refusal` test.

---

### `mcp-embedded-fast-rebind-can-fail` — toggling the embedded MCP server off then on quickly can transiently fail

**Observed**: stopping the embedded server cancels its `CancellationToken` and immediately clears
`AppState::mcp`, but the listener's actual close is asynchronous — it completes only when the
server task's own stopped report arrives. Toggling the server back on before that report lands can
race a bind against a socket the OS has not yet released, failing with a visible bind error
surfaced through `AppState::mcp_error`. A second toggle after the failure succeeds normally.

**Applies to**: `frust-tui`'s embedded MCP server, which always rebinds the same fixed port
(`DEFAULT_MCP_PORT`, 4848) rather than an ephemeral one.

**Why accepted**: blocking the toggle until the stop is provably complete would turn an otherwise
immediate action into a wait; the failure is visible and immediately retryable, so the workbench
favors responsiveness over a masked wait.

**Evidence**: `crates/frust-tui/src/engine/mod.rs`'s `Engine::stop_mcp` doc comment ("A caller that
must know the port is free again... waits for that message"); `crates/frust-tui/src/engine/update.rs`'s
`Message::McpStopped` handling (`mcp_error` retention).

---

### `tui-device-stop-app-termination-residual` — a device session's OS-level app termination is best-effort and physical-iOS-absent

**Observed**: stopping a device session's stream (the user's stop keypress, or MCP's `stop_app`/
`restart_app`) also dispatches a best-effort OS-level app termination on a tracked detached thread
— `adb shell am force-stop <package>` on Android, `xcrun simctl terminate <udid> <bundle_id>` on an
iOS Simulator. A physical iOS device has no termination call at all: `TerminationTarget` has no
variant for it (`devicectl` app termination is not implemented), so its stop remains stream-only —
the workbench's own view of the session goes to `Killed`/`Exited`, but the app itself is left
running on the device until the user closes it by hand.

**Applies to**: `frust-tui`'s `Supervisor` for every device session; physical-iOS sessions
specifically for the missing termination call.

**Why accepted**: `am force-stop`/`simctl terminate` cover the two platforms with a straightforward
CLI termination path; `devicectl`'s physical-device app-termination surface is a separate,
unresearched integration and a physical iOS device was already the workbench's least-verified
target (see `devtools-ios-physical-forward-deferred`). The termination call is best-effort by design
on every platform it exists for — a device that has gone away or an app that already exited are
normal outcomes of a stop, not failures to report.

**Evidence**: `crates/frust-tui/src/supervise/supervisor.rs`'s `TerminationTarget` enum and module
doc ("Stopping the app" section); `stopping_an_android_session_force_stops_the_app`,
`stopping_an_ios_simulator_session_terminates_the_app` tests in the same file.

---

### `mcp-stop-app-termination-in-flight` — `stop_app`/`restart_app` can return before the app is actually gone

**Observed**: `SessionBackend::stop_app`/`restart_app`'s "Blocking" describes how long the *call*
takes to return, not what has finished when it does. Both `frust-mcp`'s own `SessionEngine` and
`frust-tui`'s embedded `TuiSessionBackend` may reply once the session reaches its terminal state
while the best-effort OS-level app termination (`am force-stop`/`simctl terminate`) is still running
on its own thread. An agent that immediately re-queries device state (outside Frust's own tooling)
could observe the app as still present for a short window after the tool call returns.

**Applies to**: every `stop_app`/`restart_app` call through either `SessionBackend` implementation.

**Why accepted**: this is request semantics, not a race to fix. On `frust-mcp`'s own
`SessionEngine` the session is terminal by the time the call returns (its teardown is synchronous);
on the embedded `TuiSessionBackend` **neither** the session's terminal state **nor** the OS-level
termination is complete when the call returns — the stop request has merely been posted, the kill
issued best-effort, and the session's terminal state follows asynchronously through the workbench's
normal event path (`serve_command`'s own doc: a `stop_app` is a *request*). A `stop_app` reply's
snapshot can therefore still read `running`; an agent needing the terminal state must poll
`list_sessions`. Coupling either side would mean blocking the caller (or the workbench event loop)
on an unbounded `adb`/`simctl` call for no benefit `frust-mcp`'s own tools need today.

**Evidence**: `crates/frust-mcp/src/backend.rs`'s `SessionBackend::stop_app`/`restart_app` doc
comments ("Blocking' describes how long the call may take, not what has finished when it returns").

---

### `tui-mcp-sessions-tab-uncapped` — session tabs have no automatic cap, and an evicted MCP record's tab stays until closed by hand

**Observed**: `frust-tui`'s `AppState::sessions` map — one entry per session tab — has no automatic
eviction: a tab, finished or killed, stays parked until a user closes it (`X`, the palette's "Close
tab" or the tab's context menu; closing a live session stops it first). MCP-driven growth is bounded
one layer down: `supervise::mcp_backend::McpSessionRecords` caps its own retained launch records at
`MCP_RECORD_CAP` (oldest-terminal evicted first, live never evicted) and `run_app`/`restart_app`
refuse once that cap of live MCP sessions is reached — but the *tab* an evicted record backed is not
removed with it, so it stays in the UI, un-restartable, until closed by hand: `restart_app` on its id
reports `EmbeddedError::NoSuchSession`, diverging from `frust-mcp`'s own reference `SessionEngine`,
which evicts its terminal-session record and its tab-equivalent state together. A keyboard restart
(`R` / palette 'Restart session') of such a tab surfaces a Warn toast ('no launch record for this
session — relaunch it with o'). `runner::apply_effect` reconciles `McpSessionRecords` with the engine
after every `update()` (`observe`/`mark_closed`), so a record whose tab was closed or replaced by a
restart counts as finished and is evictable.

**Applies to**: `frust-tui`'s `AppState::sessions` for the whole session lifetime; the MCP-launched
subset's *record* (not tab) is capped as described above.

**Why accepted**: closing a tab a user might still want to scroll back through is a UX decision, not
a memory-safety one, so nothing closes one automatically. Growth in the human-driven case is bounded
by how many sessions a person opens in one sitting, and the records-layer cap covers the unattended
MCP path.

**Evidence**: `crates/frust-tui/src/engine/update.rs`'s `close_tab`;
`crates/frust-tui/src/supervise/mcp_backend.rs` module doc ("What this backend deliberately cannot
do" — "An evicted MCP record's tab still exists"); `McpSessionRecords::retain_bounded`;
`run_app_refuses_bookkeeping_free_once_the_cap_of_live_sessions_is_reached` test.

---

### `no-hot-reload-restart-is-a-rebuild` — hot patching covers in-place code edits only; every restart is a full rebuild + relaunch

**Observed**: a debug `frust run --watch` (the desktop preview, `-d <Android device>` or `-d <booted
iOS simulator>`) and the TUI's 'Watch: hot patch on save' on the same targets hot-patch a save into
the running process: the changed workspace-member crates are replayed, thin-linked into a patch
image and applied over devtools, and the CLI prints `patched in <n> ms` (the TUI a `✓ patched`
toast) with the PID and every component's `State` kept. The `frust/hotpatch` feature is switched on
for the hot run's own fat build only, never by default. On the desktop, `--no-hot`, a
profile/release build or a `--features` passthrough keeps the plain relaunch loop; `--watch -d`
refuses a non-Debug build or extra features (`run_watch_on_device`), so `--no-hot` is the only flag
that picks the device relaunch loop; a hot run whose setup fails, or that reports it cannot patch,
falls back to that loop too, watching only `src/` and `Cargo.toml`. A compile error leaves the
running app untouched. Anything a patch cannot carry prints `restart required: <reason>`, never
`patched`, and the app is relaunched from a fresh fat build with State reset. The reasons
(`RestartReason`) fall into five classes:
- *Layout or identity*: a replayable type's layout differs from the accepted set (`LayoutChanged`,
  from the host's L3 gate over DWARF (Mach-O/ELF) or PDB type records (Windows), or a mismatch the
  app reports), or a component's `State` changed type (`StateTypeChanged`).
- *Build-input or path-dependency edit*: a manifest, build script, cargo config or toolchain file
  (`BuildInputChanged`), or a file of a path dependency outside the workspace (a local framework
  checkout included) that cannot replay (`PathDependencyChanged`). On the desktop a captured path
  dependency replays with its dependents under the L3 gate; the restart remains for Android and
  the iOS simulator (which do not capture non-members), proc macros, path packages without a lib
  target, uncaptured ones and a crate name clashing with a member. Evidence:
  `examples/hotpatch-spike/RESULTS.md` `## H5-01: path-dependency replay` (a pass-through
  `RUSTC_WRAPPER` rebuilds 0 of 27 canary units) and the canary's non-member crate.
- *Budget*: patch images are never unloaded, so one process loads at most 64 patches or 96 MiB of
  patch images by default, set by `frust.toml`'s `[hotpatch]` `patches`/`bytes` (`PatchBudget`).
- *Protocol or outcome failure*: no `HotPatch` capability, a non-loopback endpoint, a failed
  devtools call, a refused patch, a reply lost after `apply_patch` (`PatchOutcomeUnknown`: the
  patch may be live), stale code still calling a replaced seam, an unmappable missed seam key, or
  a patch no component ran.
- *Builder refusal*: anything the builder does not recognise fails closed (`BuilderUnsupported`).

The first patch of a hot run is slower than steady state: a few hundred ms more on the desktop
(run 1 at 1551 ms against a 1362 ms steady median in the milestone-1 re-run), and ~5.3-5.5 s
against ~1.45 s on Windows. `R` / 'Restart session', MCP's `restart_app` and DAP's
`frustRestart` are full rebuild + relaunch with State reset (Flutter's hot restart, never
in-process); a device restart reruns build → install → launch (see
`tui-device-stop-app-termination-residual` and `mcp-stop-app-termination-in-flight`). `restart_app`
and `frustRestart` bypass the TUI's `restart_session_at`, so their replacement session starts
unwatched. In the TUI, every hot-capable launch runs hot; `r` hot-patches now (the manual apply),
and a run with 'Auto-apply on save' off is a supported hot run with no watcher. Turning Watch on
for a session launched without a hot run makes the next save a plain relaunch into a hot session.
`r` compares file modification times with the last patch, so a tool that preserves mtimes or a clock
step can hide an edit; saving the file again makes it visible.

The memory-safety argument is the host's: L1 erases each component's view inside the hot function
(`build_erased`), so the seam's boundary types are layout-fixed, and L3 refuses any patch whose
types differ from the accepted layouts. The app's L2 witness (`SeamWitness`, written by the image
that created a component's state) is a backstop with two limits. It covers only a component hosted
in its own erased `Box<dyn Widget>`; one held inline by a generic container (`Either`, or an app
`View` whose `Element` embeds a child element) is covered by L3 alone. And on a witness mismatch the
creator's `Box<C::State>` is leaked (`mem::forget`, never dropped or freed by another image) and the
run restarts: one `C::State` per mismatched widget, once.

**Applies to**: hot patching on debug builds for macOS and Linux desktop, Windows x64 MSVC (the
layout gate reads PDB type records, and the devtools token comes from the OS CSPRNG,
`BCryptGenRandom`, so `Capability::HotPatch` answers the same gate as on every other host), Android
arm64-v8a and the iOS simulator. A physical iOS device (a loaded patch would need code signing) and
the web shell (`frust-hotpatch` is a `compile_error!` on wasm32) are not watched at all; Windows
ARM64 and other Android ABIs never patch. The Windows residuals are
`hotpatch-windows-fixed-base-no-aslr`, `hotpatch-windows-patch-file-acl` and
`hotpatch-windows-pdb-layout-align-blind`. The restart semantics apply to every restart entry point
across `frust-tui`, `frust-cli`, `frust-mcp` and `frust-dap`.

**Why accepted**: a patch is applied only where the host can show the running types are unchanged;
anything else restarts rather than risk memory unsafety. The median save→frame of a patch is
13.5-38.5% of the restart median on macOS desktop, Android arm64, Windows x64 and the iOS
simulator; Linux desktop is covered for correctness by the CI canary
(`scripts/ci/hotpatch-canary.sh`) with no measured ratio. The inline-host gap is closed
by L3 on every hot target, and closing it in-app would need a witness check on every `event`,
`layout` and `paint`. In-process hot restart (State reset without a relaunch) is still out of reach:
`frust::run` takes the root `Component` by value, once; it runs under the shell's root `Owner`,
which lives for the whole process and is never disposed; and `ReactiveRuntime` sits in a
process-lifetime `OnceLock` that is never torn down.

**Reopen path**: hot restart needs `frust::run`'s by-value root replaced with a factory closure, a
disposable root `Owner`, a resettable `ReactiveRuntime` and a devtools `restart` method. A physical
iOS device needs signed patch delivery; the web shell needs a wasm32 patch loader.

**Evidence**: `examples/hotpatch-spike/RESULTS.md`: `## Milestone 1 re-run (R3-04)` (desktop macOS:
save→frame ~1.4 s, 26.8% of the restart median; `### Latency breakdown (why 1.4 s, not 493 ms, and
not 2.2 s)`; TUI Watch leg), `## Stage 2 re-run (H2-07)` (Pixel 5, Android arm64: ~2.9 s, 21.4%),
`### Stage 3 re-run (H3-03b)` under `## Stage 3: Windows gate (H3-03)` (Windows x64 MSVC: A 1468
ms, 13.5% of 10861 ms; the PDB gate refused D2 and D4), `## Stage 4: iOS simulator gate (H4-03)` (A
1694 ms, 38.5% of 4395 ms). `crates/frust-drive/src/hotpatch/session.rs` (`RestartReason`,
`Budget`); `crates/frust-drive/src/hotpatch/layout.rs` and `pdb_layout.rs` (L3);
`crates/frust-core/src/hotpatch.rs` (`build_erased`, `SeamWitness`) and
`crates/frust-core/src/component.rs` (`ComponentWidget`'s witness checks and `Drop` leak);
`crates/frust-widgets/src/either.rs` (inline arms); `crates/frust-cli/src/commands/run.rs`
(`WATCH_DEVICE_REJECTION`); `crates/frust-tui/src/engine/session_view.rs` (`supports_watch`);
`crates/frust-hotpatch/src/lib.rs` (wasm32 `compile_error!`); `crates/frust/src/lib.rs`
(`pub fn run<C: Component>`); `crates/frust-reactive/src/runtime.rs` (`static RUNTIME`);
`scripts/ci/hotpatch-canary.sh` (Linux and macOS canary).

---

### `dap-no-stepping-v1` — `frust-dap` has no breakpoints, stack, or variable inspection

**Observed**: `frust-dap`'s DAP surface is launch orchestration only —
`initialize`/`launch`/`configurationDone`/`threads`/`disconnect`/`terminate` plus the
`frustRestart`/`frustWidgetTree` custom requests. There is no `setBreakpoints`, `continue`,
`next`/`stepIn`/`stepOut`, `stackTrace`, `scopes`, or `variables` — `threads` answers a single
static thread only so a client's UI has something to show, not because stepping is supported on it.

**Applies to**: every `frust-dap` session, on every platform.

**Why accepted**: stepping is deliberately delegated to native lldb tooling (Xcode's
debugger for iOS, Android Studio/`lldb.sh` for Android, a desktop `lldb`/`gdb` attach) rather than
reimplemented on top of the launch-orchestration engine, which has no breakpoint/stack/variable
model to build one on. `frust-dap` orchestrates build/deploy/launch/logs; a real debugger attaches
to the running process for anything past that.

**Evidence**: `crates/frust-dap/src/adapter/mod.rs`'s `threads`/dispatch surface (no
breakpoint/stack/variable request handling).

---

### `dap-launch-only-no-attach-v1` — `frust-dap` cannot attach to an already-running app

**Observed**: `frust-dap`'s only entry into a session is its own `launch` request, which calls
`SessionBackend::run_app` on the **host's** shared backend and always starts a brand-new session —
even though that backend is the same one already supervising every app the host's own UI has
launched, `launch` never adopts one of those existing sessions. There is no `attach` request, and no
way to hand a connection an already-running `SessionId` (one the user started by hand, or another
DAP/MCP client started) to debug it in place.

**Applies to**: every `frust-dap` connection; a client must always `launch` a fresh app through it,
never attach to one already running in the host.

**Why accepted**: v1 scope — one app per connection is the whole orchestration-v1 contract, and `attach` needs a session-picker UX this adapter does not have (the
backend exposes `sessions()`, but nothing in the DAP protocol surface here resolves a client's
choice into one).

**Evidence**: `crates/frust-dap/src/adapter/mod.rs`'s request dispatch (no `attach` handling) and
`OrchestrationAdapter::launch` (always calls `backend.run_app`, never reuses an existing
`SessionId`).

---

### `dap-tcp-unauthenticated-v1` — the embedded DAP server has no authentication beyond the loopback bind

**Observed**: `frust_dap::serve_embedded`'s loopback listener binds `127.0.0.1` only and has no
login, token, or capability check at the DAP protocol layer — any local process that can reach the
port can drive the host workbench's own session world (launch, `frustRestart`, stop) through it,
exactly the same reach a running `frust-tui` gives its embedded MCP server. It **cannot**, however,
choose the build directory: a client's `launch.projectRoot` is always ignored — every session
launches from the workbench's currently open project (read live from the host's backend at launch
time; a client-supplied `projectRoot` is never honored), and an ignored request is echoed back,
sanitized, as a Debug Console note rather than silently dropped. So
the reach is driving sessions against the host's own open project, not running `cargo` — and
therefore arbitrary `build.rs`/proc-macro/`.cargo` runner code — from an attacker-chosen directory.

**Exposure asymmetry with the embedded MCP server**: the reach is the same once connected, but the
*likelihood of a listener existing at all* is not, and this entry would be misleading without
saying so. The MCP server has **no auto-start path** — it binds only when a human toggles it (`M`,
the panel's `s`, or the palette). The DAP server does auto-start: `[dap].auto_start_in_ide` and
`auto_configure_ide` both default **on** (a deliberate product decision), and the
IDE detection they gate on reads inherited environment variables (`TERM_PROGRAM`,
`VSCODE_IPC_HOOK_CLI`, `ZED_TERM`, `TERMINAL_EMULATOR`, `NVIM`, `INSIDE_EMACS`, `HELIX_RUNTIME`) —
which every subshell, `tmux` pane, and nested shell started from an IDE terminal inherits, so a
workbench launched well away from the editor still counts as "inside an IDE". The **first** such
auto-start on an install is therefore gated: instead of binding, it opens the DAP settings dialog
with a one-time notice naming the port, and a listener starts on that run only if the user asks it
to (`[dap].intro_seen` records that the notice was spent, whether or not they acted). Every launch
after that binds silently, as the defaults ask — the acknowledgment is one-time, not per-launch —
and auto-start can be turned off in the same dialog (or `[dap].auto_start_in_ide = false` in
`~/.config/frust/tui.toml`). With `auto_configure_ide` on, the detected IDE's DAP launch config is
also written automatically — on every app launch (one write per distinct project root) and, on a
fresh bind, for the active session's project — but only in `WriteMode::IfAbsent`: a file that
already carries the frust entry is left untouched, never rewritten (a toast names the file when the
retained entry's port has gone stale — see `dap-ide-config-normalizes-launchjson` below). Only the
dialog's own explicit Generate (`WriteMode::Refresh`) updates an existing entry, e.g. to pick up a
changed port.

**Applies to**: every `frust-dap` connection, on every platform — there is no other transport (the
server is embedded-only).

**Why accepted**: the same v1 stance as `frust-mcp`'s (see `mcp-server-unauthenticated-v1` above),
matched to the same threat model — a loopback developer tool not reachable off-host. Server-side
auth is a named follow-up, not a v1 requirement, for either embedded server. The `project_root`
confinement above is not part of that follow-up — it ships now, so an unauthenticated client cannot
escalate a session into arbitrary local code execution from a directory of its choosing. The
first-run notice is **disclosure, not authentication**: it makes the auto-start visible once, it
does not stop a local process from connecting. The remedy that would actually close the asymmetry
is a per-server token — minted on each start and written into the generated IDE config, so a client
must present what the editor was handed — which stays a named follow-up alongside MCP's.

**Evidence**: `crates/frust-dap/src/server/embedded.rs`'s `serve_embedded` doc comment (loopback
bind, the project root is read live from the host's backend and is never overridden by a client);
`crates/frust-dap/src/adapter/mod.rs`'s `OrchestrationAdapter::launch` (`client_root_note`,
sanitized echo of an ignored `projectRoot`); `crates/frust-tui/src/engine/update.rs`'s
`Message::DapAutoStart` arm and `crates/frust-tui/src/engine/dap_settings.rs`'s
`DapSettings::intro_port` (the one-time gate and its lifecycle).

---

### `dap-exited-success-flag-only` — a DAP `exited` event carries 0/1, never a real process exit code

**Observed**: `frust-dap`'s `exited` event body always carries `0` or `1` — `frust_mcp::engine`'s
`SessionState::Exited` is a success flag, not a real exit code (`StreamHandle` exposes none on any
platform), so there is no underlying value to report faithfully.

**Applies to**: every `frust-dap` session's `exited` event, on every platform.

**Why accepted**: the underlying engine has never captured a real exit code (desktop/Android/iOS
Simulator sessions alike) — `frust-dap` reports the same fidelity the engine has always had, rather
than inventing a code that would not mean anything. A client relying on `exited`'s code for
anything beyond success/failure will be misled.

**Evidence**: `crates/frust-dap/src/adapter/pump.rs`'s `EXIT_OK`/`EXIT_FAILED` doc comments;
`frust_mcp::engine::SessionState::Exited` (no exit-code field).

---

### `dap-log-subscription-may-drop-lines` — a slow DAP client can lose output lines under back-pressure

**Observed**: `SessionBackend::subscribe_session_events` is a bounded (`LOG_SUBSCRIPTION_CAP`,
4096-line) channel; a client that reads `output` events slower than the app produces lines causes
`frust-dap`'s log pump to back up and the feed to drop the oldest lines once full. Loss is never
silent: an in-band `[frust] <N> log line(s) dropped (…)` line is delivered as its own `output` event
before the gap. This is the *feed's own* loss marker — see `dap-supervisor-drop-not-marked` below
for a distinct, upstream loss the feed cannot see at all when the host is `frust-tui`.

**Applies to**: any `frust-dap` session whose client (or the DAP wire itself) cannot keep up with
the app's log rate; unreachable in ordinary interactive use.

**Why accepted**: the alternative is an unbounded buffer (a memory-growth hazard) or blocking the
app's own log-ingest thread on a slow client (a hang hazard) — neither acceptable for a debug
server. A marked drop is the same trade-off every implementer of `subscribe_session_events` makes
(`frust-mcp`'s own reference `SessionEngine`, and `frust-tui`'s embedded backend, both bound at the
same cap).

**Evidence**: `crates/frust-mcp/src/backend.rs`'s `subscribe_session_events` doc comment (bounded,
in-band loss markers); `crates/frust-dap/src/adapter/pump.rs`'s log pump (`read_feed`/
`forward_events`).

---

### `dap-helix-config-skipped` — Helix never gets a generated DAP config

**Observed**: `frust_dap::ide_config::generate_ide_config` for `ParentIde::Helix` always returns
`ConfigAction::Skipped` with a fixed reason, without writing anything, regardless of `port` or
`project_root`. Helix's own `[language.debugger]` config only knows how to **spawn** a debug-adapter
binary via `port-arg` — it has no pure attach/TCP-connection form that could point at a DAP server
already running (`frust-dap`'s only shape, since there is no standalone `frust dap` process to
spawn).

**Applies to**: any workbench where the detected or overridden IDE is Helix; the DAP settings
dialog's `g` (Generate) action always reports the skip, and the automatic on-launch/on-bind flow's
own `IdeConfigRequest::reports` treats it the same as any other no-op (silent, since it never wrote
anything a user would need to hear about).

**Why accepted**: the tool this module was ported from worked around the identical gap
by spawning a *second*, separate adapter-binary process — a workaround `frust-dap` cannot reuse,
since it has no per-session spawnable binary of its own. Generating a config that could never
actually connect was rejected in favor of honestly reporting nothing was written.

**Evidence**: `crates/frust-dap/src/ide_config/helix.rs`'s module doc and `SKIP_REASON`;
`crates/frust-dap/src/ide_config/mod.rs`'s `generate_ide_config` Helix arm.

---

### `dap-ide-config-normalizes-launchjson` — the first VS Code `launch.json` merge destroys comments and hand formatting

**Observed**: `VSCodeGenerator::merge_config` (`crates/frust-dap/src/ide_config/vscode.rs`) parses an
existing `.vscode/launch.json` as JSONC (`ide_config::merge::clean_jsonc` strips every `//`/`/* */`
comment and trailing comma) and reprints the whole document via `to_pretty_json`
(`serde_json::to_string_pretty` over the parsed value tree). Every non-frust entry's *data* survives
the merge — the doc comment's "preserve all non-frust entries" promise holds semantically — but a
hand-authored file's comments and formatting do not: they are gone after the very first real merge,
replaced by frust's own 2-space-indent reprint. `run_generator`'s skip-on-unchanged check
(`crates/frust-dap/src/ide_config/mod.rs`) only compares the merged, reprinted output against what's
already on disk — it protects a file already in frust's canonical (comment-free, reprinted) form from
a redundant rewrite, it does not prevent the destructive first rewrite of a file that still carries a
user's comments/formatting.

**Applies to**: any project whose `.vscode/launch.json` predates frust-dap and carries hand-written
comments or formatting, detecting VS Code/VS Code Insiders/Cursor as the parent IDE, in either of
two cases now that the automatic path writes under `WriteMode::IfAbsent` (see
`dap-tcp-unauthenticated-v1` above): the dialog's explicit `g` (`WriteMode::Refresh`) always merges
and can reprint the file regardless of whether a frust entry is already present; or the automatic
on-launch/on-bind path's *first* write into a file that has no frust entry yet, since `IfAbsent`
still calls `merge_config` (not a byte-preserving append) to add one. Once that first entry exists,
every later automatic write leaves the file untouched (`has_frust_entry` short-circuits it) — only
`g` can reprint it again after that. A kept entry that names a different port than the one the
server bound now surfaces as a toast (`ConfigAction::StalePort`, at most once per project root and
IDE per run) pointing at `g` — taking that suggestion runs the same destructive reprint described
above; a kept entry whose port cannot be read is left the same way but stays silent, with no toast.
Emacs' `.frust/dap-emacs.el` is outside this entry: it is frust-owned rather than user-editable and
is regenerated in full every run instead of merged, so there is no hand-written content to lose.

**Why accepted**: a byte-preserving surgical splice (find the frust entry's byte span inside the
original text and edit only that span, leaving everything else untouched) is the real fix, but was
judged disproportionate — hand-rolling JSONC span-splicing is real parser-writing risk
for a config-generation feature, and no byte-preserving JSON/JSONC crate is pinned in this workspace
(pins are LAW, `docs/DEVELOPMENT.md`'s Version-Pin Policy). `toml_edit` is the precedent for exactly
this shape on the TOML side (`docs/TUI_DEVELOPMENT.md`'s pin row), but it has no JSONC-editing
equivalent pinned here. The chosen remedy is honest disclosure — this entry and the doc comments on
`merge_config`/`run_generator`/`post_write` — rather than a bigger parser change. The write hardening
(temp-file-and-rename landing, and refusing a config path that resolves via symlink outside the
project) changes how a write lands, not what content is merged, so it does not touch this loss.

**Evidence**: `crates/frust-dap/src/ide_config/vscode.rs`'s `VSCodeGenerator::merge_config` (clean →
parse → reprint) and its module doc; `crates/frust-dap/src/ide_config/merge.rs`'s `clean_jsonc`
(comment/trailing-comma stripping) and `to_pretty_json` (`serde_json::to_string_pretty` reprint);
`crates/frust-dap/src/ide_config/mod.rs`'s `run_generator` (byte-equality skip check against the
reprinted output only, and the `ConfigAction::StalePort` branch) and
`IdeConfigGenerator::frust_entry_port`; `crates/frust-tui/src/engine/dap_settings.rs`'s
`DapSettings::admit_auto_toast`
(the once-per-root-and-IDE-per-run toast limit) and `DapIdeReport::summary`'s stale-port message;
`crates/frust-dap/src/ide_config/emacs.rs`'s `EmacsGenerator::has_frust_entry` (always `false`).

---

### `dap-mcp-launched-sessions-skip-auto-ide-config` — a session launched through the embedded MCP/DAP backend never triggers automatic IDE-config generation

**Observed**: `engine::update::launch_effect` — the one path an app launch's automatic
`GenerateIdeConfig` write rides — is reached only from the workbench's own launch paths (the
run-config modal, run-on-all-devices); a session an MCP tool or a DAP client starts through
`supervise::mcp_backend`'s `TuiSessionBackend::run_app` bypasses it entirely, by design, so that
launch never writes or refreshes an IDE's DAP launch config. (2026-09-24)

**Applies to**: every session started via the embedded MCP `run_app` tool or a DAP client's
`launch` request; a session started by hand in the workbench, or the DAP settings dialog's own
explicit `g`, are unaffected.

**Why accepted**: an MCP/DAP-launched session already has a client attached over a protocol that
required a config to connect with in the first place — generating one for it would be redundant,
not corrective. A human launching the same project by hand still gets the usual on-launch write.

**Evidence**: `crates/frust-tui/src/engine/update.rs`'s `launch_effect` doc comment ("Sessions
launched through the MCP/DAP backend … never pass through here, by design").

---

### `tui-widget-tree-refusal-code-loss` — an embedded `widget_tree` failure loses its original error code

**Observed**: `TuiSessionBackend::fetch_widget_tree` classifies a pull failure into
`TreeRefusal::Unavailable` (a typed `NOT_SUPPORTED`) or `TreeRefusal::Failed(String)` — and the
latter is built from `supervise::DevtoolsBridge`'s already-stringified error
(`engine::InspectorEvent::Failed`), so the *original* devtools error code (e.g. `unauthorized`) is
not recoverable by the time it reaches the trait method. A pull that failed because the app's
devtools token went stale arrives as plain prose rather than the classified error a caller reading
`devtools_client` directly would get — both `frust-mcp`'s `widget_tree` tool and `frust-dap`'s
`frustWidgetTree` request see the narrowed version when the host is `frust-tui`'s embedded backend.

**Applies to**: `fetch_widget_tree` calls served through `frust-tui`'s embedded `TuiSessionBackend`
only — the standalone `frust_mcp::SessionEngine` backend returns the original typed
`DevtoolsRpcError` and is unaffected.

**Why accepted**: the bridge's own event channel (`InspectorEvent`) is string-typed by the time it
crosses from `DevtoolsBridge`'s thread into engine state — reclassifying it would mean carrying a
richer error type across that channel, a larger change than reading it back out again. The source
itself records this as "a real narrowing, recorded rather than papered over by guessing a code back
from the text," not an oversight.

**Evidence**: `crates/frust-tui/src/supervise/mcp_backend.rs`'s `TreeRefusal::Failed` doc comment.

---

### `dap-supervisor-drop-not-marked` — a supervisor-level line drop is invisible to a DAP client's feed

**Observed**: `frust-tui`'s `Supervisor` reports its own bounded-channel overflow (a build/run
process producing lines faster than the workbench's internal channel can carry) as
`SessionEventKind::Dropped(n)`, a cumulative, drop-newest counter. `engine::update` stores it only
on `SessionView::dropped` (reported through the MCP session summary) — it is never appended to the session's own log
store as a line. `supervise::session_feeds::SessionSubscribers::deliver_lines` (what feeds a DAP
client's `SessionEventFeed`) replays only lines actually present in that log store, so a client
attached over `frust-dap` has no way to learn the supervisor itself silently dropped `n` lines
upstream — only the *feed's own* overflow (a separate, already-marked case; see
`dap-log-subscription-may-drop-lines` above) is reported in band.

**Applies to**: any `frust-dap` session whose host is `frust-tui` and whose supervisor-level output
channel overflows; unreachable in ordinary interactive use (the same rarity as the feed's own
overflow), and distinct from — upstream of — that overflow.

**Why accepted**: the overflow needs bursts of output deeper than the shared 256-slot channel (each
slot a coalesced batch of lines), so it is unreachable in ordinary interactive use, and the count is
still kept on `SessionView::dropped` and reported in the session summary the MCP backend serves
(`dropped_log_lines`). Closing it needs either synthesizing a log
line for the drop count (extra plumbing `SessionView` does not otherwise need) or extending
`SessionEvent` with a marker variant `SessionSubscribers` replays alongside real lines.

**Evidence**: `crates/frust-tui/src/supervise/supervisor.rs`'s `SessionEventKind::Dropped`;
`crates/frust-tui/src/engine/update.rs`'s `SessionEventKind::Dropped(n)` arm (updates
`session.dropped` only); `crates/frust-tui/src/supervise/session_feeds.rs`'s `deliver_lines` (reads
only `view.log`).

---

### `tui-dap-enabled-file-only` — `[dap].enabled` has no dialog control

**Observed**: `engine::persist::DapPrefs::enabled` ("start the server on every launch, IDE or not")
is fully read and written through `tui.toml`'s `[dap]` table, but the DAP settings dialog exposes no
control for it — only `auto_start_in_ide`, `auto_configure_ide`, `port`, and `ide_override` are
editable from the dialog (`engine::dap_settings::DapFocus` has no variant for it). A user who wants
the server always on, not just when a parent IDE is detected, must hand-edit the config file.

**Applies to**: the DAP settings dialog only; `enabled` is otherwise fully functional — loaded at
startup and consulted in the `enabled || (auto_start_in_ide && detected)` auto-start check.

**Why accepted**: documented as deliberate in the source itself — `enabled` targets an
always-on user already comfortable editing `tui.toml` directly, while the dialog's own
`auto_start_in_ide`/`auto_configure_ide` pair covers the common IDE-detected case a dialog control
would otherwise duplicate.

**Evidence**: `crates/frust-tui/src/engine/persist.rs`'s `save_dap_enabled` doc comment ("The
dialog exposes no control for it"); `crates/frust-tui/src/engine/dap_settings.rs`'s `DapFocus`
(no `Enabled` variant).

---

### `windows-smart-app-control-blocks-unsigned-binaries` — Smart App Control in Enforce mode can block freshly built binaries

**Observed**: with Windows 11's Smart App Control in Enforce mode, a freshly built unsigned
binary — a `cargo test` binary or a `frust`-scaffolded dev build alike — can be blocked from
executing, surfacing as `os error 4551` rather than a normal test failure or launch error. (2026-09-24)

**Applies to**: any Windows 11 host running Smart App Control in Enforce mode; both the workspace's
own test binaries and a scaffolded app's dev/debug builds are unsigned by default.

**Why accepted**: [`scripts/testing/tui-windows-gate.sh`](../scripts/testing/tui-windows-gate.sh)'s
per-crate test leg reports a blocked binary as BLOCKED rather than FAIL specifically because of
this — the gate can only ask for Smart App Control to be off up front, not work around Enforce mode
once it blocks a binary that was just built.

**Evidence**: a Windows 11 Pro 26200 gate, 2026-09-24.

**Trigger for removal**: signing dev builds by default, or `frust doctor` detecting Smart App
Control's Enforce mode and warning up front.

---

### `windows-scaffold-pdb-filename-collision-warning` — a scaffolded app's Windows build warns about a `.pdb` filename collision

**Observed**: building a scaffolded app for Windows desktop prints a cargo `output filename
collision` warning for the `.pdb` symbol file, because the app's lib target (cdylib/staticlib/rlib)
and its bin target share a crate name — an upstream cargo limitation (rust-lang/cargo#6313), not a
Frust template defect.

**Applies to**: every scaffolded app built for Windows desktop.

**Why accepted**: the warning is harmless — cargo still produces both artifacts correctly — and
resolving it would mean renaming one of the two targets, which would ripple into every platform's
build for a cosmetic warning on one.

**Evidence**: a Windows 11 gate, 2026-09-24; rust-lang/cargo#6313.

**Trigger for removal**: cargo resolving the collision upstream (cargo#6313), or a future template
split of the lib/bin crate names.

---

### `windows-run-warns-missing-icon-before-first-build` — `frust run` warns about a missing `icon.ico` before the first `frust build`

**Observed**: `frust run` on Windows prints a `build/desktop/windows/icon.ico not found — run frust
build` warning until a `frust build` has produced that file once.

**Applies to**: a freshly scaffolded or freshly cloned Windows project that has never run `frust
build`.

**Why accepted**: the icon is generated by the build pipeline, not the scaffold, so a project's very
first `frust run` necessarily predates it; the warning is accurate and self-resolving on the next
`frust build`.

**Evidence**: a Windows 11 gate, 2026-09-24.

**Trigger for removal**: `frust run` generating the icon itself ahead of a first build, if that is
ever judged worth doing.

---

### `i18n-live-locale-change` — a system-language change while the app runs is not reactive

**Observed**: `frust_i18n::system_locales()` re-queries the OS on every call, but nothing in
the crate subscribes to a platform locale-change notification — a user who switches the
system/app language while the app keeps running sees no automatic effect; `I18n`'s already-
negotiated locale and chain stay exactly as they were until the app polls `system_locales()`
again and calls `I18n::set_locale` itself. In practice Android recreates the launching
`Activity` on a system-language change, which incidentally re-detects on relaunch — but no live
in-process notification is delivered either way.

**Applies to**: every platform. A shell-forwarded locale-change event (the seam an app could
subscribe through instead of polling) is deferred, not shipped.

**Why accepted**: v1 scope — detection is a deliberately uncached, re-query-on-demand read
(`src/detect/mod.rs`'s module doc), and no platform event is plumbed through any shell yet. An
app that wants to react to a live change polls `system_locales()` itself (e.g. on resume) and
calls `I18n::set_locale`.

**Evidence**: `plugins/i18n/src/detect/mod.rs`'s module doc; `plugins/i18n/README.md` §6's "Not
reactive" note.

---

### `i18n-rtl-unverified` — intra-line bidi behavior with FSI/PDI marks is device-unverified

**Observed**: this framework's widget layout is LTR, and nothing in it has been
device-verified rendering right-to-left content. `LocaleSet`'s default bidi isolation wraps
every interpolated placeable in FSI/PDI marks (`U+2068`/`U+2069`) specifically so an RTL
argument cannot reorder the surrounding text — but how the text stack's bidi (`parley_engine`'s
levels plus frust's own L2 reorder in `frust-text`'s `layout/`) handles those marks alongside
mixed-direction text has not been exercised on a device.

**Applies to**: any message interpolating an RTL argument into an LTR (or mixed) message,
pending the device gate the `i18n` manual test owes (`docs/PLUGINS_DEVELOPMENT.md`).

**Why accepted**: no RTL locale or right-to-left CLDR data has shipped in this repo's locale
sets yet; bidi layout was out of scope for this plugin's v1, which owns message resolution and
formatting, not text layout.

**Evidence**: `plugins/i18n/src/engine/mod.rs`'s `LocaleSet::with_isolating` doc; the pending
`i18n` device gate in `docs/PLUGINS_DEVELOPMENT.md`.

---

### `i18n-long-unit-width-short` — English length units at Long width render the short form

**Observed**: `fmt::units` at `Width::Long` renders English `km` and `m` as `2 km` and `2 m`,
not `2 kilometers` / `2 meters`. ICU4X 2.4 / `icu_experimental` 0.7.0 itself returns the short
form at Long width, so frust's mapping passes it through. Metres are checked through ICU's typed
`CategorizedFormatter<Length>`, kilometres through the baked `long-kilometer` data attribute.

**Applies to**: `en` (and locales inheriting it) for the length units `km` and `m`; other
locales and units are not known to be affected.

**Workaround**: spell the unit in the message text, or format the number with `fmt::number` and
append the localized unit name.

**Why accepted**: an upstream CLDR data gap, not a frust fault; an in-crate override table would
duplicate ICU data. The `formatting_matrix` `2 km` Long pin records the current output and is
revisited on an ICU4X bump.

**Evidence**: `plugins/i18n/tests/units_width.rs` (typed metres, attribute kilometres); `plugins/i18n/tests/formatting_matrix.rs` (`2 km` pin).

---

### `i18n-inmessage-format-locale` — in-message `NUMBER()`/`DATETIME()` format at the resolving bundle's locale

**Observed**: in-message Fluent placeables like `{ DATETIME($when, dateStyle: "medium") }`
format using the locale of the bundle that resolved the message, not the `format_locale`.
When a message is missing from the active bundle and resolved from a fallback, the
placeable renders in the fallback's language conventions — e.g., an `en-GB` user of a
partially-translated app with `de` (partial) + `en` (fallback) bundles sees German UI
text with English month names inside (Jan instead of Dez) when hitting an untranslated
key. For messages present in the active bundle, region-level conventions still diverge
from `format_locale` — an `en-GB` user of an `en`-only app sees `Jan 31, 2024` (US date
order) inside messages, but `31 Jan 2024` (UK date order) from direct `frust_i18n::fmt::date`
calls.

The constraint covers every FTL option too: `DATETIME`'s `timeZone`/`calendar`/`hourCycle`,
`NUMBER`'s `minimumFractionDigits`/`maximumFractionDigits`/`notation`/`compactDisplay`/
`useGrouping`/`minorUnits`, and the `RELATIVE_DAY` and `LIST` functions all format at the
resolving bundle's locale.

**Mechanism**: ICU Fluent functions (`NUMBER`/`DATETIME`) are registered once per bundle
locale during `Engine::new` via `LocaleSet::with_locale_function`, with each closure
capturing its bundle's locale by value. `Engine::resolve` walks the negotiated chain
and formats with whichever bundle first owns the key; the captured locale comes from that
resolving bundle, not the active request locale. Since `Engine` is immutable after
construction, every in-message placeable formats at the capturing locale — there is no
resolve-time parameterization.

**Applies to**: any app using `formatting` feature + in-message `NUMBER()`/`DATETIME()`
placeables + (a) a user whose regional preference differs from the message-bundle language
coverage (region-drift case), or (b) a partially-translated app where some active-locale
keys only exist in a fallback bundle (fallback-language case). Both are normal with
fallback-chain message resolution.

**Residual gap**: language-level conventions (month names, era names, calendar system)
and region-level conventions (date order, grouping, currency defaults) inside FTL-embedded
formatting only. The app-side direct formatters (`fmt::date`, `fmt::decimal`, etc.) and the
message resolution path (`I18n::locale`, `I18n::t`) are unaffected.

**Workaround**: format app-side via `frust_i18n::fmt::` (passing `I18n::format_locale`)
and interpolate the formatted string into the message key, rather than embedding the
formatter call in the `.ftl` source. Example: resolve a message skeleton like
`last-visit = Last visit: {$formatted_when}`, computing `formatted_when` from an
app-side `fmt::date(i18n.format_locale(), ...)` call.

**Revisit**: resolve-time locale parameterization — registering ICU functions per
locale at resolve time rather than at engine construction. This would make in-message
formatters locale-aware like the direct API, at the cost of per-call overhead and
a more complex `Engine::resolve` contract. Deferred from v1 scope.

**Evidence**: `plugins/i18n/src/engine/resolve.rs`'s chain walk (the first owning
bundle's `entry.bundle` is formatted); `plugins/i18n/src/fmt/fluent_fns.rs`
(`with_icu_functions`' closures capture the locale at bundle-build time).

---

### `i18n-formatting-default-cost` — `formatting` feature is default-on and costs ~1.09 MB per app binary

**Observed**: `frust-i18n`'s `formatting` feature (ICU4X-backed `NUMBER()`/`DATETIME()`
formatting in FTL messages and the direct API) is in the default feature list
(`plugins/i18n/Cargo.toml`'s `default = ["frust-api", "formatting"]`). Building with the
feature enabled bakes ~1,142,992 bytes (~1.09 MB) of CLDR data into every app binary
(measured with `cargo-bloat` on a lean `frust-core`-only binary, stripping to the shared
`base.dat` singleton and baked-in symbols; the figures survive release-binary measurement).

**Mechanism**: the `formatting` feature depends on ICU4X's experimental CLDR data crates
(`icu_datetime_data`, `icu_decimal_data`, `icu_currency_provider`, `icu_plurals_data`).
Each data crate embeds CLDR tables as static initializers, which end up in the final binary.
Unlike dynamic data loading (ICU4C's `.dat` file, unreachable from pure-Rust isolation),
there is no opt-out once linked — only a compile-time feature disable.

**Applies to**: every `frust-i18n` app that ships with default features enabled —
entirely of app binaries using this crate (a mobile-first, default-enabled cost that
affects every new user's first download).

**Why accepted**: the feature's formatter quality (full CLDR support, ICU4X correctness
guarantees over hand-rolled tables) was judged to outweigh the cost. The decision was
re-confirmed after the original ~49 KB estimate was corrected to the honest +1.09 MB
figure. Accepted by the maintainers 2026-08-15.

**Opt-out**: `default-features = false` in `frust-i18n`'s dependency stanza (app's
`Cargo.toml` or workspace root). Apps that skip the feature retain the direct API's
`NUMBER()`/`DATETIME()` calls (subject to `i18n-inmessage-format-locale`'s constraints)
but lose in-message formatting entirely and gain no binary savings (Fluent's own parser
does not become conditional).

**Evidence**: `plugins/i18n/README.md` §10 (size measurement procedure and figures);
`plugins/i18n/Cargo.toml`'s default feature list and the `formatting` feature's own
dependencies.

---

### `desktop-shells-runtime-unverified` — the Linux desktop shell's Wayland `app_id` pairing is unverified

**Observed**: the Linux desktop shell's window identity has been observed on X11 only (2026-09-13,
KDE on Xorg, NVIDIA T400: `WM_CLASS` matched the app id, the `.desktop` entry paired via
`StartupWMClass`, and `_NET_WM_ICON` rendered in the titlebar and taskbar). Under a Wayland session
the `app_id` pairing with the `.desktop` entry has not been observed, so whether the compositor
groups the window under the app's entry and icon is unconfirmed.

**Applies to**: a Linux desktop app under a Wayland session.

**Why accepted**: the call site was read against vendored winit source (0.30.13), and Wayland is a
distinct winit backend for the `app_id` path that has not yet been run for this pairing.

**Evidence**: the Linux row of `docs/SHELLS_DEVELOPMENT.md` § Per-OS desktop shell gate ("the Wayland
`app_id` pairing is still owed").

---

### `desktop-windows-titlebar-theme-revert` — a Windows titlebar override can be silently reverted between app brightness changes

**Observed**: winit 0.30.13's `Window::set_theme` never writes back its own `preferred_theme`
field (a winit bug), so its `WM_SETTINGCHANGE` handler — which branches on that field being `None`
to mean "no app override, follow the system" — treats an app-forced titlebar the same as an
unthemed one and can silently re-apply the *system* theme over it, without necessarily firing
`WindowEvent::ThemeChanged`. `frust-shell-windows`'s `TitlebarTheme` (`theme.rs`)
has no `applied == wanted` latch, so every core-signaled brightness change re-issues
`set_theme`, which closes the revert path between two *different* app-resolved brightnesses. It
does not close a revert that happens *between* those changes — most notably while an app override
is active: the core's override-wins rule holds `self.theme.brightness` steady across a platform
`ThemeChanged`, so the edge-gate that drives `take_pending` may not re-fire at all while the
mismatch persists. The 2026-08-19 Windows-11 gate probed the headline sequence by hand — the
system theme flipped while an app override held — and observed **no** revert (build 26200); the
residual window is timing-dependent and stays open at this winit pin.

**Applies to**: `frust-shell-windows` apps using an OS-native (non-`with_theme`) titlebar with
either the system-follow path or an app-forced brightness override, on any winit 0.30.13 build.

**Why accepted**: no winit-level fix exists to consume (the bug is in winit, not fixable from this
crate alone); a full fix needs shell-owned system-theme detection (e.g. polling the registry key
winit itself would consult) rather than reacting to winit's event stream — and it stays deferred:
the Windows runtime gate's manual `WM_SETTINGCHANGE` probe did not reproduce a revert, so the
detector would be built against a case no one can currently trigger.

**Evidence**: `crates/frust-shell-windows/src/theme.rs` module docs (the residual-gap section).

---

### `desktop-decorations-creation-time-only` — the window frame style is fixed when the window is created

**Observed**: `DesktopConfig::decorations` and `title_buttons` are read once, while the window
attributes are built. There is no runtime setter, so an app cannot switch between `Full`, `None`
and `NoTitleBar` after launch.

**Applies to**: every desktop shell.

**Why accepted**: the frame style changes the window's style mask, title-bar state and (on
Windows) shadow, and none of that is verified to survive a live switch on all three OSes. Every
known use picks one style at startup.

**Evidence**: `crates/frust-shell-desktop/src/config.rs` (`Decorations` docs);
`crates/frust-shell-desktop/src/app_handler.rs` (`apply_decorations`).

**Trigger for removal**: an app that needs a live switch, and a gate showing the switch is sound on
each OS.

---

### `desktop-window-commands-off-desktop-noop` — window commands do nothing off the desktop shell

**Observed**: `push_window_command` only fills a process-global queue that the desktop shell
drains. Mobile and web shells never drain it, so a command pushed there is never executed; the
queue is bounded at 32 and, once full, drops the oldest non-`RequestClose` entry and logs one
warning per process.

**Applies to**: Android, iOS and web builds that share app code with a desktop build.

**Why accepted**: window actions have no meaning on those shells, and a shared title-bar widget
stays harmless there. `WindowState` is likewise absent, so an app reads it as an `Option`.

**Evidence**: `crates/frust-core/src/window_control.rs` (queue, cap and overflow warning).

---

### `desktop-macos-begin-resize-unsupported` — `BeginResize` is a no-op on macOS

**Observed**: winit's `drag_resize_window` returns `NotSupported` on macOS, so the executor logs
one warning and does nothing. AppKit keeps its own edge resizing on both decoration levels, which
covers the use (the macOS decorations gate checked both).

**Applies to**: macOS desktop apps using `window_drag_region(..).resize(edge)` or
`WindowCommand::BeginResize`.

**Why accepted**: the OS already resizes from the window edges; an app hides its resize strips on
macOS and while maximized or fullscreen.

**Evidence**: `crates/frust-shell-desktop/src/window_control.rs` (`execute`);
`crates/frust-widgets/src/window_drag_region.rs` module docs.

---

### `desktop-title-buttons-macos-best-effort` — traffic-light placement under `NoTitleBar` is best effort

**Observed**: winit 0.30 has no API to place the macOS window buttons, so
`frust-shell-macos::title_buttons` moves the three `NSButton`s inside their title-bar view by an
AppKit technique AppKit does not support. It is re-applied on resize and fullscreen exit, skipped
in fullscreen, and a hierarchy that differs from the assumed one leaves the buttons at the OS
position with one warning.

**Applies to**: macOS apps passing `TitleButtons` under `Decorations::NoTitleBar`.

**Why accepted**: without it the buttons cannot be aligned with an app-drawn bar; the failure mode
is the OS default position.

**Evidence**: `crates/frust-shell-macos/src/title_buttons.rs` module docs.

**Trigger for removal**: a winit release that exposes window-button placement.

---

### `desktop-windows-menu-bar-undecorated-unverified` — a menu bar on an undecorated Windows window is not drawn

**Observed**: winit keeps `WS_CAPTION` on an undecorated window and zeroes its non-client area.
`frust-shell-windows` still installs the `HMENU` (`GetMenu` returns it) and warns once, but the
Windows gate saw no menu bar drawn, no space reserved, and nothing opening where it would sit.
(The id keeps its historical `-unverified` suffix because the runtime warning cites it.)

**Applies to**: Windows apps combining a native menu spec with `Decorations::None` or
`NoTitleBar`.

**Why accepted**: an app that draws its own title bar draws its own menu there; keyboard access to
the hidden `HMENU` is untested.

**Evidence**: Windows row of the decorations gate (SHELLS_DEVELOPMENT.md § Decorations and window
commands); `crates/frust-shell-windows/src/lib.rs` (`menu_on_undecorated` and its warning).

**Trigger for removal**: the shell skips the `HMENU` when undecorated, or the combination is
documented as unsupported in the API.

---

### `desktop-linux-wayland-minimized-unreported` — `WindowState.minimized` is always `false` on Wayland

**Observed**: Wayland gives a client no minimized state, so winit reports `is_minimized()` as
unknown and the desktop shell publishes `minimized: false`. `WindowCommand::Minimize` works (the
compositor hides the surface); restoring is the compositor's, not the app's.

**Applies to**: Linux Wayland sessions reading `use_context::<WindowState>()`.

**Why accepted**: a protocol limit, not a frust gap; X11, Windows and macOS report it.

**Evidence**: Linux Wayland row of the decorations gate (SHELLS_DEVELOPMENT.md § Decorations and
window commands); `crates/frust-shell-desktop/src/window_control.rs` (`is_minimized`).

---

### `desktop-macos-reopen-gap-already-active` — Dock-click reopen does not re-show an already-active hidden window

**Observed**: winit 0.30 owns `NSApplication`'s delegate and panics if replaced, so
`applicationShouldHandleReopen:` is unreachable; `frust-shell-macos` instead observes
`NSApplicationDidBecomeActiveNotification`. AppKit does not post that notification when the app is
already active — only on a genuine activation transition — so a Dock click on an already-active
app whose window is hidden does not re-show it. A `MenuSpec` app-item activated while the window is
hidden also stays queued (bounded at 64, oldest dropped with a warning) until some other event
produces a frame.

**Applies to**: macOS apps that hide (rather than close) their window and rely on Dock-click to
bring it back.

**Why accepted**: no public reopen hook exists in winit 0.30; the notification observer is the best
available substitute; the gap's exact shape was confirmed live on real hardware.

**Evidence**: runtime confirmation on a MacBook: a
Dock click on the already-active app with the window hidden did not re-show it (gap reproduced
exactly); the inactive-path reopen works.

---

### `desktop-macos-quit-skips-executor-drop` — ⌘Q/menu Quit bypasses the whole `ShellHandler` `Drop` chain

**Observed**: `frust-shell-macos` leaves Quit on muda's predefined AppKit `terminate:` action (both
⌘Q and the menu item) because it must work while the window is hidden, when no frames are being
produced to service a pumped quit request. On that route winit dispatches `LoopExiting`, but
`run_app` never returns — AppKit tears the process down itself instead of returning control to
Rust, so none of `ShellHandler`'s locals unwind. That skips its entire `Drop` chain, not only the
frame executor: app `State`, `RenderRoot`, `TextContext`, the accesskit adapter, `FrameExecutor`
(render-thread join, final present, best-effort pipeline-cache persist), `MacosExtensions`' menu +
observer, and anything else reachable from app `State` — including cleanup that would normally ride
root-`Owner` disposal rather than a hand-rolled `Drop` (see
[docs/CODE_STANDARDS.md](CODE_STANDARDS.md) § State & Reactivity Conventions). A window-close
request under the default policy (`CloseAction::Exit`) is unaffected and does take the full
drop-chain shutdown.

**Applies to**: macOS apps quitting via ⌘Q or the app menu's Quit item.

**Why accepted**: scoped to the frame executor only — its skipped pipeline-cache persist is a
documented no-op on macOS already (Metal has no `PIPELINE_CACHE`, per `frust-shell-desktop`'s own
`cache` module docs), and nothing in the crate calls `std::process::exit`, so nothing observable is
lost there specifically. That scoping does **not** extend to the rest of the skipped chain: an app
holding an OS-resource-owning plugin handle in its `State` gets no chance to release it on this
route. `frust-camera`'s `AppleSession` (shared by macOS and iOS; its `Drop` releases the capture
device) is a real current instance of this, not a hypothetical one — a `CameraSession` reachable
from app `State` on this quit route leaves the camera device unreleased by any Rust-side cleanup.
Both routes were observed live on real hardware. A later fix would route termination through
`appkit_glue`'s `applicationShouldTerminate:` (`NSApplication.TerminateReply.terminateLater`, run
the graceful shutdown, then `terminateNow`) instead of the current predefined action.

**Evidence**: `plugins/camera/src/apple.rs` (`AppleSession`'s `Drop`, module doc's macOS-shares-the-Apple-arm
note). Runtime confirmation on a MacBook:
the `terminate:` route (menu Quit and ⌘Q, including while the window was hidden) exits without
`run` returning; the close-window route returns from `run` cleanly (exit code 0).

---

### `desktop-macos-app-menu-title-process-name` — the bold application-menu title shows the process name, not `DesktopConfig::app_name`, for unbundled binaries

**Observed**: AppKit derives the bold application-menu *title* in the menu bar from the
bundle/process name and ignores the installed `NSMenu` item's title for unbundled binaries. A
`cargo run`-style dev binary named "Frust Gate" via `DesktopConfig` shows `frust-gate` (the
process name) as the bold menu title, while every item label inside it (About/Hide/Quit Frust
Gate) correctly carries the configured name. Verified live on a MacBook (2026-08-15).

**Applies to**: unbundled macOS dev-preview launches (`cargo run` / `frust run` fallback) using a
desktop shell.

**Why accepted**: no public API sets the application-menu title for an unbundled process — it is
sourced from the process/bundle name, not from any NSMenu item. A `.app` bundle ships
`CFBundleName` (from `DesktopConfig::display_name`) in every `Info.plist`, and a MacBook run
(2026-08-17) confirmed that a `frust build macos` bundle's bold menu title reads the display name.
The shell's default About/Hide/Quit item labels follow the same name — `resolve_app_name` prefers
the bundle's `CFBundleDisplayName`/`CFBundleName` over the executable stem when no
`DesktopConfig::app_name` is set. Only the unbundled bold title remains process-named, which is
AppKit's own behavior.

**Evidence**: MacBook runtime verification of the window title and application-menu name;
`crates/frust-shell-macos/src/lib.rs` (`resolve_app_name`), `src/appkit_glue.rs`
(`bundle_display_name`).

---

### `desktop-installer-runtime-partial` — some installer formats are refused, and some have never run for real

**Observed**: `frust build <os> --installer` shells out to a pinned `cargo-packager` over an
already-assembled bundle. An app author can hit four limits:

1. **`.rpm` is not a supported output.** `cargo-packager` 0.11 builds only `.deb`/AppImage/pacman on
   Linux, so a requested `.rpm` is a typed `InstallerError::RpmNotSupported` refusal naming
   `tauri-bundler`'s documented `.rpm` path (or `alien` over the `.deb`) as the workaround.
2. **AppImage requires a square configured icon.** An absent or non-square `[desktop] icon` surfaces
   as `cargo-packager`'s own typed refusal ("Could not find a square icon to use as AppImage icon")
   rather than a bundle with no icon, unlike a plain `frust build linux`, where a bad icon only
   downgrades to a `BundleNote`.
3. **`--installer appimage` fails on a host whose `binutils` emits a `.relr.dyn` section.** The
   `strip` step of `cargo-packager` 0.11.8's vendored `linuxdeploy` rejects that relocation section,
   so the build ends in `InstallerError::PackagerFailed`. The `.deb` path is unaffected. It is an
   upstream tool mismatch, not a Frust defect, and host-conditioned: the same config shape has built
   an `.AppImage` on another host.
4. **Runs that were never made.** Windows NSIS `.exe` and WiX `.msi` assembly is unit-tested against
   a faked process runner only. A macOS `.dmg` has been built and mounted with an `Apple Development`
   identity (MacBook, 2026-08-17), but never with a `Developer ID Application` identity, and no
   `[macos] notarize = true` run has reached Apple's `notarytool`, so the packager's re-sign, its
   secure timestamp, the pre-notarization `spctl` verdict and a real notarization are unproven.

**Applies to**: `frust build <os> --installer`, and any macOS `.dmg` distributed outside the building
machine.

**Why accepted**: the `.rpm` and square-icon rules are `cargo-packager`'s own limits, each surfaced as
a typed refusal rather than a silent failure. The AppImage failure is a host tool-version mismatch
with a proven typed-error path, not an unverified code path; it closes on a host with older or
compatible `binutils`, or on an upstream `linuxdeploy` fix, neither of which this pipeline
controls. The unmade runs need a Windows installer pass and a
Developer ID identity with Apple credentials; both lanes stay opt-in and unit-tested until then.

**Evidence**: `crates/frust-drive/src/desktop_build/installer.rs` (`InstallerFormat::for_target`,
`RpmNotSupported`, the square-icon refusal test); a MacBook run of the `.dmg` lane, 2026-08-17; a
Linux run of `--installer appimage` on a host with newer `binutils` that failed in the vendored
`strip` step with `PackagerFailed` while `--installer deb` produced a genuine Debian package.

---

### `desktop-single-window` — desktop shells support exactly one window

**Observed**: `frust-shell-macos`/`-windows`/`-linux` and the shared `frust-shell-desktop` core
create and manage a single `winit::window::Window`; there is no multi-window API at the facade or
any per-OS shell.

**Applies to**: every desktop shell, on every OS.

**Why accepted**: a deliberate scope choice, not a defect — revisit only if a future winit brings first-class multi-window
support worth exposing through `DesktopConfig`.

**Evidence**: `crates/frust-shell-desktop/src/app_handler.rs` (the one `create_window` call);
`DesktopConfig` has no multi-window field.

---

### `desktop-contributions-v1-scope` — three named scope limits on the plugin desktop-contribution seam

**Observed**: three deliberate v1 narrowings on the desktop-contribution feature, each
enforced structurally rather than accidental:

1. **No Windows variant.** `Contribution` has no Windows-lane desktop variant at all —
   `desktop_build::contributions::applies_to` filters every existing variant out on a Windows
   build, by an exhaustive (non-wildcard) match naming this explicitly.
2. **Boolean-true-only entitlements.** `Contribution::MacosEntitlement` carries only a key and a
   comment — `<key>k</key>`/`<true/>` is the only shape a plugin can contribute; a value-carrying
   entitlement is not representable.
3. **Base-contributions-only detection.** `frust_drive::plugin::desktop_contributions` scans only a
   plugin's base `Contribution`s — one gated behind an opt-in `FeatureSpec` is invisible to it even
   when a project selected that feature, since `add_plugin`'s `features` argument is never
   persisted anywhere durable for this function to read back.

**Applies to**: any future plugin wanting a Windows desktop integration point, a value-carrying
entitlement, or a desktop contribution behind an optional feature bundle.

**Why accepted**: all three are named, structural v1 boundaries in the code they live in (an
exhaustive match, a doc-commented shape, a doc-commented scan scope), not gaps found later — each
is future work gated on a real need materializing (a plugin wanting the wider shape) rather than
spec'd speculatively ahead of one.

**Evidence**: `crates/frust-drive/src/desktop_build/contributions.rs`'s `applies_to` (Windows
match arm); `crates/frust-drive/src/plugin/mod.rs`'s `Contribution::MacosEntitlement` doc comment;
`crates/frust-drive/src/plugin/apply.rs`'s `desktop_contributions` doc comment.

---

### `native-typeface-first-publish-latch` — a mid-process design-system-to-design-system face swap doesn't reach native controls until relaunch

**Observed**: `plugins/native-widgets`' typeface resolution (theme ladder L3) is extension-first,
per slot, and re-publishes host-side (`api::theme`'s `PublishGuard`) whenever the active
(button, body) face pair changes — including a live design-system swap. The *platform* halves do
not follow: Android's `set_glyph_bytes` is a `OnceLock::set`, caching each resolved face object
process-wide once registration succeeds; iOS and macOS share one `coretext.rs` module (`apple`'s
`fonts` is a re-export of it, so the two Apple arms cannot drift from each other) with the same
`OnceLock`-published-bytes/cached-descriptor shape, just thread-local rather than process-wide for
the descriptor cache (`CTFontDescriptor` is not `Sync`). On all three platforms, only the *first*
(button, body) pair a process ever publishes actually reaches a native `Button`/`Label`/`Switch`.
A later design-system swap re-publishes the new bytes from the host side with no error, but the
platform keeps rendering the first system's faces until the process relaunches.

Scope: the ladder's step-3 (`Typeface::System`) arm publishes no bytes (`&[]`), and an all-empty
pair never crosses the platform seam (`publish_font_bytes`'s own skip), so an extension-less resolve
latches nothing and a subsequently installed design system's pair becomes the process's first real
publish and registers correctly. The gap is platform-side only: a **mid-process swap from one real face pair to another** (e.g. one design
system's `install()` replaced by a second's — `frust_glyph::baseline()`'s `NativeTypefaces` attach
is the one built-in byte source today, `plugins/glyph/fonts/` — publishing a fresh pair after an
extension-only pair already latched; there is no design-language shortcut in the ladder to special
case here, see NATIVE_WIDGETS_ARCHITECTURE.md's theme ladder) still does not re-register on device
until relaunch.
A half-filled extension (one slot real, one slot `System`) is not itself a new instance of this
gap — the empty slot resolves to `System` in that same resolve, so nothing ever asks the platform
half to register bytes for it; the gap only resurfaces if a *later* resolve wants real bytes for
that same slot, which is the ordinary swap case above.

**Applies to**: the platform halves of the typeface ladder on Android, iOS, **and macOS** — the
AppKit arm resolves L3 through the same `coretext.rs` module the iOS arm re-exports
(`plugins/native-widgets/src/appkit/mod.rs`'s module doc, "L3's CoreText half is the shared
`crate::coretext`"), so a mid-process design-system swap latches on macOS exactly as it does on
iOS.

**Why accepted**: widening the platform halves to re-register on a swap is a platform-side change
with its own device gate, not a host-side one — deferred rather than blocking this feature.
**Also owed**: custom-face rendering (either system's) has never been exercised on real Android/iOS
hardware — this entry covers both the swap gap and that outstanding device gate.

**Evidence**: `plugins/native-widgets/src/api/theme.rs`'s module doc ("Publishing: last-pair-wins,
not once-per-process" and "System publishes nothing"); `plugins/native-widgets/src/android/fonts.rs`
module doc (`OnceLock`/process-wide cache notes); `plugins/native-widgets/src/coretext.rs`'s module
doc (the shared-module rationale and the thread-local descriptor cache, consumed identically by
`crate::apple::fonts`' iOS re-export and `crate::appkit`'s macOS call sites).

---

### `native-widgets-macos-image-cover-letterboxes` — `Fit::Cover` degrades to a letterbox on macOS instead of cropping

**Observed**: `native_image`'s `Fit::Cover` means "fill the box, keep aspect, crop the overflow" —
Android's `ScaleType.CENTER_CROP` and iOS's `UIViewContentMode.ScaleAspectFill` (plus a
`clipsToBounds` normalization) both implement it exactly. `NSImageScaling` has no equivalent
constant: the only scale-and-keep-aspect option, `ScaleProportionallyUpOrDown`, fits the whole
image inside the box rather than cropping the overflow. The macOS arm maps `Fit::Cover` to that
same constant it uses for `Fit::Contain`, so a `Cover`-fit image on macOS is letterboxed (visible
background around the shorter axis) rather than cropped to fill — the only one of the four `Fit`
variants where macOS's rendered result differs from Android's and iOS's.

**Why accepted**: a real crop-and-fill needs custom drawing (`NSImageScaling` has no primitive for
it), which this control does not do — a platform capability gap recorded per-platform rather than
corrected onto the platform that lacks it (`docs/PLUGINS_CODE_STANDARDS.md`'s Plugin Conventions).

**Evidence**: `plugins/native-widgets/src/controls/image.rs`'s `Fit::image_scaling` doc and its
`NSImageScaling` mapping table (the `Fit::Cover` row explicitly marked "Degraded, not equivalent").

---

### `native-widgets-macos-slider-no-drag-edges` — `DragStart`/`DragEnd` are never emitted by `native_slider` on macOS

**Observed**: Android (`onStart/StopTrackingTouch`) and iOS (`TouchDown`/`TouchUpInside|Outside`
UIKit control events) both report a slider drag's gesture edges; macOS's `NSSlider` target-action
has no counterpart — an `NSControl` sends its one action on every drag step with no phase
information, so `FrustNativeControlTarget::detail_for` refuses `EVENT_KIND_DRAG_START`/
`EVENT_KIND_DRAG_END` outright rather than guessing one from the value stream. `native_slider` on
macOS therefore only ever delivers `ValueChanged` events; an app whose slider handler relies on
`DragStart`/`DragEnd` (e.g. to suspend other work while scrubbing) sees no signal at all on macOS.

**Why accepted**: recovering the edges would need inspecting `NSApp.currentEvent`'s type inside the
action or overriding `NSSliderCell`'s tracking methods — a new `NSEvent` dependency and a second
target-action class — and a synthesized edge from the value stream alone would be a guess, worse
than an honest absence.

**Evidence**: `plugins/native-widgets/src/appkit/events.rs`'s module doc ("Drag start/end are never
emitted on macOS") and its `the_drag_edges_are_refused_never_synthesized` test;
`plugins/native-widgets/src/controls/slider.rs`'s macOS `platform` module doc ("Which event kinds
this arm emits").

---

### `native-widgets-macos-imageio-dyld-shadow` — `native_image` silently shows nothing when the host process's ImageIO codecs are DYLD-shadowed

**Observed**: when the running process's `DYLD_LIBRARY_PATH` has replaced one of ImageIO's private
codec dylibs (see [DEVELOPMENT.md](DEVELOPMENT.md)'s Known Issues for the mechanism and the
developer-side fix), `native_image`'s macOS arm detects the shadow once and refuses every later
`NSImage` decode rather than crashing the process: the slot's view is left empty, and exactly one
process-wide warning names the shadowing file. An app has no API-level way to detect this
degrade — it looks identical to an ordinary undecodable-payload result, the same empty-view
degrade every arm already gives a bad image (`BitmapFactory.decodeByteArray`/`UIImage.imageWithData:`
failing return the same clear-and-warn outcome) — so a QA pass on a contaminated dev machine can
read as "images don't work" with no code-level cause.

**Why accepted**: the alternative is letting the process SIGBUS on first decode, which is strictly
worse; fixing the shadow is a developer-environment problem outside this plugin's own control
(`DEVELOPMENT.md`'s workaround), and the degrade-not-crash contract matches every other undecodable-
payload case this control already handles.

**Evidence**: `plugins/native-widgets/src/controls/image.rs`'s macOS `platform` module doc
("`DYLD_LIBRARY_PATH` can take ImageIO's codecs away") and its `shadowed_codec`/`shadowing_file`
functions and their host tests.

---

### `native-widgets-segmented-stepper-apple-only` — `native_stepper` is Apple-only; `native_segmented` and `show_native_sheet` have Android arms, `native_tab_bar` stays iOS-only

**Observed**: `native_stepper` is registered in `APPLE_KINDS`
(`plugins/native-widgets/src/controls/mod.rs`), not `SHARED_KINDS`/`MATERIAL_KINDS` — it has no
Android `NativeWidget` impl, nor a Linux/Windows/web one; Android's framework has no
increment/decrement control and a composite (two `ImageButton`s plus a `TextView`) is a named
follow-up. `native_segmented` is in both `MATERIAL_KINDS` (Android: `MaterialButtonToggleGroup`) and
`APPLE_KINDS`; `show_native_sheet` has an Android arm (`present::android_sheet`, a
`BottomSheetDialog`) beside iOS's. Each app-facing builder resolves the rest at compile time:
`SEGMENTED_ARM` is `true` on iOS/macOS/Android and `STEPPER_ARM` on iOS/macOS only
(`plugins/native-widgets/src/api/builders.rs`); elsewhere the builder renders the frust-drawn
`RefusalBanner` naming the missing arm. The sheet resolves through `UnsupportedSheet`
(`PresentError::Unsupported`) on macOS, Linux, Windows and web. `native_tab_bar` (a bare
`UITabBar`, `IOS_ONLY_KINDS`) is still iOS-only — macOS has no bottom-tab idiom; on
Android the tab bar is the chrome's `BottomNavigationView` (`native_scaffold`), not this control.

The Android sheet has one draggable half ratio: `BottomSheetBehavior` holds a single half-expanded
ratio, so of several half detents the user drags between the selected (else smallest) one and the
full height (`SheetHandle::select_detent` reaches the others); `largest_undimmed` and
`scrolling_expands` are ignored (the dialog dims at every height, the scroll view always hands
nested scrolls to the behavior). One live presentation supersedes the previous: showing a sheet or
alert takes down any alert or sheet on screen and resolves its abandoned resolver.

**Why accepted**: the stepper composite is follow-up work, and no Linux/Windows/web arm exists for
the segmented control or the sheet (a macOS `NSPopover` arm is also follow-up); the compile-time
refusal covers every such target uniformly. The Android embedding ships AndroidX and the plugin's
Gradle module takes Material Components (`com.google.android.material:material`, the pin row in
`docs/PLUGINS_DEVELOPMENT.md`), whose theme requirement is
`native-chrome-android-material3-theme-required`.

**Evidence**: `plugins/native-widgets/src/controls/mod.rs`'s kind tables and their source-scanning
parity test; `controls/segmented.rs`'s *The Android arm*, `controls/stepper.rs`'s
module doc and `controls/tab_bar.rs`'s *No macOS arm, no Android arm*;
`api/builders.rs`'s `SEGMENTED_ARM`/`STEPPER_ARM`/`TAB_BAR_ARM`; `present/mod.rs`'s *Platform arms*
and `present/android_sheet.rs`; `plugins/native-widgets/README.md`'s controls table.

---

### `native-chrome-android-material3-theme-required` — Android chrome, sheet, segmented and the toolbar FABs need a Material3-derived Activity theme

**Observed**: the Material views are built under the Activity's manifest theme
(the Kotlin-internal `FrustMaterialContext.wrap`), because `create_control`'s night-qualified context
carries the framework default theme. A stock theme (e.g. `Theme.Material.NoActionBar`, the scaffold
default) makes Material's theme enforcement throw: the chrome install answers `INSTALL_REFUSED` →
`ChromeError::Refused`, the slot pins the generation to the frust-drawn fallback bars and logs the
fix once; a segmented control logs and takes the host's dead-slot path; the sheet fails with
`PresentError::Platform`. Never a crash.

The Material chrome also costs a landscape phone roughly half its height to the top bar and
`BottomNavigationView`; there is no `NavigationRail` arm yet.

**Applies to**: Android apps whose Activity theme derives from neither `Theme.Material3.*` nor
`Theme.MaterialComponents.*`.

**Why accepted**: Material's own enforcement is the only reliable signal, and the fallback keeps
the app usable; the fix is one theme parent in the app.

**Evidence**: `plugins/native-widgets/src/android/chrome.rs`'s *Install, refusal and the theme
requirement*; `controls/segmented.rs`'s Android arm; `platform/android/build.gradle.kts` (THEME
REQUIREMENT); device gates on iPhone SE (iOS 26.7.1), Xiaomi 12 (Android 16), Pixel 5 (Android 14)
and macOS; release APK cost: the Material pin alone is +116,211 B (the figure in
`platform/android/build.gradle.kts`); the whole gate app (Material plus chrome code, demo code and
gate icons) is +1.44 MB universal against a build without them (resources.arsc +594 KB, dex +166 KB,
arm64 .so +141 KB), and R8 binds.

---

### `native-chrome-android-bitmap-icons` — Android chrome icons are bitmaps; SF Symbols and menu-item icons are not drawn

**Observed**: Android tab and action icons travel as PNG bytes (`setIcon`); an Apple SF Symbol has
no Android rendering, so a symbol tab or action shows its title alone and `Leading::Symbol` shows
no leading item (encoded `"none"`). A bottom item has one icon (a tab's selected-state icon is
unused) and a menu item's icon is not shown.

**Applies to**: `native_scaffold` on Android.

**Why accepted**: there is no cross-platform symbol vocabulary to map; apps supply bitmaps for
Android, and the title keeps every item reachable.

**Evidence**: `plugins/native-widgets/src/android/chrome.rs`'s *Icons* and
`chrome/mod.rs`'s `Leading::Symbol` doc.

---

### `native-chrome-single-surface-containment` — one frust surface serves every chrome tab; iOS navigation controllers have no stack

**Observed**: the iOS chrome re-roots the window into a `UITabBarController` of
`UINavigationController`s and moves the one frust `UIViewController` into the selected page, so tab
state lives in the frust tree, not in per-tab UIKit stacks: there is no native push animation and
`Leading::Back` is a custom item reporting `BackPressed`. Tab selection is native and optimistic:
a tap selects at once and stays pending for a ~250 ms settle window; if the app's spec names
another tab by then, the bar snaps back to it, so an app must confirm in `on_tab_select`. While a
toolbar is shown the iOS tab bar is hidden.

**Applies to**: iOS `native_scaffold`. Android and the frust-drawn fallback are fully controlled
(no optimism, no settle).

**Why accepted**: frust owns routing and its single surface cannot be duplicated per
tab; native selection is what keeps the Liquid Glass lens animation. The toolbar hide is chosen
because frust pushes never hide the tab bar the native way and the toolbar would sit behind it.

**Evidence**: `plugins/native-widgets/src/apple/chrome.rs`'s *Selection*, *Toolbar* and
*Limitations* sections; a device gate on iPhone SE (iOS 26.7.1).

---

### `native-tab-bar-bare-no-liquid-glass` — `native_tab_bar` is a bare `UITabBar`, so it never gets iPadOS 18+'s top placement or the Liquid Glass scroll-edge effect

**Observed**: `native_tab_bar` builds and drives a bare `UITabBar` directly from Rust —
deliberately never a `UITabBarController` (`plugins/native-widgets/src/controls/tab_bar.rs`'s module doc, *A bare bar, never a
`UITabBarController`*): frust owns screen ownership and routing, so the bar reports a tap and
nothing more. iPadOS 18's top-placed tab bar and the floating glass bar both live on
`UITabBarController`'s adaptive presentation, which a bare `UITabBar` has no access to — on iPadOS
this control stays a bottom bar with no Liquid Glass regardless of OS version, exactly as
`plugins/native-widgets/README.md`'s tab-bar paragraph already states. The delegate wiring
(`UITabBarDelegate.tabBar:didSelectItem:` on the one target class,
`plugins/native-widgets/src/apple/events.rs`) is the whole surface UIKit gives a bare bar —
nothing there can opt into the controller-level chrome either.

**Applies to**: `native_tab_bar` on iPadOS, every OS version; the bar always renders at the bottom,
never atop content, and never gains the translucent glass material a `UITabBarController` gets
automatically on iPadOS 18+/26.

**Why accepted**: adopting `UITabBarController` to get the adaptive placement would hand it screen
and navigation ownership frust's own `Router` already owns — the whole
reason this control is a bare bar in the first place. Nothing about the bare-bar shape can be
patched onto the controller-only chrome without reversing that decision.

**Evidence**: `plugins/native-widgets/src/controls/tab_bar.rs`'s module doc, *A bare bar, never a
`UITabBarController`*; `plugins/native-widgets/README.md`'s tab-bar paragraph ("On
iPadOS the bare bar stays at the bottom and gets no Liquid Glass — the iPadOS 18 top tab bar and
the floating glass bar are `UITabBarController` features");
`plugins/native-widgets/src/apple/events.rs`'s `UITabBarDelegate`/`tabBar:didSelectItem:` wiring.

---

### `native-widgets-component-same-kind-children-indistinguishable` — a `NativeComponent` cannot tell two same-family children's events apart

**Observed**: `ComponentCtx::attach_listener` binds a view to the slot's own id and a
`ListenerKinds` family (click, toggled, value changed); the event it later delivers to
`NativeComponent::on_event` is a `NativeEvent` carrying only a `kind` and a primitive `detail` — no
view or child identity (`plugins/native-widgets/src/component.rs`). Two children of the same
component attached for the same family therefore report identically: a click is `(KIND_CLICK, 0)`
whichever child produced it, so `on_event` cannot distinguish them. The shipped `DemoCard` (this
plugin's own non-default `demo-components` feature) demonstrates the consequence directly: its
**secondary** button is deliberately left unwired, because wiring both buttons to `CLICK` would
make their events indistinguishable — only the primary button's clicks are attached and forwarded.

**Applies to**: any `NativeComponent` implementor that attaches more than one child to the same
`ListenerKinds` family. The documented escape is to give each child a distinct family, or to mount
each as its own slot (a separate `NativeComponent`/builder) rather than as children of one.

**Why accepted**: `NativeEvent`'s wire shape is the same `(slotId, kind, detail)` triple every
built-in control's listener already uses (`crate::events`) — widening it to carry a child/view
identity would mean a new field on the shared wire and every listener class, not a
`NativeComponent`-only fix, and no production component has needed to tell same-family children
apart yet.

**Evidence**: `plugins/native-widgets/src/component.rs`'s `Bridge::on_event` doc comment (the event
gate, and its note that a fabricated id naming a slot that attached a family is "indistinguishable
from the real listener — the runtime asks nothing about which object fired") and `NativeEvent`'s
`kind`/`detail` fields (no view identity); `plugins/native-widgets/src/demo.rs`'s module doc,
*Event wiring: the primary button reports its clicks* (the secondary button is deliberately left
unwired because "two children attached for the same family are indistinguishable in `on_event`").

---

### `native-widgets-android-date-picker-range` — Android's `DatePicker` resolves a missing bound to its own 1900–2100 range and clamps into it; the Apple arms do not

**Observed**: `android.widget.DatePicker`'s documented default and usable range is 1900-01-01
through 2100-12-31 (`date_picker.rs`'s Android `platform::BOUNDS`, built from `PLATFORM_MIN`/
`PLATFORM_MAX`). A missing `min`/`max` resolves to that range on Android and to this crate's own
unbounded `CivilDate::MIN`/`CivilDate::MAX` on iOS/macOS (`Bounds::APPLE`). At plan time
(`Bounds::effective_range`, called from `DatePickerProps::plan`), an explicit bound outside the
calling arm's own range — or a date outside the resolved range — is clamped into it, never left to
reach a platform setter as an out-of-range or inverted pair; `warn_if_clamped` logs one warning per
process — a static once-guard, the same shape as the stepper's non-positive-step warning — naming
the first offending slot, the clamped values, and the arm's range. On Android the initial date
handed to `DatePicker.init` is the same effective (clamped) date the plan uses for `updateDate`,
through `DatePickerProps::effective_date`. Because this clamp is arm-resolved rather than the
platform-agnostic clamp `DatePickerProps::decode` already applies to an explicit `min > max`, an
app that sets, say, a minimum date after 2100 gets a single-day range on Android (the floor
collapses onto the ceiling) while iOS/macOS honour the same value unclamped.

**Applies to**: `native_date_picker`'s `min`/`max` builders (`NativeDatePickerView::min`/`max`) on
Android specifically; iOS and macOS resolve the same bounds unbounded.

**Why accepted**: the range is the framework's own — `DatePicker`'s calendar/spinner presentation
sizes itself from `max - min` — and never handing it an inverted or out-of-range pair is the safe,
Apple-first contract this crate commits to elsewhere (module doc's *Range*): clamp to what the
platform will actually hold rather than asking it to reject or silently reinterpret an illegal
range.

**Evidence**: `plugins/native-widgets/src/controls/date_picker.rs` — `Bounds::APPLE`, the Android
`platform::BOUNDS`/`PLATFORM_MIN`/`PLATFORM_MAX`, `Bounds::effective_range`, `DatePickerProps::plan`,
`warn_if_clamped`, the module doc's *Range* section, and the tests
`an_explicit_min_above_androids_ceiling_clamps_the_bound_and_the_date`,
`an_explicit_max_below_androids_floor_clamps_the_bound`, and
`the_same_out_of_androids_range_bounds_are_not_clamped_on_apple`; `plugins/native-widgets/src/api/builders.rs`'s
`NativeDatePickerView::min`/`max` rustdoc.

**Workaround**: keep app-supplied bounds inside 1900-01-01..2100-12-31 where Android matters.

---

### `native-widgets-android-component-listener-disarm` — releasing a `NativeComponent`'s `ListenerHandle` on Android disarms the listener object, not the view's interface

**Observed**: Android's `View.setOn*Listener` setters each hold exactly one listener, replace it
outright, and expose no getter — so a release can never confirm it still holds the listener it
created, and nulling the interface could silently wipe a newer listener a later
`ComponentCtx::attach_listener` call set on the same view. `ListenerHandle`'s Android `Drop` (and
the equivalent explicit `ComponentCtx::detach_listener`) therefore never calls a `setOn*Listener`
setter: it obtains a JNI env from the process VM, checks for a pending Java exception first
(skipping the disarm and logging if one is pending, since no JNI call is safe with one pending), and
otherwise calls `FrustNativeListener.disarm()` through `NativeCtx::disarm_listener` — flipping that
one instance's `@Volatile armed` flag, which every overridden callback checks before reporting —
then releases both global references regardless of the outcome. A disarmed instance is left set on
the view: it stays there, inert, until a later `attach_listener` call replaces it or the view itself
is destroyed. The one place nulling a listener interface remains is `android/ctx.rs`'s
`unwind_partial_attach`, reached only from inside the same `attach_listener` call that just set
those interfaces synchronously moments earlier — nothing else can have replaced them in between, so
nulling is identity-safe there and nowhere else.

**Applies to**: any `NativeComponent` implementor on Android calling `ComponentCtx::attach_listener`
more than once for the same view, or relying on `detach_listener`/a handle's drop to stop a listener
from ever firing again on the platform side. The disarm mechanism itself is exercised on a device
gate, not by a host-only `cargo test` — the host arm's `ListenerInner` stand-in has no `armed` flag
to disarm.

**Why accepted**: the alternative — nulling the view's interface on release — is exactly the
regression a review caught: it can wipe a newer listener a later attach already installed. The cost
of disarming instead is one inert Java listener object per released handle, kept alive by the
view's listener field — not by any JNI global reference, both of which the drop releases — until a
later attach replaces it or the view is destroyed.

**Evidence**: `plugins/native-widgets/src/component.rs` — `ListenerHandle`'s doc, the Android
`ListenerInner` struct, and `impl Drop for ListenerHandle` under `target_os = "android"`;
`plugins/native-widgets/src/android/ctx.rs`'s `disarm_listener` and `unwind_partial_attach`;
`plugins/native-widgets/platform/android/src/main/kotlin/dev/frust/nativewidgets/FrustNativeListener.kt`'s
class doc, `armed`, and `disarm()`.

---

### `native-widgets-alert-busy-slot-unobserved-host-teardown` — a presenting host torn down through a path the arm cannot observe leaves the Busy slot held indefinitely

**Observed**: both Apple alert arms document a teardown path their resolution mechanism cannot see: macOS's `NSWindowWillCloseNotification` fires only on a real window close, not `orderOut:` (hidden, not closed — `appkit_alert.rs`'s *Not observed*); on iOS the app replacing the presenting controller chain outside UIKit's own dismissal path removes the alert with no callback the arm can see (`apple_alert.rs`'s *Not observed*). In both cases the presentation stays pending in `present::ACTIVE` (the process-wide Busy slot) until `present::dismiss` resolves it `Dismissed` or the caller drops the `Presentation` future; every later `show_native_alert`/`show_native_alert_into` answers `PresentError::Busy` until then (a Busy refusal is logged at warn with the live presentation's generation and age).

**Applies to**: `show_native_alert`/`show_native_alert_into` on macOS and iOS/iPadOS — an app that hides rather than closes its presenting window (macOS `orderOut:`) or replaces a presenting controller chain outside UIKit's dismissal path (iOS) while an alert is live.

**Why accepted**: no watchdog by design — a long-lived alert (a slow user decision) is legitimate, so a timeout would misfire on the common case to guard the rare one; `dismiss` and dropping the future are the caller's two ways out.

**Evidence**: `plugins/native-widgets/src/present/appkit_alert.rs` *Not observed*; `plugins/native-widgets/src/present/apple_alert.rs` *Not observed*; `plugins/native-widgets/src/present/mod.rs` module doc (the contract's slot-release bullet).

**Workaround**: close (not hide) the presenting window on macOS while an alert may be live; call `dismiss` before tearing down a presenting controller chain on iOS.

---

### `native-widgets-android-alert-theme-from-activity` — the Android alert's light/dark appearance follows the hosting `Activity`'s theme, not the app's `Brightness`

**Observed**: `FrustNativePresenter.kt` builds the dialog with `AlertDialog.Builder(activity)`; the framework dialog's light/dark styling resolves from the hosting `Activity`'s own theme attributes, not from `frust::Theme`'s `Brightness`, unlike the native-widgets controls (whose theme ladder reads `frust::Theme` directly).

**Applies to**: `show_native_alert`/`show_native_alert_into` on Android.

**Why accepted**: the arm is framework-only by decision (no AppCompat/Material theming), and `android.app.AlertDialog` offers no per-dialog brightness without a themed context this crate does not construct.

**Evidence**: `plugins/native-widgets/platform/android/src/main/kotlin/dev/frust/nativewidgets/FrustNativePresenter.kt` (`AlertDialog.Builder(activity)`); `plugins/native-widgets/src/present/android_alert.rs` module doc.

---

### `native-widgets-alert-drop-leaves-platform-ui` — dropping a `Presentation` frees the Busy slot but leaves the platform alert or sheet on screen

**Observed**: dropping `Presentation<T>` releases the process-wide Busy slot (a new request — alert or sheet — may present at once) but does not dismiss the live platform UI. An alert stays on screen until the next request displaces it (the displacing arm takes it down first) or the user answers it, and that answer is discarded because nothing is listening. A sheet behaves the same way: dropping its future frees the slot, not the sheet, so a newer request (of either kind) can find the older sheet still live; whatever its kind, a displaced presentation goes down through the shared `LivePresentation::dismiss` seam and the successor presents from that dismissal's completion (`apple_host.rs`'s *The live-presentation guard*; `apple_sheet.rs`'s *A displaced presentation*).

A mid-interactive-dismissal race is accepted rather than closed: a successor request arriving while the user's swipe is still animating a displaced sheet away resolves it at once (`DisplacedAction::ResolveAndContinue`, `apple_host.rs`) without waiting for that animation, so the successor's own presentation attempt can find UIKit still mid-transition and resolve `PresentError::NoHost` instead of showing; and if the user's swipe is then cancelled, the old sheet is back on screen with no live entry to answer it — removable only by another swipe, not by a further programmatic `dismiss`.

**Applies to**: any caller that drops the future returned by `show_native_alert` (or the request behind `show_native_alert_into`) without calling `dismiss`, on all three alert arms; the same holds for `show_native_sheet` (or `show_native_sheet_into`) on its one arm, iOS/iPadOS.

**Why accepted**: documented module contract (`present/mod.rs`: dropping the future frees only this module's bookkeeping); tearing down live platform UI from a dropped future would need arm-specific plumbing for a case the API already covers with `dismiss`.

**Evidence**: `plugins/native-widgets/src/present/mod.rs` module doc; `apple_alert.rs` / `appkit_alert.rs` / `android_alert.rs` and `apple_sheet.rs`'s *A displaced presentation* sections.

---

### `native-sheet-ipad-regular-width-detents` — a page sheet in a regular-width iPad window ignores detents

**Observed**: UIKit presents a page sheet in a regular-width iPad window as a centered form sheet at a fixed size, not at any of its configured detents; only an edge-attached presentation — compact width, or compact height with `prefersEdgeAttachedInCompactHeight` — rests at its detents. The arm configures `sheetPresentationController.detents` regardless (they take effect the moment the window turns compact — Slide Over, Split View) and never fakes parity between the two idioms.

**Applies to**: `show_native_sheet`/`show_native_sheet_into` on iPadOS whenever the presenting window is in regular width (full-screen or a large Split View pane).

**Why accepted**: the API never promises detent parity across idioms; it is a UIKit presentation-controller rule, not a bug this crate's arm can route around without abandoning `UISheetPresentationController` (and the page-sheet chrome that comes with it) for a hand-built modal.

**Evidence**: `plugins/native-widgets/src/present/apple_sheet.rs`'s module doc, *iPad in regular width ignores detents*; `plugins/native-widgets/src/present/mod.rs`'s `Detent` doc.

**Workaround**: none — design sheet content to read well at both a compact edge-attached presentation and a regular-width centered form sheet; never rely on detent changes firing on an iPad in regular width.

---

### `catalog-input-timers-follow-time-scale` — some catalog input timers slow under a non-1.0 animation time scale

**Observed**: `frust-widgets` times input on `PaintCtx::input_time()`, the raw shell clock, so long-press, double-tap windows, caret blink, velocity stamps and auto-scroll ignore `set_animation_time_scale`. Some catalog widgets still time input on `frame_time()`, the scaled animation clock, so under a scale other than 1.0 their gesture and dwell timing stretches or shrinks with the animation.

**Applies to**: (a) input timers still on `frame_time()`: beui components/tooltip hover open delay, beui blocks/otp_input caret blink, glyph toast auto-dismiss hold, material snackbar auto-dismiss hold, and the material slider and range-slider haptic scheduler (borderline); (b) velocity and fling machinery that stores `last_frame_time`: material dismissible, carousel, sheet and overlay/modal; glyph sheet; shadcn message_scroller and overlay/modal; beui agents/message_scroller — only while the time scale is not 1.0 (a devtools `set_animation_time_scale` call or an app's own `set_animation_time_scale`).

**Why accepted**: the scale is a debugging and test lever, default 1.0, and the affected timers behave correctly at 1.0; moving each remaining widget's input timers is a per-widget change that is not a framework gap.

**Trigger for removal**: the listed catalogs read `input_time()` for their input timers and `frame_time()` only for animation drivers.

**Evidence**: `crates/frust-core/src/widget.rs` (`PaintCtx::frame_time` / `input_time` docs); the `frame_time()` reads in the listed catalog widgets' input paths.

---

### `hover-window-leave-standing` — hover and press chrome outlive the state that caused them, until the next in-window `Move`

**Observed**: four deliberate, test-pinned v1 gaps share one cause — nothing re-derives hover
without a fresh uncaptured pointer `Move`, and the desktop shell delivers no window-leave event.
A pointer leaving the window keeps the last-hovered widget's tint standing (the shell's
`CursorLeft` arm only ends an OS file drag, so nothing clears it); a `Down` that ends a hover with the pointer then held still shows no
tint for the press itself until the next `Move`; a click's `Up` likewise ends the link, so a mouse
resting where it clicked shows no hover tint until it moves again; and a scroll or any other
mutation happening under a stationary pointer leaves stale hover/pressed chrome in place rather
than re-testing the pointer's position against whatever moved. All four resolve themselves on the
next in-window pointer `Move`, which is what makes them a standing-until-next-move gap rather than
a stuck one.

**Applies to**: every desktop shell (`frust-shell-desktop`); any widget using
`EventCtx::claim_hover`/`PaintCtx::is_hovered`. **Mobile is affected too, in one shape**: each
contact carries its source (`PointerId`/`PointerSource`, read through `EventCtx::pointer_id`), but
the hover pass does not consult it and both mobile shells map a touch drag to `PointerPhase::Move`,
so the hover refusal is structural only for a **captured** pointer. A touch drag that captured nothing — a finger sliding
over a non-capturing hover consumer — is an ordinary hover pass and does tint it. The tint is
transient: the lift's `Up` ends the link, which is why `Up` is a hover-ending pass at all.

**Why accepted**: hover is deliberately derived only from a real pointer `Move`, with no
synthetic re-derivation pass, because the alternative (a per-frame position re-test independent of
input) would give hover its own polling loop the rest of the pipeline doesn't have. A window-leave
event is a real, addressable gap (winit exposes `CursorLeft`) but is not wired to hover; the
rest are accepted properties of the epoch model, not shell gaps. The touch residual is accepted
because the tint is transient: the `Up` rule already bounds it to the length of the gesture.

**Evidence**: `crates/frust-core/src/app/tests/hover.rs`'s hover pipeline tests (`a_captured_move_cannot_claim_hover`,
`a_down_up_or_cancel_ends_the_hover`, `a_non_pointer_pass_leaves_a_live_hover_standing`);
the hover pipeline (2026-08-17, commit `68ac7e93`) listed this among its known v1 gaps;
`crates/frust-core/src/event/pointer.rs` (`PointerSource`/`PointerId`).

---

### `cursor-stale-until-next-move` — a torn-down or outrun cursor request keeps its last shape until the next `Move`

**Observed**: `RenderRoot::cursor()` only re-resolves on a pointer `Move` (captured included); a
widget that is torn down while its request stands, or that the pointer scrolls/moves out from
under while stationary, leaves the last-resolved shape in place rather than falling back to
`CursorIcon::Default` immediately. The next pointer `Move` re-resolves it correctly — this is the
cursor's own version of `hover-window-leave-standing`'s standing-until-next-move window, on the
same root cause (nothing re-derives without a real `Move`).

**Applies to**: every desktop shell; any widget using `EventCtx::set_cursor`.

**Why accepted**: re-resolving without a pointer `Move` would mean tracking every requester's
liveness independently of input, the same cost the hover model above declines to pay; the residual
is cosmetic (a stale shape, never a stuck-captured pointer) and self-corrects on the next motion.

**Evidence**: `crates/frust-core/src/app/session.rs`'s `RenderRoot::cursor()` doc comment ("Residual:
a widget that is torn down … leaves the last shape in place until the next `Move`");
the cursor API (merged 2026-08-17, `49bc7657`) listed this among its limitations.

---

### `overlay-portal-v1-scope` — six named narrowings in the framework overlay portal

**Observed**: `frust::overlay_portal` and `frust::authoring::OverlaySlot` float a pod above the
whole app — owner-hosted, root-painted after the main tree, root-routed ahead of it — and the v1
seam declines six things, each named in its own source:

1. **No focus restore.** A modal pod (`OverlayKeys::Modal`, or a consuming outside tap) masks
   main-tree shortcuts, confines Tab traversal to itself, and ends a main-tree focus session on the
   paint that registers it, but nothing returns focus to the opener on dismiss and the pod's first
   focusable is not auto-focused (`overlay-no-auto-focus-on-appear`), so after the modal closes the
   next Tab starts from the first stop. Traversal also never stops in a Transparent pod, a
   non-Current navigator page or an unselected `IndexedStack` child.
2. **Declined-key forwarding carries keys only, on request.** Key, IME and edit-command events
   stay focus-routed (`InputEvent::is_focus_routed`). A declined `Key` reaches a pod only when its
   owner asked for the focus session's declined keys (`EventCtx::forward_declined_keys_to`) or, for
   an Escape, when the pod is modal; IME and edit commands are never re-offered, and PageUp/PageDown
   cannot be forwarded at all because `NamedKey` has no such keys.
3. **The catalogs' own hosts are not on it.** All six `overlay::anchored`/`overlay::modal` hosts
   across `frust_shadcn`, `frust_material` and `frust_beui` still place and paint their surfaces
   in-tree; `frust_shadcn::tooltip`/`hover_card`, `frust_beui::tooltip`, and
   `frust_material::tooltip`/`rich_tooltip` are the catalogs' only components riding the portal so
   far.
4. **A pod is invisible to inspection and to assistive technology.** It is not reached by
   `Widget::visit_children` unless its owner chooses to visit it, so `RenderRoot::inspect` — the
   devtools widget tree — sees the owner and not the floated surface; and the portal publishes no
   semantics for it, because a pod's nodes would attach under the owner's node at the owner's
   position rather than at the floated rect. An assistive-technology user reaches a floated surface
   through the owner's own node (see `selection-verbs-advertised-not-invocable` for the state of
   the text field's half of that). **For `frust_material::tooltip`/`rich_tooltip` this is a
   regression, not only a scope note.** Before either panel rode the portal, both mounted through
   the in-tree `crate::overlay::anchored` host and were reached by the ordinary widget-tree
   semantics walk, so a screen reader learned the plain panel's label and the rich panel's action
   buttons (its title and supporting text were never wired to semantics, ported or not — only the
   action row was). Floated through the portal instead, neither owner (`TooltipWidget`,
   `RichTooltipWidget`) declares its panel a semantics child, so a screen reader reaches none of
   that today; the regression holds for both panels regardless of input class — the plain panel
   registers `OverlayInput::Transparent` (skipped by hit-testing outright, the same class
   `frust_beui::tooltip` rides), while the rich panel registers `OverlayInput::Interactive` with a
   non-consuming outside-tap so its action row keeps routing and a tap elsewhere still dismisses it
   — neither classification changes whether the portal walks a pod's semantics, which it does not,
   for any registered pod, in v1. The panels' own `semantics` methods are otherwise untouched by
   the port and still push those same nodes — dead code today that resumes for free the moment this
   restriction lifts, the same shape `frust_beui::tooltip` deliberately restores after an interim
   revision of its own port had dropped the push entirely; `frust_material`'s was simply carried
   over unchanged and was never touched by the migration.
5. **Every overlay pointer event walks the whole tree.** A hit on a registered rect dispatches
   `InputEvent::Overlay` as a broadcast, and a broadcast is forwarded to every child
   unconditionally — no hit test, no capture fast path, no focus gate — so one press inside a
   floated surface costs a full tree walk. The key side shares the cost: every declined-key offer
   and every `InputEvent::Shortcut` candidate is a broadcast pass of its own.
6. **An owner torn down by a structural rebuild cannot animate out.** The registry is per paint
   pass, so a surface lives exactly as long as its owner keeps registering it, and an exit ramp is
   "keep registering while it runs" — which an owner the rebuild has unmounted cannot do. This is
   the portal-side statement of the same framework gap
   `shadcn-anchored-exit-needs-kept-mounted` records from the catalog side.

Two smaller consequences of the same per-pass registry: **no nesting** (a registration made from
inside a floated pod's own paint is dropped, so a menu opening a submenu registers both surfaces
from the one owner) and **no hover inside a pod** (the root marks a hover pass on a hit-tested
event only, and an overlay event is a broadcast).

**Applies to**: every caller of `frust::overlay_portal` or `frust::authoring::OverlaySlot` — today
the baseline `TextInput`'s selection toolbar, the `frust_widgets::drag::draggable()` ghost,
`frust_shadcn::tooltip`/`hover_card`,
`frust_beui::tooltip`, and `frust_material::tooltip`/`rich_tooltip`. Item (3) applies to the three
catalogs' own hosts, which is where most floated surfaces still live.

**Why accepted**: each is a seam the first callers do not need, and each is cheaper to add once a
second caller states its shape than to guess at now. (3) in particular is migration work with no
behaviour riding on it — the catalog hosts work, and moving them is a port rather than a fix.
(5) is a real cost rather than a correctness gap: the walk happens per overlay pointer event, not
per frame, and the broadcast is the only route that reaches an owner without knowing where it sits.

**Evidence**: `crates/frust-core/src/overlay.rs`'s "Not in v1" section (focus trap, inspection,
nesting), its declined-key paragraph and its per-pass registry contract;
`crates/frust-core/src/app/session.rs`'s `trap_modal_focus` ("Not restored when the modal closes");
`crates/frust-core/src/app/dispatch.rs`'s `dispatch_declined_key` and `focus_stops`; `crates/frust-widgets/src/overlay.rs`'s
own "Not in v1" section (semantics, hover, IME); `crates/frust-core/src/event/input.rs`'s
`InputEvent::Overlay` routing contract (the broadcast); `plugins/shadcn/src/components/tooltip.rs`,
`plugins/beui/src/components/tooltip.rs`, and `plugins/material/src/tooltip.rs` module docs (all
titled "Riding the framework portal") against the six unported
`plugins/{shadcn,material,beui}/src/overlay/{anchored,modal}.rs` hosts;
`plugins/material/src/tooltip.rs`'s `TooltipWidget`/`RichTooltipWidget` `semantics` methods and
their pre-port use of `crate::overlay::anchored` (for item (4)'s regression above).

**Trigger for removal**: per item — focus restore and auto-focus for a modal pod, forwarding for IME,
edit commands and the missing named keys, the catalog hosts
ported onto `OverlaySlot`, an overlay-aware semantics and inspection path, and a keyed dispatch
that reaches an owner without a full walk.

---

### `selection-toolbar-no-handles-magnifier-v1` — the framework selection route ships the bar alone: no drag handles, no magnifier

**Observed**: under `SelectionToolbarPolicy::Framework` a field's selection affordance is the
floated toolbar and nothing else. There are no draggable handles at the selection's ends, so the
only pointer route that adjusts a selection is a drag still belonging to the press that made it
(by word after a long-press, by cluster otherwise) — once the finger is up, a selection can be
re-made but not adjusted, since the next primary `Down` puts the bar away and starts over. A
hardware keyboard's Shift+arrows still extend one. And there is no magnifier loupe over the caret or the grab point,
so on a touch device the finger covers exactly what it is positioning. Neither surface exists
anywhere in the widget tier: this is absence, not degradation.

**Applies to**: every platform on the framework route — Android, desktop and web. iOS is
unaffected: it selects `SelectionToolbarPolicy::Native` and gets UIKit's own handles and loupe.

**Why accepted**: handles and a loupe are each a rendered, hit-tested, platform-flavoured surface
in their own right, and the bar is what makes the clipboard verbs reachable at all — the distance
between "no way to copy" and "copy works, adjusting a selection does not" is the one worth closing
first. A design system that wants them is not blocked: the builder seam replaces the whole floated
view.

**Evidence**: `crates/frust-widgets/src/selection_toolbar.rs` (the entire baseline view — a pill of
verb buttons and nothing else); `crates/frust-widgets/src/textinput.rs`'s "Selection gestures and
the toolbar" section, which enumerates the four gestures that reach a selection, none of them a
handle drag.

**Trigger for removal**: a handle and loupe surface in the widget tier, which needs a second
floated-surface owner per field and a magnifier able to sample the painted scene.

---

### `drag-ghost-pod-no-semantics` — a drag ghost publishes no accessibility node and is invisible to `inspect()`

**Observed**: `frust_widgets::drag::draggable()` floats its ghost through the overlay portal
(`OverlayAnchor::Window`, band `Tooltip`, input `Transparent`), which is v1's floated-pod scope
exactly: no semantics are published for a registered pod and it is not reached by
`RenderRoot::inspect()`/`WidgetTree::inspect()` unless its owner visits it (see
`overlay-portal-v1-scope`'s item 4 above). The drag's actual semantics live on the in-tree
`Draggable`/`DragTarget` nodes instead — a `Role::Button` source labelled `"Draggable"` idle,
`"Dragging"` while a pointer session is live and `"Drop"` while its own keyboard session is, a
`Role::Group` target labelled `"accepts drop"`/`"drop target"` — so an assistive-technology user
never reaches the ghost itself, only the source and target it moves between. The richer per-verb
surface those nodes would ideally advertise (`accesskit::Action::CustomAction` for
`lift`/`drop`/`cancel` individually) is not wired to anything either: `frust-core`'s
`perform_accessibility_action` matches only `Action::Click`/`Action::Focus` and drops a custom
action's id (the same gap `selection-verbs-advertised-not-invocable` records for `TextInput`).
Both nodes' `Click` are genuine drop verbs, not mere activations: the target's own primary `Up`
handler drops a live, accepting *keyboard* session (a pointer session's `Up` belongs to the
source that captured it and passes through every ancestor target untouched), and the source
answers a primary `Down` while it drags its own keyboard session exactly the way Enter/Space
would — drop onto whatever is hovered, cancel when nothing is — whether that `Down` is a real
press or the synthesized half of
`perform_accessibility_action`'s `Down`+`Up` pair; no synthetic-origin marker is needed for either
reading, since a real press on the lifted item is just as honest a "put it down". Idle, the same
`Click` reaches the source as a plain tap instead, same as any other control. The keyboard chord
(Enter/Space to lift and drop, the arrows to cycle, Escape to cancel) remains the primary
assistive-technology path through the whole walk; `Click` on the source now reuses its drop/cancel
half rather than only ever cancelling.

**Applies to**: every `frust_widgets::drag::draggable()`/`drag_target()` consumer, including
`reorderable_list()` (built on both) — every platform, since the gap is in the shared overlay and
accessibility-action seams, not a per-shell one.

**Why accepted**: the ghost is a visual affordance only; the session's real state already has an
accessible home on the source/target nodes, so publishing a second, floated accessibility node for
the ghost would be a duplicate read rather than new information. Wiring a richer custom-action set
needs the same framework-wide accessibility-action seam change `selection-verbs-advertised-not-
invocable` is waiting on, not a drag-specific fix.

**Evidence**: `crates/frust-widgets/src/drag/draggable.rs`'s "Semantics" and "Ghost and source
feedback" module-doc sections; `crates/frust-widgets/src/drag/target.rs`'s "Click-to-drop and
semantics" section; `crates/frust-core/src/overlay.rs`'s per-pass registry contract (no semantics,
no `inspect()` visibility for any registered pod).

**Trigger for removal**: the same accessibility-action seam widening `selection-verbs-advertised-
not-invocable` names, after which a custom `lift`/`drop`/`cancel` action could reach these nodes
too.

---

### `drag-keyboard-cycle-visible-targets-only` — keyboard cycling only reaches a target visible in the latest paint pass

**Observed**: `DragCoordinator::move_to_next_target`/`move_to_previous_target` — the
`ArrowRight`/`ArrowDown`/`ArrowLeft`/`ArrowUp` half of `draggable()`'s keyboard chord — cycle
through the registry a `drag_target()` populates with its own bounds each paint, so a target a
`ListView`/`ScrollView` has scrolled out of view that frame is simply absent from the cycle; no
step scrolls an off-screen target into view to make it reachable. A keyboard user can therefore
move a lifted item only between whichever targets of a list taller than its viewport are
currently on screen.

**Applies to**: every `frust_widgets::drag::draggable()`/`drag_target()` consumer whose targets
sit inside a scrollable container taller than its viewport — `reorderable_list()` included.

**Why accepted**: resolving a keyboard-cycled hover is already gated on visibility to avoid
dropping onto a target whose bounds are stale or hidden; widening that to scroll a hidden target
into view is a separate, larger feature (an auto-scroll driven by the keyboard cycle rather than
the pointer).

**Evidence**: `crates/frust-widgets/src/drag/draggable.rs`'s "Lift, cycle, drop" module-doc
section; `crates/frust-widgets/src/drag/coordinator.rs`'s target registry, rebuilt from visible
bounds each paint.

**Trigger for removal**: a scroll-into-view hook on keyboard cycling, so a step that would land on
a registered-but-off-screen target scrolls it into view first.

---

### `file-drop-desktop-only` — mobile and web shells publish no OS file-drop signal, and an unregistered region never opens a session

**Observed**: `InputEvent::FileDrop` is published only by `frust-shell-desktop`, which maps
winit's `HoveredFile`/`DroppedFile`/`HoveredFileCancelled` window events onto it (see
SHELLS_ARCHITECTURE.md); Android, iOS and web surface no equivalent OS signal, so
`frust_widgets::drag::drag_target::<Vec<PathBuf>>` never engages on those hosts. On desktop, a file
drag hovering a window region with no `DragTargetWidget` anywhere in its hit-test path never opens
(or updates) an `ExternalFiles` session, since nothing calls `handle_file_drop` for it; a session
already open over one target keeps that target's last resolved hover while the drag crosses such a
region. The drag still always ends: a `Drop` or `Cancel` that reaches no target is followed by the
root's `FileDropPhase::Ended` broadcast, which ends the session (a `Drop` there cancels — nothing
accepts it), so in-app drags are never left blocked behind a finished OS drag. Every `Hover`, `Drop`
and `Cancel` is resolved at the shell's last in-window cursor position, never a position winit's
file events carry (they have none) — and on a platform that sends no `CursorMoved` during an OS drag
that is wherever the cursor was last seen in the window before the drag, not the point the files
were released over.

**Applies to**: every `frust_widgets::drag::drag_target()` typed over `Vec<PathBuf>`, on every
platform for the mobile/web gap, and on desktop for the unregistered-region gap.

**Why accepted**: mobile and web is a platform-signal gap, not a framework omission — neither OS
embedding surfaces a drag-and-drop event to this framework today. The unregistered-region gap is
the coordinator's own registry-not-hit-test design working as specified: resolution reads the
registry a target reports at paint, and a region with nothing registered has nothing to resolve
against.

**Evidence**: `crates/frust-core/src/event/input.rs`'s `FileDropEvent` "Source" and "Broadcast follow-up"
doc sections; `crates/frust-widgets/src/drag/target.rs`'s "OS file drops" module-doc section ("The
drag always ends", "Known limit"); `crates/frust-shell-desktop/src/app_handler.rs`'s
`FileDropAccumulator`, `FileHoverLatch` and the
`WindowEvent::HoveredFile`/`DroppedFile`/`HoveredFileCancelled` arms.

---

### `file-drop-overlay-surface-unreachable` — an OS file drop over a floated overlay surface never reaches a drop target hosted in it

**Observed**: an OS file drop whose pointer lands inside a floated overlay surface never reaches a
drop target hosted in that surface: file-drop events are routed by the main tree's hit test, not
through the overlay registry, so the surface's own targets see no `Hover`, `Drop` or `Cancel`.

**Applies to**: a `frust_widgets::drag::drag_target()` typed over `Vec<PathBuf>` and hosted inside
a floated overlay surface, on the desktop shells (the only shells that publish `FileDrop`).

**Why accepted**: a file drop is a hit-tested main-tree event by design, and no shipped floated
surface hosts a file-drop target: the portal's current callers float tooltips, a hover card, the
selection toolbar and a drag ghost.

**Evidence**: `crates/frust-core/src/app/dispatch.rs`'s file-drop handling and `route_overlay`
(which passes `FileDrop` straight through); `crates/frust-core/src/event/input.rs`'s `FileDropEvent`.

**Trigger for removal**: overlay-aware file-drop hit testing.

---

### `selection-toolbar-labels-english-v1` — the baseline toolbar's four labels are English, always

**Observed**: the framework-drawn toolbar reads its labels from one fixed table — "Cut", "Copy",
"Paste", "Select all" — and never localises them. A Japanese or Arabic app on the framework route
gets English verbs, in English order: the pill lays its items out left to right with no RTL
mirroring. The same four strings are what the field publishes as the labels of its accesskit
custom actions, so a screen reader reads them out in English too.

**Applies to**: every platform on the framework route (Android, desktop, web) whose app or design
system has not installed a toolbar builder of its own. iOS is unaffected — UIKit localises its own
edit menu.

**Why accepted**: localisation is deferred to the builder seam by design rather than missing — a
design system or an app installs a translated view through
`frust_core::set_selection_toolbar_builder`, and replacing this baseline that way is the documented
path. Baking a string table into `frust-widgets` would put a translation surface in the one crate
with no locale to resolve it against: `frust-i18n` is a plugin, and the widget tier does not depend
on it.

**Evidence**: `crates/frust-widgets/src/selection_toolbar.rs`'s "Labels" section and its `LABELS`
table ("This baseline never localises its own four labels");
`crates/frust-widgets/src/textinput.rs`'s custom-action labels beside `A11Y_CUT_ID`.

**Trigger for removal**: a locale seam the widget tier can read, or a localised builder shipped by
each design system — which closes it for that catalog's apps only, not for the baseline.

---

### `selection-toolbar-mouse-hold-opens` — a held mouse press opens the selection toolbar, because the field's long-press ignores the contact source

**Observed**: `EventCtx::pointer_id` reports whether a contact is a mouse or a touch, but the text
field's long-press does not consult it and arms on any primary `Down`, so holding a mouse button
still inside a field for the long-press threshold selects the word under it and raises the toolbar,
which no desktop platform does. The desktop gesture actually meant to open it (a secondary press)
works as well: this is an extra route, not a missing one.

**Applies to**: desktop and any mouse-driven host on the framework route. `drag::draggable()` does
read the source and needs a hold only for touch; the field does not. The entry is scoped to the text field: the generic
gesture detector (`crates/frust-widgets/src/gesture.rs`) does not consult the source either, so its
`on_long_press` and `on_hold_progress` also fire for a held mouse.

**Why accepted**: the wrong behaviour costs a selection the user can dismiss, never an edit.

**Evidence**: `crates/frust-core/src/event/pointer.rs`'s `PointerSource`/`PointerId`;
`crates/frust-core/src/event/ctx.rs`'s `EventCtx::pointer_id`;
`crates/frust-widgets/src/textinput.rs`'s "Selection gestures and the toolbar" section (the
long-press arms on a primary `Down`, with no source consulted);
`crates/frust-widgets/src/drag/draggable.rs` (`DragPolicy::Auto`).

**Trigger for removal**: the field's long-press gates on `PointerSource::Touch`.

---

### `text-input-read-only-not-copyable` — a read-only field cannot be selected from or copied

**Observed**: `TextInputView::read_only(true)` makes a field non-interactive through the same focus
gate `enabled(false)` uses — it never takes focus — and a field that never holds focus never
receives a selection gesture, an `EditCommand` or a toolbar. So a read-only field's text cannot be
selected, copied, or read out verb-by-verb by an assistive client, even though it is live,
undimmed and visually ordinary. Material 3 and Apple's HIG both keep a read-only field focusable
and copyable; this widget deliberately does not, and says so in place.

**Applies to**: every `frust::TextInputView` with `read_only(true)` on every platform, and every
design-system field wrapping one (`frust_material::text_field` and the shadcn/beUI equivalents
inherit it). A `enabled(false)` field is not covered: it is meant to be inert.

**Why accepted**: read-only reuses the focus gate rather than growing a parallel one, which is what
keeps "interactive?" and "dimmed?" independent and makes a field turned read-only while focused
release exactly like one turned disabled. A copyable read-only field needs a third state —
focusable and selectable, but refusing every mutation — threaded through that same gate and through
every handler that reads it, which is a design rather than a flag.

**Evidence**: `crates/frust-widgets/src/textinput.rs`'s "Read-only mode" section and
`TextInputWidget::interactive` (`enabled && !read_only`), plus that module's clipboard-verb section
("a read-only field cannot be copied from — Material 3 and the HIG would keep it focusable, and
this widget deliberately does not").

**Trigger for removal**: a focusable-but-immutable mode on the field, gating mutation instead of
focus.

---

### `textinput-no-text-decoration` — a text field draws no underline or line-through

**Observed**: `TextStyle.decoration` (underline, line-through) is painted only by `TextView`. A
`TextInputView` accepts a decorated style for its content and its placeholder, but its paint path
never asks the layout for decoration rectangles, so no line is drawn under or through the text.

**Applies to**: every `frust::TextInputView` and every design-system field wrapping one, on every
platform.

**Workaround**: draw the line with a separate `TextView` over a read-only presentation, or leave the
field undecorated.

**Why accepted**: a decoration line crosses the caret, the selection highlight and the composing
underline, and which layer wins at each crossing is an open design question rather than a missing
call.

**Evidence**: `crates/frust-widgets/src/text.rs` (`TextWidget` paints `TextLayout::decoration_rects`)
against `crates/frust-widgets/src/textinput.rs`, which has no decoration paint.

**Trigger for removal**: the field paints `TextLayout::decoration_rects` for its content and
placeholder, with the caret and selection interplay settled.

---

### `selection-verbs-advertised-not-invocable` — the field publishes Copy/Cut/Paste/Select all to a screen reader, and none of them can be invoked

**Observed**: the floated toolbar is a pointer affordance and the portal publishes no semantics for
the pod it floats, so the baseline `TextInput` publishes the four clipboard verbs on its **own**
accessibility node instead, as accesskit custom actions — accesskit models none of the four
natively, so each is a stable id plus a label the client reads out. Only enabled verbs are offered
and the offer never depends on the bar being up, so the advertisement itself is correct. **It
cannot be acted on.** A platform adapter reports an invoked custom action as an
`accesskit::ActionRequest` carrying `Action::CustomAction` plus the id in its `data`; the
shell-to-core seam `AppTree::perform_accessibility_action` forwards only `(node_id, action)` and
drops `data`, and `RenderRoot::perform_accessibility_action` models `Click` and `Focus` and nothing
else. A screen-reader user therefore sees four verbs on the field and gets silence from all four —
in one respect a worse failure than never advertising them, since the field's own rule is that an
action offered and then refused is worse than one never offered, and this is that case one layer
down. It is recorded here rather than left silent for exactly that reason. (2026-09-12)

**Applies to**: every platform with an accessibility adapter — the desktop shells and both mobile
bridges alike, since the seam that drops `data` is the shared one. The field's own half is
complete: every verb already has a route into its command handler the moment one arrives as an
`InputEvent::EditCommand`, which is precisely what a shell dispatches for a platform edit menu.

**Why accepted**: closing it is not a widget change. `RenderRoot::perform_accessibility_action`,
`AppTree::perform_accessibility_action` and the `Widget` trait have to widen together — the seam
must carry the action's data, the root must route a custom action to the node that published it,
and a widget needs a hook to receive one — which is a framework-wide seam change rather than
something a text field can do from inside itself.

**Evidence**: `crates/frust-widgets/src/textinput.rs`'s "The clipboard verbs and assistive
technology" section ("What is still missing is the dispatch, and it does not live here") and its
`A11Y_CUT_ID` neighbours' `set_custom_actions` publication;
`crates/frust-shell-common/src/app_tree.rs`'s `AppTree::perform_accessibility_action` signature;
`crates/frust-core/src/app/semantics.rs`'s `RenderRoot::perform_accessibility_action` (a `Click` arm, a
`Focus` arm, nothing else).

**Trigger for removal**: the three widened together, proven by a custom action invoked from a real
screen reader landing as an `EditCommand` in the focused field.

---

### `shadcn-hover-overlays-touch-inert` — tooltip and hover-card never open on a touch device

**Observed**: `frust-shadcn`'s `tooltip`/`hover_card` open only through the shared hover latch
(`TooltipHover`, a trigger-rect-plus-open-flag `Rc<Cell<_>>` written from a hover-timing pass, not
an event). Touch input has no hover, so neither component has any path that opens it; a
long-press-opens-a-tooltip/hover-card gesture is not modelled.

**Applies to**: `frust_shadcn::tooltip`/`hover_card` on Android, iOS, and any touch-driven desktop
input; every other anchored/modal component in the catalog opens on a press and is unaffected.

**Why accepted**: matches shadcn/ui's own upstream behavior (its tooltip/hover-card are hover-only
on the web too); a long-press affordance would be new interaction design, not a port, and is
deferred rather than invented speculatively.

**Evidence**: `plugins/shadcn/src/components/tooltip.rs` module docs ("Touch" section — "Touch has
no hover, so a tooltip never opens on a touch device. Long-press-opens-a-tooltip is not
modelled.").

---

### `shadcn-select-no-typeahead-fixed-height` — select/combobox have no type-ahead and a fixed dropdown height

**Observed**: a v1 gap in `frust-shadcn`'s `select`/`combobox`, named in the component's own source.
Radix's jump-to-typed-match behavior is not ported; `SELECT_MAX_HEIGHT` is a 300px constant rather
than measured from the host's available space (no such measurement reaches a plugin-tier widget
today).

**Applies to**: `frust_shadcn::select`/`combobox`.

**Why accepted**: a named, structural v1 boundary at the point in the source it lives (a doc comment
and a named constant), not a gap found by later testing; deferred interaction work with no
upstream-parity requirement forcing it into v1.

**Evidence**: `plugins/shadcn/src/components/select.rs` module docs ("No type-ahead" and
`SELECT_MAX_HEIGHT`).

---

### `overlay-no-auto-focus-on-appear` — the framework has no seam to claim focus (widget or IME) on mount

**Observed**: the framework exposes no auto-focus-on-appear hook a widget can call on mount —
`EventCtx::request_focus` is reachable only from a widget's own event pass, which never runs for
something nobody has touched yet. Two independent consequences: (1) neither `frust-shadcn`'s or
`frust-beui`'s `modal`/`anchored` hosts, nor `frust_material::dialog`, claim focus when they
appear — each claims focus only in response to a `Down` inside itself, so Escape (wired to fire
only once the host holds focus) does nothing until a caller completes one pointer interaction with
the overlay first; (2) `frust::TextInputView` has no `autofocus`/`request_focus` builder either, so
`frust_material::search`'s full-screen view — whose upstream reference opens with `autoFocus: true`
to raise the keyboard immediately — instead opens with no IME up until the user taps its field once.

**Applies to**: `frust_shadcn::overlay::modal`/`anchored` and every component built on them
(dialog, alert-dialog, sheet, drawer, command, popover, dropdown/context menu, select, combobox);
`frust_beui::overlay::modal`/`anchored` and every beUI component built on them (same list shape,
see PLUGINS_ARCHITECTURE.md's Design-System Plugins); `frust_material::dialog` (same gap);
`frust_material::search`'s header field (IME-autofocus manifestation — no widget anywhere can claim
focus programmatically on mount, text input included).

**Why accepted**: a framework-level focus-management primitive (claim focus, or the text-input-
specific `autofocus`, on mount) does not exist yet; every current design-system caller works around
it the same documented way rather than inventing a per-crate special case. Closing the IME half
needs a `frust-widgets` seam on `TextInputView` (an `autofocus` builder or a reachable
`request_focus`); closing the general half needs a framework-wide claim-on-mount hook.

**Evidence**: `plugins/shadcn/src/overlay/modal.rs` and `plugins/shadcn/src/overlay/anchored.rs`
module docs ("there is no auto-focus-on-appear hook in the framework");
`plugins/beui/src/overlay/anchored.rs` module docs (same line, "there is no auto-focus-on-appear
hook in the framework"); `plugins/material/src/search/mod.rs` module doc's "IME: no programmatic
focus" section (explicitly names this as "the same framework gap").

---

### `no-plugin-reachable-deferred-state-callback` — a plugin-tier widget cannot queue a state-bearing callback onto a later frame

**Observed**: the framework's `mark_pending_result_flush`/`take_pending_result_flush` seam that
lets a widget defer a callback across frames is `frust-core`-internal — not re-exported through
`frust::authoring` (`frust_widgets::authoring` re-exports the shape it wants callers to match, not
the seam itself). A plugin-tier widget that needs to fire a callback carrying `&mut State` from a
non-event pass (e.g. a hover-delay timer observed only in `paint`) has no way to reach it, and so
cannot hold app state at all for that decision. `frust-shadcn`'s hover tooltip/hover-card is built
around this exact hole: the open/close decision lives entirely in a widget-local, non-reactive
`Rc<Cell<_>>` latch instead of app state, specifically because a resting pointer sends no event to
carry `&mut State` through and the framework offers no alternate route to queue one. `frust-beui`'s
`todo_list`/`agent_activity` hit the rebuild-driven variant of the same hole: upstream's
`collapseOnComplete` reports the auto-collapse through a callback the instant it fires; a rebuild
dispatches no event either, so each port flips its own internal open flag silently instead of
reporting it, and a host that must know passes the flag itself (`TodoListView::open`/
`AgentActivityView::open`) rather than reading a callback.

**Applies to**: any plugin-tier widget wanting to drive app state from a non-event pass — a
paint-clock-driven delay (`frust_shadcn::tooltip`/`hover_card`) or a rebuild-driven state flip
(`frust_beui::todo_list`, `frust_beui::agent_activity`) alike.

**Why accepted**: the workaround (a shared, non-reactive latch plus, for shadcn, a pod floated
through the framework overlay portal in its input-transparent tooltip band; an owned, host-driven
flag for beUI's two lists) fully covers each current caller's needs; widening the public seam is
framework-level work with no caller it blocks today.

**Evidence**: `plugins/shadcn/src/components/tooltip.rs` module docs ("the framework exposes no way
for a plugin-tier widget to queue a state-bearing callback onto the next frame");
`crates/frust-widgets/src/authoring.rs` (`mark_pending_result_flush`'s shape, not the function
itself, documented there); `crates/frust/src/lib.rs`'s `authoring` module (no
`mark_pending_result_flush` re-export); `plugins/beui/src/agents/todo_list.rs` module docs ("The
automatic collapse reports nothing").

---

### `shadcn-bubble-fixed-to-unit-state` — `bubble()` cannot be embedded directly in a `View<State>` tree for `State != ()`

**Observed**: every other leaf component in `frust-shadcn` is generic over `State` (even the
non-interactive ones carry an unused `State` bound so they compose directly into any app's tree);
`bubble()`'s `BubbleView` instead implements `View<()>` only — its sibling `bubble_group()` in the
same module is properly `State`-generic. An app with `State != ()` cannot place a `bubble()` as a
direct child; it needs a `Component`-boundary indirection (a nested component whose own `State` is
`()`) to embed one.

**Applies to**: `frust_shadcn::bubble` only — no other component in the 55-component catalog has
this asymmetry, `message`/`message_scroller` included.

**Why accepted**: a chat bubble has no callbacks to carry `State` for in the first place; the fix
(a `State`-generic signature matching every sibling) is a small, low-risk cleanup with no
functional gap behind it — tracked here as a catalog ergonomic inconsistency, not a capability
gap.

**Evidence**: `plugins/shadcn/src/components/bubble.rs` (`impl View<()> for BubbleView` versus
`impl<State: 'static> View<State> for BubbleGroupView<State>` in the same file).

---

### `shadcn-control-ladder-under-touch-floor` — every shadcn control height sits below the 44px tap-target floor

**Observed**: `frust-shadcn`'s entire control-height ladder — `HEIGHT_XS`/`HEIGHT_SM`/
`HEIGHT_DEFAULT`/`HEIGHT_LG` (24/32/36/40 logical px, `plugins/shadcn/src/style.rs`) — sits below
the 44px mobile tap-target convention, including `HEIGHT_LG`, the roomiest rung the ladder offers.
There is no larger size to opt into and no density mechanism that widens one.

**Applies to**: every sized control in the 55-component catalog that reads the ladder (button,
input, select trigger, and every component built on them); every platform this crate targets,
touch and pointer alike — the metrics are fixed, not resolved per input modality.

**Why accepted, and permanent by design**: port fidelity to shadcn/ui's exact metrics is a hard
requirement of this external-origin catalog — changing the visual heights is not on the table. The
crate's own charter is "desktop-first, mobile-friendly" (`plugins/shadcn/src/lib.rs`, Charter):
shadcn's metrics are kept as-is, touch *correctness* (press states, scrim taps, scrolling) is
required, a separate mobile design is not, and no density mechanism is invented — deliberately
distinct from the framework's overall mobile-first charter. The ladder is pinned by test
(`control_heights_are_the_shadcn_size_ladder`, which asserts every rung, including `lg`, stays
under the 44px floor), so this cannot regress silently or drift toward "fixed" over time. An
extended-hit-area mechanism — hit-testing beyond a control's visual bounds on touch — was evaluated
and is not cheaply available today: `frust-core`/`frust-widgets` expose no min-target convention or
hit-padding seam a plugin-tier widget could opt into. If one ever lands framework-wide, this catalog
is a natural adopter; that is the remedy path, recorded here rather than invented per-catalog.

**Evidence**: `plugins/shadcn/src/style.rs` (`HEIGHT_XS`/`HEIGHT_SM`/`HEIGHT_DEFAULT`/`HEIGHT_LG`
doc comments and the `control_heights_are_the_shadcn_size_ladder` pinning test);
`plugins/shadcn/src/lib.rs`'s Charter section ("Desktop-first, mobile-friendly").

---

### `shadcn-sidebar-v1-residuals` — three named v1 narrowings in the shadcn sidebar port

**Observed**: three deliberate v1 gaps in `frust_shadcn::sidebar`, each named in the component's own
source rather than found later:

1. **No mobile sheet fallback.** Upstream swaps the whole panel for a `Sheet` under a 768px media
   query; there is no media/size seam a view can branch on before layout here, so the port is
   desktop-only — a mobile app composes `frust_shadcn::sheet` itself.
2. **No icon-mode tooltips.** `SidebarMenuButton`'s upstream `tooltip` prop shows the label in a
   hover card while the rail is collapsed to icon width; skipped for now (the label still serves as
   the button's accessible name, so a collapsed rail is not mute, just unlabelled visually).
3. **No composed sub-menu disclosure.** The module docs recommend composing `collapsible` with
   `sidebar_menu_button` as the trigger and `sidebar_menu_sub` as the content for a collapsible
   sub-menu, but the demo's attempt found the menu button's own press handling eats the trigger
   press before `collapsible` sees it — the documented composition does not actually toggle, and the
   demo ships its sub-list permanently open instead.

**Applies to**: `frust_shadcn::sidebar` only.

**Why accepted**: (1) and (2) are named, structural v1 boundaries with a documented workaround; (3)
is a composition gap tracked here pending a fix to the button/collapsible press interaction, rather
than a hand-rolled sub-menu special case.

**Evidence**: `plugins/shadcn/src/components/sidebar.rs` module docs ("Not in this port" — mobile
sheet, icon-mode tooltips, sub-menu disclosure); the demo has no composed disclosure for `sidebar_menu_sub`, so the
sub-list is permanently open there.

---

### `shadcn-anchored-exit-needs-kept-mounted` — a framework-level anchored overlay's exit ramp requires the app to keep it mounted

**Observed**: an `anchored`-host overlay drives its exit ramp from a builder-level `.open(bool)`,
not from mount/unmount — the framework has no seam for keeping a conditionally-mounted view alive
past the rebuild that unmounts it, so an exit animation is only reachable for a host the app mounts
*unconditionally* and toggles closed via `open(false)` (the *kept-mounted pattern*). An app that
instead mounts the host only while its own flag is set and drops it when the flag clears gets the
entrance ramp but no exit — the widget is gone by the next frame, so any in-flight ramp is simply
truncated. This is a framework gap, not a per-catalog one: `frust_shadcn::overlay::anchored`
originated it, `frust_material::overlay::anchored` (a sibling port, ported rather than
depended on) hits the identical gap and cites this same id as "the framework-wide gap it is", and
`frust_beui::overlay::anchored` (an independently-authored third instance of the same shape, not a
port of either) hits it too.

**Applies to**: every component built on any of the three catalogs' `overlay::anchored` host —
shadcn's popover, tooltip, hover-card, dropdown/context menu, select, combobox; material's menu
(incl. submenu), dropdown, and the search view's docked panel; beUI's popover, context menu,
citations' preview, and the dropdown panels of select/combobox/multi_select. Neither material's nor
beUI's tooltip is built on this host any longer — both now float through the framework overlay
portal instead (see `overlay-portal-v1-scope`). Each catalog's `modal` host is unaffected, since its
exit is staged through the navigator's own pop-result/back-press machinery (shadcn, material) or
through `StagedPop`'s own depth-guarded close (beUI) instead of a mount flag.

**Why accepted**: this is the framework-level trade the pattern makes explicit, not an oversight — a
kept-mounted host costs one layout of its content per frame while closed and nothing else, which all
three catalogs accept as the price of a real exit ramp with no framework support for outliving a
rebuild.

**Evidence**: `plugins/shadcn/src/overlay/anchored.rs`, `plugins/material/src/overlay/anchored.rs`,
and `plugins/beui/src/overlay/anchored.rs` module docs ("Mounting, and what an exit animation
costs"/"Mounting, and what an exit costs" — "The framework has no seam for keeping a
conditionally-mounted view alive past the rebuild that unmounts it").

---

### `shadcn-otp-table-button-api-gaps` — two named API-surface gaps in `table` and `button`

**Observed**: two deliberate v1 narrowings, each named in the component's own source:

1. **`table`'s header/footer are label strings, not views.** `TableView::header`/`footer` take
   `Vec<String>`, since upstream's head/footer cells are markup this port never generalized to
   arbitrary content. A tri-state "select all" checkbox or a sortable-header control therefore
   cannot live in the header row itself — the demo's data-table page fakes one by prepending a
   normal body-styled row instead, at the cost of the header's own chrome and semantics.
2. **`button` has no icon-view slot.** `ButtonSize::Icon`/`IconSm`/`IconLg` size a button to a fixed
   square, but the label is a plain `String` with nowhere to put an icon view — an icon-only button
   (e.g. a row's `⋮` menu trigger) has to fake it with a literal glyph character.

**Applies to**: `frust_shadcn::table` and `button` respectively.

**Why accepted**: each is a named v1 narrowing recorded at the point it was found rather than a
regression; a real fix (view-typed table header/footer cells, an icon-view button slot) is
plugin/framework follow-up work with no caller forcing it in yet.

**Evidence**: `plugins/shadcn/src/components/table.rs`'s header/footer type (`Vec<String>`);
`plugins/shadcn/src/components/button.rs`'s `ButtonView::label: String` field.

---

### `shadcn-components-not-ported` — six upstream shadcn/ui components are not portable as-is, one is deferred

**Observed**: the shadcn/ui v4 registry sweep behind this catalog found seven components with no
port plan, split into two classes:

- **Deferred**: `calendar` — depends on `react-day-picker`; porting it needs its own date-widget
  design plus a new dependency, and version pins are LAW here, so it is deferred rather than rushed.
- **Not portable as-is**, each for a dependency this catalog cannot carry: `menubar` and
  `navigation-menu` (built on Radix primitives with no frust analogue), `form` (built on
  `react-hook-form`), `sonner` (wraps an external toast library), `chart` (wraps `recharts`), and
  `direction` (a Radix RTL context provider with nothing to provide to).

**Applies to**: `frust_shadcn` — these seven names do not exist anywhere in the 55-component catalog.

**Why accepted**: each is a named dependency boundary, not an oversight — every one of the six
"not portable" components is a thin wrapper over a JS-ecosystem library or a Radix-only primitive
with no frust equivalent to port against; `calendar`'s gap is scope (a real date-widget design) and
policy (no drive-by dependency addition), not a dependency wall, so it is the one candidate for a
future round rather than a permanent exclusion.

**Evidence**: the component sweep deferred `calendar` and marked menubar, navigation-menu, form,
sonner, chart and direction as not portable as-is.

---

### `beui-components-not-ported` — sixteen `shader_background` variants, and named gaps across eight other components

**Observed**: `frust-beui` (beUI v2 port) leaves several upstream pieces out, each named at the
point it was found:

- `shader_background` ports 5 of 21 upstream shader slugs (`mesh-gradient`, `dot-grid`, `waves`,
  `static-radial-gradient`, `static-mesh-gradient`); the other 16 —
  `grain-gradient`, `dot-orbit`, `warp`, `water`, `voronoi`, `swirl`, `smoke-ring`, `neuro-noise`,
  `metaballs`, `god-rays`, `spiral`, `dithering`, `pulsing-border`, `color-panels`,
  `simplex-noise`, `perlin-noise` — are `DEFERRED_VARIANTS`: each is a distinct hand-written WGSL
  fragment program not yet written. The five that *are* ported are semantic
  approximations of `@paper-design/shaders-react`'s GLSL, not pixel ports — that source is neither
  vendored nor readable from here — and `dot-grid`'s cell/dot metrics are device pixels (the shader
  reads the target's texel resolution, not the window's scale factor), where upstream's grid is CSS
  px.
- `smooth-scroll`/`scroll-to`'s Lenis provider is not ported at all — frust owns its own scroll
  physics (`crate::physics`, see WIDGETS_ARCHITECTURE.md) and a second physics engine bolted on top
  would fight it; `scroll_to` ports only the eased-value half (see
  `scroll-view-no-external-offset-seam` below).
- `message_bubble` ships no `MessageBubbleCollapsible` (needs a line-cap-over-arbitrary-subtree or
  a mask brush, neither of which the framework's text stack exposes) and no `render`-slot/
  interactive bubble.
- `message_scroller` ships no preview rail (no seam to read a rendered child subtree's text back
  out of a widget tree).
- `prompt_input` ships no model selector and no prompt-actions popover.
- `streaming_response` ships no completion action row (needs a clipboard seam the framework does
  not publish) and no sources disclosure (upstream folds `citations`' components into its footer;
  here `citations` composes above `streaming_response` instead, as its own slug).
- `approval_card` ships the review surface but not the multi-step question wizard (a distinct,
  larger surface left out of the port).
- `button` ships no `ButtonLink` — frust has no anchor element, so upstream's link-flavoured
  button variant is out; a navigation button here is a callback like any other.
- `knockout_bracket` ships no `knockout-wheel` — the registry bundles a second, 1,100-line
  radial-layout component behind the same slug; porting it would be a second component behind
  one name, so it is not carried.

**Applies to**: `frust_beui::components::shader_background` and the eight components above
(`scroll_animation`, `message_bubble`, `message_scroller`, `prompt_input`, `streaming_response`,
`approval_card`, `button`, `knockout_bracket`). Smaller feature- or prop-level "not ported" notes
elsewhere in the catalog (for example popover's hover-trigger mode, wheel_picker's tick sound,
tooltip's tap-to-toggle) are degradations of a shipped component, not a missing sub-component or
surface, and are out of scope for this entry.

**Why accepted**: each is a named dependency or scope boundary recorded at the point it was found,
not a regression — sixteen more hand-written WGSL programs (each needing its own GPU verification),
a second scroll-physics engine, a multi-step wizard surface, an anchor-element button variant, and
a second radial-layout component are all real follow-up scope, not oversights baked into the ones
that shipped.

**Evidence**: `plugins/beui/src/components/shader_background.rs` module docs and
`DEFERRED_VARIANTS`; `plugins/beui/src/components/scroll_animation.rs`,
`plugins/beui/src/agents/message_bubble.rs`, `message_scroller.rs`, `prompt_input.rs`,
`streaming_response.rs`, and `approval_card.rs` module docs' "Degradations" sections;
`plugins/beui/src/components/button.rs:40-42`; `plugins/beui/src/blocks/knockout_bracket.rs:46`.

---

### `beui-backdrop-blur-degraded` — no blur filter anywhere in the port, catalog-wide

**Observed**: `PaintScene::backdrop_blur` and the baseline `glass_surface` widget now give a plugin
a real backdrop blur (see `backdrop-blur-bound` for its limits), but `frust-beui` has not adopted
them, and no layer-filter blur exists (`filter: blur(...)` on a widget's own content; the engine's
internal filter passes are not exposed as a `frust_scene::Command`). Every one of upstream's
`backdrop-filter`/`filter: blur(...)` effects therefore still degrades the same way catalog-wide: a frosted backdrop (drawer, bottom sheet, popover, morphing
modal, center-morph modal, command palette, expandable action bar, project folder, wallet card,
morphing search) becomes a flat wash with no blur behind it; a text/badge/number roll's blur half
(animated badge, number ticker, action-swap's `Blur` variant, checkbox exit, tooltip, select item
entrance, text-reveal/chromatic-reveal, preview rail entrance, adaptive stepper) is dropped and the
opacity/scale/translate half is kept; and the one filter-driven merge effect — adaptive stepper's
upstream `Liquid` gooey metaball surface (an SVG blur + contrast filter fusing three pills into one
blob) — has no primitive to approximate at all, so each pill paints on its own instead.
`morphing_tabs` is not a blur casualty: upstream's `morphing-tabs.tsx` has no gooey SVG filter to begin with, so the port's analytic
rounded-rect morph path is not a degradation of anything upstream actually did.

**Applies to**: every `frust-beui` component whose upstream source names a `filter`/
`backdrop-filter` CSS property — dozens across `components`, `agents`, and `blocks`; see each
component's own "Degradations" section for its specific blur.

**Why accepted**: adopting `glass_surface` for the frosted backdrops is a port-side change not yet
made, and `frust-shadcn`'s and `frust-material`'s drawer/sheet/dialog/popover scrims accept the same
flat wash (no LIMITATIONS entry exists for those catalogs' own instances yet, since no gate has
needed one); a layer-filter blur needs a scene-level `Command` that does not exist yet
(`frust-engine::filters` already builds and schedules the passes internally), not something a
facade-only plugin can add.

**Evidence**: `plugins/beui/src/components/adaptive_stepper.rs`, `drawer.rs`, `bottom_sheet.rs`,
`popover.rs`, `morphing_modal.rs`, `center_morph_modal.rs`, `theme_toggle.rs`,
`components/animated_badge.rs`, `number.rs`, `action_swap.rs`, `checkbox.rs`, `tooltip.rs`,
`select.rs`, `text_animation.rs`, `preview_rail.rs`; `plugins/beui/src/blocks/command_palette.rs`,
`expandable_action_bar.rs`, `project_folder.rs`, `wallet_card.rs`, `morphing_search.rs`,
`overflow_actions.rs`, `not_found.rs`, `feedback_widget.rs`, `dynamic_island.rs`,
`availability_scheduler.rs`, `swap.rs`, `expandable_tabs.rs`, `infinite_masonry.rs`,
`prediction_market.rs`, `otp_input.rs` (each "No blur" or "No backdrop blur" degradation);
`plugins/beui/src/blocks/morphing_tabs.rs` (the gooey-filter premise correction);
`docs/RENDER_ARCHITECTURE.md`'s GPU Substrate table (`frust-engine::filters`).

---

### `beui-per-char-text-shaping` — cross-cell typographic relationships are lost in effect text

**Observed**: `motion::chars` shapes every grapheme cell independently so it can be individually
transformed, which means every cross-cell typographic relationship a single shaped run would keep
is lost: kerning pairs no longer tighten, ligatures no longer form, and a cursive or complex script
(Arabic, Devanagari) loses the joining/reordering that makes it legible. This is inherent to the
effect, not an implementation shortcut, and the module docs restrict its own use to short display
strings for exactly this reason — never body text.

**Applies to**: `motion::chars::CharCells`/`CharCellsView` and every component built on it (the
per-letter `action_swap` cascade, `text_animation`'s `Reveal`/`Cascade` arms, `citations`' cascade).

**Why accepted**: the shaping compromise is structural — the effect requires independently
transformable letters, and a single shaped run has none — and upstream (`inline-block` spans per
letter) carries the identical compromise for the identical reason.

**Evidence**: `plugins/beui/src/motion/chars.rs` module docs ("The shaping compromise, stated
plainly").

---

### `beui-3d-degradations` — the 2D approximation is the default build's behaviour and every opt-in's runtime fallback; five components lift it behind `gpu-effects`

**Observed**: `tilt_card` (perspective tilt) paints an affine shadow slide instead of a true
perspective transform; `wheel_picker` (an iOS-style drum) and `cylinder_carousel` render a 2D
cylinder projection rather than a real 3D drum; `project_folder`'s file fan and `wallet_card`'s
account-switcher fan are 2D approximations of upstream's layered/perspective originals — none
composites a real depth buffer or a perspective projection matrix. That is still every one of the
five's only behaviour in a default build and the runtime fallback of every opt-in: the non-default
`gpu-effects` feature (`plugins/beui/src/gpu_fx`) adds a genuinely projected quad substrate, and
each component takes it only through an explicit, never-auto-on opt-in
(`TiltCardView::gpu_face`, `WheelPicker::gpu_drum`, `CylinderCarouselView::gpu_cylinder`,
`ProjectFolderView::gpu_fan`, `WalletCardView::gpu_fan`). What stays true even with the feature on
and a device reachable: a widget subtree is never perspective-transformed — the substrate tilts
only a face it renders itself (a colour, a gradient, or a caller-owned texture), so every
component's own text, avatars, and captions keep compositing flat under `Affine`, opt-in or not.

**Applies to**: `frust_beui::components::tilt_card`/`wheel_picker`/`cylinder_carousel`,
`frust_beui::blocks::project_folder`/`wallet_card`, and their `gpu_face`/`gpu_drum`/`gpu_cylinder`/
`gpu_fan` opt-ins.

**Why accepted**: a widget subtree cannot be perspective-transformed by any path this substrate
offers — that boundary is structural, not a scoping gap — and the 2D approximations read correctly
on their own at the sizes and interaction distances these components are used at, which is why the
3D surface stays an opt-in enhancement rather than becoming the default. Rig coverage is uneven:
proven on Linux, an NVIDIA T400 over Vulkan, by `plugins/beui`'s own `#[ignore]`d GPU read-back
tests (`gpu_fx::card3d`/`cylinder`/`fan`/`quad3d` test modules) and by a manual desktop pass of
`examples/beui-demo`'s GPU Effects page on 2026-09-04 (every 3D pane live, the tilt face
foreshortens on hover, the wallet switcher fans). Metal is expected to behave the same — the
substrate reaches the GPU only through `frust::gpu`, with no backend-specific code of its own — but
unverified until a Mac runs it. beUI keeps acquisition closed on Android and iOS (its passes do not
yet rebuild device-owned resources on a device change), so every gpu-effects variant renders its 2D
path there.

**Evidence**: `plugins/beui/src/components/tilt_card.rs`, `wheel_picker.rs`,
`cylinder_carousel.rs`, `plugins/beui/src/blocks/project_folder.rs`, `wallet_card.rs` module docs'
"Degradations"/"The true-3D ..." sections; `plugins/beui/src/gpu_fx/mod.rs` module docs ("What it
still cannot do"); `examples/beui-demo/README.md`'s
"GPU Effects" section (T400 substrate coverage, Metal owed).

**Trigger for removal**: the widget-subtree boundary is permanent (a new renderer capability,
not a fix, would lift it) and stays out of scope for removal; the rig-coverage half closes once
both mobile shells install a `DeviceHandle` and a Mac records a Metal pass of the GPU Effects page.

---

### `beui-gpu-fx-face-fidelity` — every `gpu_fx` face is logical-resolution, square-cornered, and glare is a linear ramp

**Observed**: three fidelity limits hold across every `gpu-effects` opt-in, all substrate-wide
rather than per component: (1) a target is allocated in logical pixels — one texel per logical
pixel, with no scale-factor axis at the paint seam — so the composite upscales like any other
unscaled raster on a HiDPI surface; placement is texel-exact only when the card's own sides are
integral — `card3d::target_for` forces each axis's overscan margin to an even texel count
(`even_margin`, which rounds the card's own side up (`original.ceil()`) before taking the margin,
so the margin comes out an even texel count even for a fractional side), so a zero-tilt face's
texel grid lands on the device pixel grid with no half-texel offset when the card's side is
already integral — but flex layout can hand a component a fractional side, and for one of those
the composite resamples like any other fractional-origin paint; (2) a face is a
`QuadFace::Solid`/`Gradient`/`Texture` fill only, and always square-cornered, since a rounded clip
is a rectangle in scene space and a tilted or turned face is not one; (3) `tilt_card`'s
pointer-tracking glare has no radial-gradient `QuadFace` to reach for, so it degrades to a linear
ramp aimed at the pointer rather than a true radial falloff.

**Applies to**: every `gpu_fx`-rendered face (`tilt_card::gpu_face`, `wheel_picker::gpu_drum`,
`cylinder_carousel::gpu_cylinder`, `project_folder::gpu_fan`, `wallet_card::gpu_fan`) for (1) and
(2); `tilt_card`'s glare specifically for (3).

**Why accepted**: each is a primitive limit of the substrate itself, not a per-component
shortcut — `PaintScene` publishes no HiDPI-aware scale factor at the external-texture seam,
`QuadFace` has no rounded-rect clip that survives a projective transform, and `QuadFace` has no
radial-gradient variant — and at card/plate scale the differences (a HiDPI upscale, a squared-off
corner behind whatever rounded 2D clip sits around it, a linear rather than radial glare) read
close enough that a bespoke fix per component was not justified.

**Evidence**: `plugins/beui/src/gpu_fx/card3d.rs` module docs ("One target texel is one logical
pixel", "and why a face is still pixel-exact at rest" — the integral-side qualifier, "The glare is
a linear ramp, not a radial one") and `target_for`/`even_margin` (the even-margin,
no-half-texel-offset guarantee, and `even_margin`'s `original.ceil()` before taking the margin);
`plugins/beui/src/gpu_fx/quad3d.rs`'s `QuadFace` enum; `plugins/beui/src/components/tilt_card.rs`
("The face is rendered at logical resolution", "The face has square corners");
`plugins/beui/src/components/wheel_picker.rs`, `plugins/beui/src/components/cylinder_carousel.rs`,
`plugins/beui/src/blocks/project_folder.rs` (each "The plate(s) have square corners").

**Trigger for removal**: a paint-seam scale-factor axis for offscreen targets, a projective
rounded-rect clip in the substrate, and a `QuadFace::RadialGradient` variant — three independent
renderer additions, none currently planned.

---

### `beui-gpu-fx-depth-and-pooling` — a translucent face writes depth like an opaque one, and the target pool over-allocates small faces

**Observed**: three mechanics bound every `gpu_fx` scene.

- **(a) Translucent faces write depth.** A `Quad3dScene::depth` attachment writes depth for a
  translucent face as it would for an opaque one, so a scene wanting faces to blend has to submit
  them in the right order. The wheel drum, carousel wall and wallet switcher sort nearest-first (a
  far-then-near submission would blend a translucent plate twice and darken every overlap); the
  folder fan keeps the caller's back-to-front order, because a nearer sheet blends over a further
  one and that order is the only signal the pile has.
- **(b) One binding per pass, not per surface.** With two engine-tier surfaces live, the second
  surface's `record` sees the generation the first already recorded, so that surface never receives
  the bind and composites nothing under its `SceneTextureId`.
- **(c) A coarse, per-component target pool.** `pool::TargetPool` rounds every offscreen target up
  to a 256 px square, so a small face still occupies a full 256 px texture, reaps a key unused for
  120 frames, and refuses a target past `min(4096, max_texture_dimension_2d)` or one the device
  rejects. A component commits to the 3D path in paint, on the UI thread, against the 4096 policy cap
  alone; the device-limit refusal comes later on the render thread, so such a face silently never
  appears, with no 2D fallback for that extent. Each component owns its own pool with no aggregate
  budget, so a colour plus `Depth24Plus` target at the 4096 px ceiling (on the order of 100 MB) is a
  per-component worst case. `Quad3dRenderer` is built once per component instance, so two live
  instances each pay their own pipeline-compile cost on their first frame.

**Applies to**: every `gpu_fx` consumer; (a)'s nearest-first sort to `cylinder_carousel::gpu_cylinder`,
`wheel_picker::gpu_drum` and `wallet_card::gpu_fan`, its back-to-front order to
`project_folder::gpu_fan`; (b) only when a process drives two engine-tier surfaces.

**Why accepted**: (a) the order is geometry-specific by design, so no single rule serves both. (b)
every shipped shell drives one engine-tier surface per process. (c) the 256 px quantization and
120-frame reap mirror `frust_gpu::effects`' own texture pool, a refused target draws nothing instead
of panicking, the device limits are not published through the facade for a paint-side check, no
shipped component drives more than a handful of targets, and sharing one renderer per device would
serialize concurrent `record` calls.

**Evidence**: `plugins/beui/src/gpu_fx/cylinder.rs` and `fan.rs` module docs; `card3d.rs`
(`fan_scene`, `target_for`); `quad3d.rs` ("Depth", `Quad3dRenderer`); `schedule.rs` (`PassState`,
`ExternalPass::record`); `pool.rs` (`TARGET_QUANTUM`, `MAX_UNSEEN_FRAMES`, `MAX_TARGET_SIDE`,
`TargetPool::acquire`, `create_target`).

**Trigger for removal**: none planned for (a) or the pool's quantization, reap and per-instance
compile. The render-thread refusal in (c) ends when the facade exposes the live device's limits to
widgets; (b) ends with a per-surface `Binding` keyed by `ExternalFrame::drain_id`, or the first
shell that keeps two engine-tier surfaces live.

---

### `beui-gpu-fx-component-additions` — the true-3D opt-ins grow a surface, or a reading, upstream never had

**Observed**: turning `gpu-effects` on changes what a component's surface *is*, per component.
`wheel_picker::gpu_drum` and `cylinder_carousel::gpu_cylinder` seat a plate behind every row/ball
that upstream's CSS 3D scene never paints at all — a deliberate departure, not a port, since a
transparent drum or wall would project nothing — and the wheel's plate and its 2D label
projection disagree by a few percent near the cutoff, because the plate is projected by the
substrate's own camera while the label is scaled by the component's `PERSPECTIVE` constant (both
are masked to near-zero alpha there, so the disagreement is not visible at the horizon itself).
`project_folder::gpu_fan` composites every sheet's plate into one target before painting captions
over it, so a caption a nearer sheet's translucent fill would dim in the flat 2D pile instead reads
at full strength once the fan is projected. `wallet_card::gpu_fan` has no upstream card stack to
fan at all — upstream's only run of stacked surfaces is the account switcher's own open panel — so
what the 3D path fans is that panel's rows (see `wallet_card`'s own "A premise correction").

**Applies to**: `wheel_picker::gpu_drum`, `cylinder_carousel::gpu_cylinder`,
`project_folder::gpu_fan`, `wallet_card::gpu_fan`.

**Why accepted**: each is a named, deliberate consequence of the one boundary every opt-in shares
(`beui-3d-degradations` above) recorded at the point it was found, not an oversight — a plate is
what makes a projected drum or wall visible at all; the caption-dimming loss and the card-stack
substitution both follow directly from "a 3D face is a colour, a gradient, or a texture, never a
widget subtree," stated once and true everywhere it applies.

**Evidence**: `plugins/beui/src/components/wheel_picker.rs` ("The drum grows a surface it did not
have", "The plate and its text disagree slightly at the horizon"); `plugins/beui/src/components/cylinder_carousel.rs`
("The stage grows a wall it did not have"); `plugins/beui/src/blocks/project_folder.rs` ("An
overlapped caption stops being dimmed"); `plugins/beui/src/blocks/wallet_card.rs` ("A premise
correction"; "The 2D selected-row plate stands down while the fan is live").

**Trigger for removal**: none planned — each is a direct consequence of the substrate-wide face
boundary (`beui-3d-degradations` above), not an independent gap.

---

### `beui-overlay-seam` — the anchored host needs bounded constraints

**Observed**: `overlay::anchored`, beUI's non-modal trigger-relative host (see
PLUGINS_ARCHITECTURE.md's Design-System Plugins for the seam itself, and
`shadcn-anchored-exit-needs-kept-mounted` for its kept-mounted exit ramp, which beUI's anchored host
shares), fills whatever area it is given and expects bounded constraints, so it cannot sit inside a
`frust::scroll_view` (whose child gets an unbounded max on the scroll axis) — the `beui-demo` gallery
hits this directly: `citations`' hover preview is documented to mount through `overlay::anchored`,
but the gallery's page slot is itself a scroll view, so the demo instead reads the hover through
`on_hover_change` and paints the same preview panel inline, in a page-owned slot the caption names
as the workaround.

**Applies to**: every component still mounted through `overlay::anchored` — popover, context menu,
the dropdown panels of select/combobox/multi_select, citations' preview. `tooltip` no longer mounts
through this host at all: it rides the framework overlay portal instead (see
`overlay-portal-v1-scope`), so it carries no gap from this entry.

**Why accepted**: the same bounded-constraints/no-scroll-view mounting contract `frust_shadcn`'s and
`frust_material`'s `overlay::anchored` hosts already carry (`overlay/mod.rs`'s "scroll-view trap" in
all three catalogs) — the remedy is at the mount site, not the host, and the gallery demonstrates the
correct workaround rather than avoiding the case.

**Evidence**: `plugins/beui/src/overlay/mod.rs` ("The scroll-view trap");
`plugins/beui/src/agents/citations.rs` module docs (the `overlay::anchored` mounting sequence);
`examples/beui-demo/src/pages/agents/panels.rs`'s `citation_panel` (the inline workaround and its
caption); `plugins/beui/src/components/tooltip.rs` module docs ("Riding the framework portal" — the
section documenting `tooltip`'s move off this host onto the framework overlay portal instead).

---

### `beui-focus-and-keys` — five named focus/keyboard gaps across the catalog

**Observed**: five related gaps, the first four named in their components' own source:

1. **No type-ahead or keyboard list navigation.** `select`, `combobox`, and `multi_select` ship none
   of Radix's jump-to-typed-match or arrow-key row traversal — a wrapped editable owns the focus
   path instead, the same v1 boundary `shadcn-select-no-typeahead-fixed-height` already documents
   for `frust_shadcn`.
2. **A container cannot release a descendant's focus session.** `feedback_widget` does not blur its
   message field when the panel closes while the field holds focus — the framework gates the orphan
   mark on a live focus chain a rebuild cannot see (`docs/CODE_STANDARDS.md`), so the session stands
   until the next press elsewhere blurs it, the same shape `overlay-no-auto-focus-on-appear`
   documents from the opposite direction. The close is a paint-only morph (the widget stays mounted
   and keeps its focus), so keystrokes and IME composition keep routing into the now-invisible field
   for as long as the session stands. Clearing the field on close discards what it holds but does
   not end the capture; only unmounting the block, or an app-owned barrier that takes the next
   press, ends the session — so an app hosting a sensitive value there unmounts it on close.
3. **`hold_action_button`'s `on_complete` fires on the next event pass, not the instant the fill
   lands.** The fill is advanced during paint, which carries no `EventCtx` to call an app callback
   through; the completion is latched at paint and drained on the next pointer event the widget
   sees (release, for a real hold gesture) — the same paint-vs-event boundary
   `no-plugin-reachable-deferred-state-callback` documents, with no `Housekeeping` flush seam on the
   public facade to close it. Keyboard activation (`Enter`/`Space`) is not ported either, for
   `hold_action_button` and `slide_action_button` alike.
4. **No built-in auto-dismiss.** `animated_toast_stack` ports no timer and carries no `duration`.
   Upstream's timer lives in a React hook; here a widget cannot drive the dismissal, because a
   countdown expiring in `paint` has no route to `&mut State` and `frust::time::Timer`'s callback
   carries none either. The app owns the schedule (its own `Timer` or signal drops the entry, and
   the exit plays from there).
5. **No Tab stops.** The framework's root Tab/Shift+Tab traversal exists, but no beUI control
   answers `Widget::is_focusable`, so Tab never stops anywhere in the catalog and no control
   activates from the keyboard through it.

**Applies to**: `frust_beui::components::select`/`combobox`/`multi_select`,
`frust_beui::blocks::feedback_widget`, `frust_beui::components::expanding_arrow_button` (both
`hold_action_button` and `slide_action_button`), `frust_beui::components::animated_toast_stack`;
(5) every interactive `frust_beui` control.

**Why accepted**: (1) mirrors an already-accepted shadcn boundary; (2) and (3) are framework-level
gaps with no plugin-tier fix available, already accepted for their originating catalogs/entries;
(4) is a correct reading of what a widget can own versus what only the app's own clock can drive;
(5) is per-control opt-in work (`is_focusable`, Space/Enter activation, a ring read through
`PaintCtx::is_focused_for_ring`) not yet done for this catalog.

**Evidence**: `plugins/beui/src/components/select.rs`, `combobox.rs`, `multi_select.rs` module docs
("No type-ahead"/"No type-ahead, and no keyboard list navigation");
`plugins/beui/src/blocks/feedback_widget.rs` module docs ("Closing does not blur the message
field", "It reserves its open box and paints inside it" — the morph is pure paint, no rebuild);
`plugins/beui/src/components/expanding_arrow_button.rs` module docs ("fires on the next
event pass", "Keyboard activation is not ported"); `plugins/beui/src/components/animated_toast_stack.rs`
module docs ("No timer"); no `is_focusable` implementation under `plugins/beui/src` (for item 5).

---

### `beui-no-file-drop` — `file_upload`'s dropzone does not accept an OS file drop on its own

**Observed**: `blocks::file_upload` wires no `drag_target`. Its dropzone is driven through
`FileUploadView::on_browse`/`add_files`, and `FileUploadView::dragging` is a prop, never an observed
drag state, so a file dropped on it does nothing unless the app wires the drop itself.

**Applies to**: `frust_beui::blocks::file_upload` on every platform; Android, iOS and web have no OS
file-drop signal at all (`file-drop-desktop-only`).

**Why accepted**: on desktop an app wraps the block in `frust::drag::drag_target::<Vec<PathBuf>>`,
hands the dropped paths to `add_files`, and drives `dragging` from the drag session. The block needs
no shell access of its own.

**Evidence**: `plugins/beui/src/blocks/file_upload.rs` (`FileUploadView::add_files`/`on_browse`/
`dragging`); no `drag_target` use under `plugins/beui/src`.

---

### `beui-consent-correlation-is-opt-in` — a decision/action callback only correlates to its request when the caller sets an `id`

**Observed**: `agents::tool_approval` and `agents::approval_card` each expose two decision
callbacks: `ToolApprovalView::on_decision`/`ApprovalCardView::on_action` (the default, no request
identity) and `ToolApprovalView::on_decision_with_id`/`ApprovalCardView::on_action_with_id` (which
also report `Option<String>` — `Some(id)` when the builder's own `id(..)` was set on that card,
`None` when it was not). Neither module derives a fallback identity from its content — `tool`/
`title` are not unique — so an app that leaves `id` unset and wires only the id-less callback
cannot distinguish which of several overlapping requests a decision answers.

**Applies to**: `frust_beui::agents::tool_approval::ToolApprovalView` and
`frust_beui::agents::approval_card::ApprovalCardView`.

**Why accepted**: correlation is opt-in by design — a caller with only ever one open request at a
time has no need for it, and forcing an `id` on every card would be surface no single-request
caller wants. The hazard is the id-less path being the default: a caller that grows to multiple
concurrent requests without also switching callbacks silently routes consent to the wrong one.

**Evidence**: `plugins/beui/src/agents/tool_approval.rs` module docs ("Correlating a decision with
the request it answers") and `ToolApprovalView::id`/`on_decision_with_id`;
`plugins/beui/src/agents/approval_card.rs` module docs ("Correlating an action with the review it
answers") and `ApprovalCardView::id`/`on_action_with_id`.

**Trigger for removal**: a required-id builder variant, or a lint/debug-assert that catches an
id-less callback wired alongside more than one concurrently open card.

---

### `beui-agents-degradations` — named gaps across the `agents` catalog beyond the not-ported items above

**Observed**: recurring, named gaps across `frust_beui::agents`, each a platform or scope boundary
rather than an oversight:

- **No syntax highlighting.** `code_block` and `tool_approval`'s optional code value ship no shiki
  equivalent — every line is one monochrome `CodeTokenClass::Plain` run, with a token-class seam
  left for a future highlighter rather than colors baked per-theme the way shiki's are.
- **`file_diff` takes a pre-parsed `FileDiffModel`.** The caller supplies hunks/lines already split;
  no diff algorithm runs in this crate.
- **No scroll viewports in agent lists.** `code_block`, `file_diff`, `tool_result`, and
  `agent_activity` clip rather than scroll their bounded-height bodies — none reads a scrollable
  viewport, only a "bounded and following" one.
- **No favicons.** `citations` renders numbered chips instead of `useFavicon`'s fetched/fallback
  images; `CitationStack`'s overlapping-favicon cluster is dropped with it.
- **`streaming_response`'s reveal ramp is this port's own** — upstream does not animate streamed
  text arriving at all.
- **Clipboard is the app's.** `tool_result` and `code_block`'s copy actions have no clipboard to
  copy to (that is the app's own clipboard plugin's job), so both report a copy request rather than
  performing one.
- **Avatars are text, not a slot.** `message`'s avatar takes initials text, not upstream's
  arbitrary `ReactNode`.
- **`image_generation` drops blur** on its media-reveal states, the catalog-wide blur absence (see
  `beui-backdrop-blur-degraded`).
- **`ai_sidebar` has no drag-and-drop and no inline rename** — moves run through the four
  `Alt+Shift+Arrow` commands (`AiSidebarView::on_move`, `ai_sidebar_move`), and a host renames
  through `ai_sidebar_rename` from its own dialog.
- **Agent lists auto-collapse silently** — `todo_list`'s and `agent_activity`'s automatic
  `collapseOnComplete` flips an internal flag rather than reporting through a callback, the
  rebuild-driven variant `no-plugin-reachable-deferred-state-callback` now also documents.
- **`chat_app`'s user bubble is `Soft`, not upstream's default variant** — a deliberate visual
  choice for the assembled example, named in its own module doc.

**Applies to**: `frust_beui::agents::code_block`/`tool_approval`/`file_diff`/`tool_result`/
`agent_activity`/`citations`/`streaming_response`/`message`/`image_generation`/`ai_sidebar`/
`todo_list`/`chat_app`.

**Why accepted**: each is either an inherited platform gap already accepted elsewhere (clipboard,
deferred-callback), a named scope line (shiki, pre-parsed diff model, favicons, drag-and-drop,
inline rename), or
a deliberate design choice recorded in its own module doc (the reveal ramp, the bubble variant).

**Evidence**: `plugins/beui/src/agents/code_block.rs`, `tool_approval.rs`, `file_diff.rs`,
`tool_result.rs`, `agent_activity.rs`, `citations.rs`, `streaming_response.rs`, `message.rs`,
`image_generation.rs`, `ai_sidebar.rs`, `todo_list.rs`, `chat_app.rs` — each module's own
"Degradations against upstream" section.

---

### `beui-blocks-degradations` — named gaps across the `blocks` catalog

**Observed**: recurring, named gaps across `frust_beui::blocks`, each a platform or scope boundary:

- **`notification_stack` animates its own height.** frust hit-tests a widget's layout box only, so
  the box grows downward as the stack spreads and the expanded stack displaces what is below it
  rather than covering what is above it.
- **`swipeable_list`'s thresholds are distance-only.** Upstream's release rules pair a distance arm
  with a velocity arm. Only the distance arm is ported (the velocity arm's constants are recorded,
  unused), although drag recognizers do publish a release velocity (`DragEndDetails::velocity`), so
  a fast, short flick that upstream would open springs back here.
- **No outside-press dismissal for `expandable_tabs`/`notification_stack`.**
- **No pointer-type distinction** anywhere in the catalog — the contact's source rides beside the event
  (`EventCtx::pointer_id`), but no beUI component reads it, so a mouse and a touch press are handled identically.
- **`morphing_tabs`' active-tab order commits on release, not after the neighbours settle.**
- **Text narrowing (`ReactNode` → `String`) in stack/list rows** — `notification_stack` and similar
  row-based blocks take plain text where upstream takes arbitrary content, the same narrowing
  `shadcn-otp-table-button-api-gaps` names for `table`'s header/footer.
- **`marquee` is not interactive** — only hover pauses it; a press does not.
- **`theme_toggle`'s reveal is scoped to the toggle itself**, not the full viewport upstream's View
  Transition API sweeps.

**Applies to**: `frust_beui::blocks::notification_stack`/`swipeable_list`/`expandable_tabs`/
`morphing_tabs`; `frust_beui::components::marquee`/`theme_toggle`; no-pointer-type-distinction
applies catalog-wide.

**Why accepted**: each is a named platform gap (no out-of-flow layout) or a scope line recorded in
its own module doc, not a regression found later.

**Evidence**: `plugins/beui/src/blocks/notification_stack.rs`, `swipeable_list.rs`,
`expandable_tabs.rs`, `morphing_tabs.rs`, `plugins/beui/src/components/marquee.rs`, `theme_toggle.rs`
— each module's own "Premise correction"/"Degradations" sections; no `PointerSource` use under
`plugins/beui/src`.

---

### `scroll-view-no-external-offset-seam` — the programmatic-scroll seam exists; only the `message_scroller` ports and beUI's `scroll_to` remain to migrate onto `ScrollController`

**Observed**: the baseline programmatic-scroll seam now exists (`ScrollController`: an
offset-write path usable outside event dispatch, applied at the attached surface's next layout or
paint, plus a published-snapshot read) and `frust-shadcn`'s `scroll_area` is already migrated onto
it — its thumb drags through `jump_to` once a controller is attached. `frust-shadcn`'s
`message_scroller` and `frust-beui`'s `message_scroller` (same registry slug, ported
independently, same seam rationale recorded in both module docs) have not migrated: each still
owns its own offset field, wheel/drag consumption, and fling/glide physics in parallel with
`ScrollView`'s — two independent scroll-gesture implementations whose feel (slop thresholds, wheel
line height, decay curves) must be kept consistent with the baseline by hand. `frust-beui`'s
`scroll_to` is the same gap from the write side: it still ports only upstream's *animation* (an
eased offset ramp), publishing each frame's value into a caller-owned signal rather than driving a
`ScrollController`.

**Applies to**: `frust-shadcn`'s and `frust-beui`'s `message_scroller`; `frust-beui`'s `scroll_to`.
A baseline `ScrollView` feel/physics tuning has no mechanism to propagate into either
`message_scroller` copy and will silently drift. Neither port gets overscroll rubber-band or
pull-to-refresh either, for the same reason: a transcript's live edge is not `ScrollView`'s own
rubber-band surface, and riding the baseline surface through the controller is what would bring it.

**Why accepted**: migrating each is per-catalog follow-up work now that the seam exists and is
proven (`scroll_area`'s thumb drag), not a missing framework capability; each parallel
implementation was the honest v1 route and is tested on its own terms.

**Evidence**: `crates/frust-widgets/src/scroll_controller.rs` (`ScrollController`'s record/drain/
publish contract); `plugins/shadcn/src/components/scroll_area.rs` (migrated: `.controller()` plus
the thumb's `jump_to`-driven drag); `plugins/shadcn/src/components/message_scroller.rs` and
`plugins/beui/src/agents/message_scroller.rs` (still their own offset/fling/glide state machines);
`plugins/beui/src/components/scroll_animation.rs` (`scroll_to` still animates a signal it does not
apply to a surface).

**Note**: unrelated to this gap, a `ScrollController::scroll_to_item` animated onto a keyed row
whose rows ahead were never measured can end with a small correction snap once the real extents are
known — the settle-and-correct pass nudges to the re-resolved position rather than retargeting the
animation mid-flight (see WIDGETS_ARCHITECTURE.md's *Virtualized ListView*).

---

### `list-view-anchor-same-frame-both-sides` — a same-frame mutation on both sides of a keyed `ListView`'s anchor can leave the scroll position uncorrected for one frame

**Observed**: a keyed `ListView` re-anchors its scroll offset with an anchor-shift probe that tests
only two candidate index shifts. A frame that mutates rows on both sides of the anchor (e.g. a
prepend above and an append below) can miss that probe, leaving the scroll position uncorrected for
that frame, a visible jump. Any pending measured-anchor correction is discarded rather than
committed into geometry it can no longer explain.

**Applies to**: keyed `ListView` (`ListView::builder_keyed`) rows when one frame mutates both sides
of the anchor row. Fixed for `sliver_list`, whose anchoring goes through `locate_key` (a scan of up
to 64 items around the anchor) and the viewport's correction loop: a two-sided mutation is anchored
exactly when the anchor row moved by no more than that radius.

**Why accepted**: the cost is one frame's scroll position, never a misattached measurement or a
leaked row entry. Correcting every two-sided mutation would re-key all `item_count` items, the O(N)
work the keyed design exists to avoid.

**Trigger for removal**: a two-sided correction that does not re-key every item (see the
`list_view.rs` module doc, *Cache hygiene*), or a device report of the jump in a real app.

**Evidence**: `crates/frust-widgets/src/list_view.rs:157-164` (module doc, "Two acknowledged gaps");
`docs/WIDGETS_ARCHITECTURE.md` *Virtualized ListView* (keyed-list re-anchoring paragraph) and
*Slivers and the viewport* (correction-based anchoring).

---

### `sliver-v1-scope` — the sliver viewport ships a deliberately narrow v1

**Observed**: `custom_scroll_view` and the sliver family lay out and scroll forward-growing content
only, with these gaps:

- forward growth only: no reverse growth and no `center` anchor sliver;
- no overlap absorber or `NestedScrollView`-style coordination between a header and an inner scroller;
- no snapping persistent headers, and no stretch or expansion past `max_extent` during overscroll;
- a floating header reveals only under a drag or fling (the viewport reports Idle for a wheel step,
  a controller `jump_to` or `animate_to`, as Flutter's pointer-scroll does);
- header and content rebuild one frame after the geometry that drove them (paint requests the frame
  and the next rebuild calls `build`), so a resizing header's content lags by one frame;
- a gap above row 0 wider than the scroll position (rows above measured shorter than estimated) in a
  `sliver_list` closes with a visible shift, as in Flutter;
- a horizontal viewport needs a bounded height;
- `scroll_to_item` is ignored by `CustomScrollView` (item commands are list-only); a controller
  `animate_to` ignores a scroll-offset correction (absolute target, as in Flutter);
- the shared core is built per axis and follows an axis change on rebuild; only the stretch
  overscroll transform is still transposed for a horizontal viewport. `SliverConstraints` has no
  constructor (needed before it could become `#[non_exhaustive]`).

**Applies to**: apps composing a `custom_scroll_view` that need any behavior above.

**Why accepted**: the v1 covers the common collapsing-header, lazy list and grid compositions; each
gap needs protocol work (a second growth direction, an overlap absorber) not worth carrying before a
real consumer asks.

**Trigger for removal**: each bullet is deleted when it lands, the entry when the last one does.

**Evidence**: `crates/frust-widgets/src/viewport.rs`, `crates/frust-widgets/src/slivers/` module
docs; `docs/WIDGETS_ARCHITECTURE.md` *Slivers and the viewport*.

---

### `scroll-controller-desktop-frame-gate-gap` — a `ScrollController` write from a handler that requests no redraw waits for the next desktop frame

**Observed**: a desktop shell does not consult the pending-result-flush flag the mobile frame gate
uses. A `jump_to`/`animate_to` issued from a handler that itself requests no redraw is recorded, but
it is applied only at the attached surface's next layout or paint, which waits for whichever desktop
frame some other input produces. The scroll visibly lags until then.

**Applies to**: desktop shells (macOS, Windows, Linux) driving a `ScrollView`/`ListView` (or
`frust-shadcn`'s `scroll_area`) from a non-redrawing handler such as a timer or a background signal.

**Why accepted**: the same mechanism gap `PanZoomController` already has. The queued write is never
lost, only applied on a later frame.

**Trigger for removal**: desktop shells consult the pending-result-flush flag (the mobile form is in
SHELLS_ARCHITECTURE.md, *Mobile frame path*), or a controller write requests its own redraw.

**Evidence**: `docs/WIDGETS_ARCHITECTURE.md` *Programmatic scroll* (desktop frame-gate paragraph).

---

### `scrollbar-vertical-pointer-only` — the baseline scrollbar is vertical, pointer-driven and invisible to semantics

**Observed**: `.scrollbar(ScrollbarStyle)` exists only on the vertical `ScrollView` and `ListView`; horizontal surfaces and `CustomScrollView` take no bar. The bar is reached by pointer alone — no keyboard focus or keys — and contributes no semantics node of its own. A `ListView` can show one blank frame after a large thumb jump, as after any controller jump applied at paint, before the next rebuild re-windows its rows.

**Applies to**: apps opting into the baseline scrollbar on any platform.

**Why accepted**: the bar is an opt-in visual indicator and pointer affordance over surfaces that already scroll by touch, wheel and `ScrollController`; a horizontal or sliver bar and keyboard/semantics exposure are separate work.

**Trigger for removal**: a horizontal or `CustomScrollView` bar, a keyboard or semantics surface for the bar, or a `ListView` that re-windows within the jump's frame.

**Evidence**: `crates/frust-widgets/src/scrollbar.rs` (module doc); `crates/frust-widgets/src/scroll_core.rs` (`route_pointer`); `examples/playground/README.md` (scrollbar gate row).

---

### `android-overlays-opt-in-pending-device-leg` — Android overlays are off by default although the host passes its device leg

**Observed**: the Android overlay host (frust paint above a native view on an opaque surface) runs
only when `FRUST_OVERLAYS` is set (runtime env, compile-time `--define`, or the
`FRUST_OVERLAY_SPIKE` alias; `media` selects the `setZOrderMediaOverlay` arm). With it unset the
shell calls plain `ingest`, which never emits overlays, and the frame path is byte-identical to a
build without them. The host itself passes its device legs: on a Pixel 5 (Android 14) and a
OnePlus 9 (Android 15) the overlay draws above the native view with correct premultiplied alpha, is
repainted within one frame of a slot remount or background-to-foreground, lets drags reach the
native control, and leaves no leaked overlay `SurfaceView` after rapid toggles (present skew about
0.5 ms average, at most 3.9 ms).

**Applies to**: Android apps that composite frust content over a native view (native-widgets
popovers and badges, video or camera controls over the picture) with the knob on.

**Why accepted**: the default stays off until the flip is decided on its own. Every host requests
a frame when an overlay layer or window is attached, resized or withdrawn, and withdrawing an
overlay does not block the UI thread; the cost is described in
`android-overlay-withdraw-after-return`.

**Trigger for removal**: the default flips (or the knob is retired); see
`android-overlay-withdraw-after-return` for the withdraw behaviour that flip depends on.

**Evidence**: `crates/frust-shell-android/src/app/frame.rs`, `jni_glue.rs`, `overlay_state.rs`,
`crates/frust-shell-common/src/overlay.rs`; SHELLS_ARCHITECTURE.md
§ Native-view hosting.

---

### `android-overlay-withdraw-after-return` — An Android overlay withdrawn mid-frame is released after that frame

**Observed**: with `FRUST_OVERLAYS` on, `surfaceDestroyed` for an overlay never waits on the render
thread. An idle overlay's GPU surface is dropped and its window released before the call returns.
An overlay whose surface is out for a frame is released when that frame lands, so that one frame
may acquire and present into a window Android has already destroyed. The present fails (wgpu
reports a lost or out-of-date status rather than panicking) and is logged at debug.

**Applies to**: Android apps with the knob on that dispose overlays or background the app while a
frame is in flight.

**Why accepted**: blocking `surfaceDestroyed` on the render thread is what the hardening removed,
because it could stall the UI thread for a surface-acquire timeout. The window object stays valid
until the frame ends because the shared overlay table holds its own reference.

**Trigger for removal**: an ack-barrier that lets `surfaceDestroyed` wait at most one frame
without holding the lock across the acquire, or evidence of a driver that mishandles a present on
an abandoned BufferQueue.

**Evidence**: `crates/frust-shell-android/src/jni_glue.rs` (`nativeOverlaySurfaceDestroyed`,
`FrameOverlays`); `crates/frust-shell-common/src/overlay.rs`.

---

### `android-overlays-cover-later-slots` — an on-top overlay covers every native view, including later slots

**Observed**: an Android overlay `SurfaceView` stacks above every native view, so frust content
interleaved *between* two slots (A, frust card, B) is drawn over B as well. The native-widgets demo's
`sandwich` page shows it with its "Interleaved" checkbox.

**Applies to**: Android apps with frust content painted between two hosted native views while
`FRUST_OVERLAYS` is on. One cut around a single view is unaffected.

**Why accepted**: `setZOrderOnTop` places a `SurfaceView` above the whole view hierarchy, not just
above one sibling, and the `media` arm is a different trade-off rather than a fix. The common case
(everything under the views, then both views, then everything over them) is one cut and unaffected.

**Trigger for removal**: a z-order scheme that places each overlay between its two slots, or a
compositing path that does not need a surface per cut.

**Evidence**: `examples/native-widgets-demo/src/pages/sandwich.rs`; `FrustViewHost.kt`.

---

### `android-hwbuffer-import-opt-in` — Android zero-copy import is off by default and RGBA8-only

**Observed**: importing an `AHardwareBuffer` as a `wgpu::Texture` needs a hand-opened Vulkan device,
created only when `FRUST_ANDROID_HWBUFFER_IMPORT=1` or sysprop `debug.frust.hwbuffer_import` is set
(`TierCaps::hwbuffer_import`). It imports RGBA8 buffers only and needs API 29 or later; with the
knob off, no importer is installed and a native frame is refused, so producers fall back to a CPU
copy. The `debug.frust.hwbuffer_import` property is honoured in release builds too, because the
device legs run release builds; setting it needs adb or root and is device-wide. While the knob is
on, other threads submitting wgpu work are unsupported (debug builds assert that imports stay on
the first importing thread).

**Applies to**: Android texture-bus producers (video player, any plugin pushing native frames).

**Why accepted**: the hand-opened device is a deliberate deviation from wgpu's own device creation.
On a Pixel 5 and a OnePlus 9 the importer's Vulkan device opens and video texture output plays
correctly both with and without it; the first delivered frame per session logs its path at Info,
and on the OnePlus 9 (Android 15) that line reads `texture output via AHardwareBuffer (zero-copy
importer installed)` with the knob on and `via CPU copy` with it off, the picture identical in
both. The default stays off because the hand-opened device is still a deviation.

**Trigger for removal**: the knob default flips, plus a YUV import path if decoders are to be
sampled directly.

**Evidence**: `crates/frust-gpu/src/import/android.rs`, `context.rs`;
`crates/frust-render/src/native_import/android.rs`.

---

### `video-texture-android-gles-copy` — Android video texture mode copies through GLES

**Observed**: hardware decoders emit YUV and the importer is RGBA8-only, so a `VideoOutput::Texture`
session renders into a `SurfaceTexture` and a GLES2 copy on its own GL thread writes each frame into
an RGBA_8888 `ImageReader` (about 1.05 ms per 1080p frame measured on the host; device unmeasured).
The frame then travels as a zero-copy `AHardwareBuffer` when the importer is installed and API is 29
or later, else as a CPU copy.

**Applies to**: Android apps opting into `VideoOutput::Texture`; the native-view output (the
default) is unaffected.

**Why accepted**: sampling the decoder's YUV buffer directly needs a YUV import path the bus does not
have; the copy keeps the importer simple.

**Trigger for removal**: a YUV (external-format) import path.

**Evidence**: `plugins/video-player/src/android.rs`; `plugins/video-player/README.md`.

---

### `camera-texture-cpu-copy-only` — camera preview textures are CPU-copied

**Observed**: `preview_texture` on Android runs a second RGBA_8888 `ImageAnalysis` and copies each
frame to the CPU; there is no zero-copy path (Pixel 5: p50 13 ms, 94 of 120 frames over 16.7 ms).
Holding analysis images while the bus retires them would stall the analysis, so the zero-copy
hand-off is not used. The Apple producer is zero-copy (a second 32BGRA video data output, 20-24 fps
on an iPhone SE); a device may refuse that second output while an image stream runs, which is
logged and leaves the mailbox empty.

**Applies to**: Android apps opting into the camera's texture preview; the native preview (default)
is unaffected.

**Why accepted**: the CPU copy is the safe producer until the analysis pipeline can lend images
without stalling.

**Trigger for removal**: a camera producer that lends hardware buffers without stalling analysis.

**Evidence**: `plugins/camera/src/android.rs`, `texture.rs`.

---

### `texture-bus-native-release-delay` — a native frame's release can lag by a submit, and a full backlog drops the newest frame

**Observed**: a native frame's release runs only after its texture is retired and a
`Queue::on_submitted_work_done` fence fired; a registration dropped while a frame is shown chains a
second fence, delaying the producer's buffer reuse by up to one extra submit. At most
`MAX_RETIRING` (4) retired textures wait per registration; a native frame arriving beyond that is
refused and released at once, so the last good frame stays shown. The two chained fences cover
every submission made before the drop plus one more completion point, not a frame recorded before
the drop and submitted after it: a registration dropped mid-drain may release its last native frame
while the frame that sampled it is still executing, and that one frame may tear. This is not a
memory-safety issue (the importer holds its own reference and wgpu keeps the texture alive).

A release callback that panics is contained in unwinding builds (dev and test) without a log line
on the producer side (the renderer side logs); under the shipped `panic = "abort"` profile it
aborts the process, so callbacks must not panic.

**Applies to**: producers with a buffer ring shallower than the retire backlog plus one shown frame.

**Why accepted**: releasing earlier would let a producer overwrite a buffer the GPU is still
sampling.

**Trigger for removal**: a different ownership scheme (producer-side fences) that makes retirement
explicit, or tying the release to the frame's own submission — owed before the hardware-buffer
import knob ever defaults on. A device replaced by a newer one is polled until its retire fences
fired before the bus lets go of it; a device that is never polled again (lost outright) releases
its retiring frames only on its next poll.

**Evidence**: `crates/frust-render/src/texture_bus.rs` module docs.

---

### `retained-pages-budget-refusals` — a retained page is a budgeted, refusable cache; every refusal paints plain

**Observed**: the retained store (`frust-engine::cache::retained`) keeps page textures within a byte
budget tiered by adapter family (`FRUST_ENGINE_RETAINED_BUDGET_MB` overrides). A body is refused —
rendered inline, its key absent from the ledger — when the compiler marked it unretainable (under a
rounded or path clip mask; it draws a punch, an external texture, a shader quad or a `BackdropBlur`;
a stray pop cut it; it is not wholly inside the viewport and every scissor), when it is empty, draws
outside its rect, exceeds the texture ceiling or the whole budget, or reuses a key within a frame. A
`DrawRetained` the store cannot serve refuses the frame (`EngineError::RetainedKeyMissing`); the
shell repaints it without pages. Widget-side limits: navigator pages at rest paint plain (only a
transition's seed frame records the leaving page, and a slide's entering page paints plain); bodies
using paint side channels (IME, platform view, input shield, overlay, selection toolbar, hero) paint
every frame and hero pages never draw a page; a page moved by a fractional translation composites up
to half a pixel off the inline frame (translation rounds to whole device pixels); a boundary inside a
clipping container cannot see that clip, so it may record a body the engine then refuses to retain.

**Applies to**: apps using `repaint_boundary`, `AnimatedOpacity::cached`, navigator or switcher
transitions.

**Why accepted**: a refusal degrades to an inline, page-less paint for that frame, never to a wrong
pixel: "key absent from the ledger" also holds for an already-resident key whose re-record was refused,
nested or unrendered (its page is evicted), and the paint serial stops a dropped scene passing an older
page off as the latest recording. The one leftover case is a key drawn and refused-on-re-record in the
same frame, which keeps the drawn page; hover and focus transitions re-record every boundary once; the kill switches `FRUST_NO_RETAINED_LAYERS` and `FRUST_ENGINE_NO_LAYERS` revert wholesale.

**Trigger for removal**: a boundary that sees its enclosing clip, and subpixel-exact page placement.

**Evidence**: `docs/RENDER_ARCHITECTURE.md` (`cache::retained`); `crates/frust-widgets/src/repaint_boundary.rs`.

---

### `retained-ledger-single-renderer` — the retained ledger slot and repaint flag are process-global

**Observed**: the published ledger slot and the repaint-owed flag are process-global. A second renderer in one process (a second desktop window, iOS multi-scene, several web canvases) would publish over the first and is not detected. A `RenderRoot` replaced in-process while the renderer lives restarts its paint serials at 1, a narrow window in which the ledger's `frame` can exceed the new serials.

**Applies to**: multi-window, multi-scene or multi-canvas hosts using retained layers.

**Why accepted**: every shell hosts one renderer today; a wrong page needs both a second renderer and colliding keys.

**Trigger for removal**: a shell hosting a second renderer per process; key the slot and flag per renderer.

**Evidence**: `crates/frust-shell-common/src/render_split.rs`.

---

### `retained-stale-ledger-eviction-thrash` — an eviction between a ledger publish and the next scene refuses the whole frame

**Observed**: an eviction, shrink or clear landing between a ledger publish and the UI's next scene makes that scene's `DrawRetained` fail. The engine refuses the whole frame (`EngineError::RetainedKeyMissing`, nothing presented), clears every retained page, and the shell repaints against the empty ledger. Near the budget (`RetainedBudget`: 32 MiB on a phone, about three full-screen pages; 64 MiB on the `mobile_large` tier) this can recur, and each refusal is only debug-logged.

**Applies to**: mobile apps with several full-screen retained pages near the budget. See `retained-pages-budget-refusals` for the other refusals.

**Why accepted**: the result is a correct but slower frame, never a wrong pixel.

**Trigger for removal**: device evidence of repeated refusal thrash, or a scene that echoes the ledger generation it was built against, so only the offending key is refused.

**Evidence**: `crates/frust-engine/src/cache/retained.rs` module docs and `RetainedBudget`; `EngineRenderer::take_retained_refusal` in `crates/frust-engine/src/renderer.rs`.

---

### `backdrop-blur-bound` — `BackdropBlur` has five limits

**Observed**: (1) no backdrop inside a translucent (isolated) layer, a retained body or a suppressed
layer — it is dropped, not blurred; (2) at most the per-frame cap, more refuses the whole frame
(`EngineError::TooManyBackdrops`, never a degraded render): `MAX_BACKDROPS_PER_FRAME` (4) by
default, an app sets it with `set_max_backdrops_per_frame` (process-wide, clamped to
`1..=BACKDROP_CAP_CEILING`, 16); only a backdrop whose blur region reaches the frame target counts,
so glass in the warm band a `Flex` inside a `ScrollView` paints one viewport beyond the visible
area does not count, but the up-front test ignores retained corrections and clip coverage and can
over-count a backdrop the frame later drops inside a clipped or retained region; (3) pixels beyond the frame border read as
transparent, so a blur at the edge fades; (4) a region past the page ceiling is refused; (5) a
fragment-sampled copy of an sRGB target loses precision. The CPU oracle blurs its own canvas and is
exact only for device σ <= 2; the `unit-backdrop-blur` golden compares with the clip rim eroded.
On the direct present arm a frame carrying a backdrop renders via an intermediate and pays one
present copy (T400: ≈0.239 ms/frame at 1080x2400, ≈0.189 ms at 1920x1080).

**Cost of raising the cap** (delivered fps, 600 frames per cell, N sigma-12 `glass_surface`s at cap N
over animated stripes; GPU timestamps do not capture blur work on these devices, so fps is the
signal): Pixel 5 (Adreno 620, 90 Hz, split thread) 0 surfaces 89.8, 4: 83.3, 6: 57.2, 8: 44.7, 4 at
sigma 24: 67.4 — about +2.6 ms of frame interval per backdrop past 4, linear, and sigma 24 costs about
one extra sigma-12 backdrop; iPhone 13 mini (60 Hz, single thread) 60 through 6, 53.5 at 8 (its
split arm is pacing-capped at 40 fps even with no surfaces, so it is not cited); Mac M4 (120 Hz)
120 through 8. A frame over the cap is refused, never slow. Cost scales with sigma as well as count,
so the default stays 4 and an app targeting stronger hardware raises it.

**Applies to**: frosted-glass style chrome.

**Why accepted**: the schedule serves one blurred copy per backdrop at the frame's root; nesting inside
layers needs the general scheduler.

**Trigger for removal**: the general scheduler, or a measured need above the 16 ceiling.

**Evidence**: `docs/RENDER_ARCHITECTURE.md` (`schedule::build_frame`); golden `unit-backdrop-blur`
(cpu, engine-metal-macos and engine-vulkan-nvidia-t400); backdrop-cap device gate, 2026-11-05,
commit aea75586e (Pixel 5, iPhone 13 mini, Mac M4).

---

### `cupertino-glass-intent-not-sigma` — the iOS-27 glass scale carries radius-family blur values, not sigma

**Observed**: `frust_cupertino::glass::ios27()` sets `blur_radius_intent` to 75 (chrome), 45 (bar)
and 15 (control), the mined kit's radius-family numbers. `glass_surface` reads the field as a Gaussian
sigma in logical px (CSS `blur(Npx)` is sigma N), so under Cupertino it blurs far more heavily than
the design intends and the engine clamps the largest values to its maximum sigma. Cost also scales
with sigma (see `backdrop-blur-bound`).

**Applies to**: `glass_surface` (and any widget passing the intent to `PaintScene::backdrop_blur`)
under the Cupertino theme; other design systems author their own sigma.

**Why accepted**: the correct sigmas have not been re-derived from the kit; rewriting the numbers
without that source would swap one guess for another.

**Trigger for removal**: re-derive the three tiers as sigma in `plugins/cupertino/src/glass.rs`
and compare against the kit.

**Evidence**: `plugins/cupertino/src/glass.rs` (`ios27`), `crates/frust-theme/src/glass.rs`
(`GlassMaterial::blur_radius_intent`), `crates/frust-widgets/src/glass_surface.rs`.

---

### `path-clip-mask-rerasterized` — `PushClipPath` is always a coverage mask, re-rasterized per push

**Observed**: `ClipStack::push_path` always takes the mask shape — never a page, never a scissor —
and there is no mask cache, so a path clip is re-rasterized on every push; a retained body under it
is never retained.

**Applies to**: scenes with many or large path clips per frame.

**Why accepted**: correctness first; clip masks for rounded rects already work this way.

**Trigger for removal**: a measured frame where path-clip rasterization shows up, then a mask cache.

**Evidence**: `docs/RENDER_ARCHITECTURE.md` (`compile::clip`); `crates/frust-engine/src/compile/clip.rs`.

---

### `nav-swipe-depth-staleness` — `depth()`/`can_pop()`/`back_interest()` run eager, the route-state observable runs conservative, for the same in-flight interactive swipe

**Observed**: from the left-edge steal (`begin_interactive_pop`) to the settle-frame publish,
`NavigatorController::depth()`/`can_pop()`/`back_interest()` and the route-state observable
(`route_stack()`/`RouteObserver`) diverge in opposite directions rather than both lagging alike.
`begin_interactive_pop` pops the page out of the retained stack immediately, at steal, before the
drag paints a frame; `NavigatorView::rebuild`'s trailing `publish_state()` call is unconditional,
so every drag frame republishes `depth`/`can_pop`/`back_interest` against that already-popped
count — EAGER, ahead of the commit. The route-state observable's own publish is gated on the
transition's `interactive` flag and returns early while the drag holds, so it keeps reporting the
pre-swipe stack — CONSERVATIVE, behind the commit — until the settle-frame publish. Unbounded in
wall time because a finger can hold; a cancelled swipe makes the lagging observable's read
retroactively correct rather than something to retract.

**Applies to**: any consumer reading `depth`/`can_pop`/`back_interest`/the route-state observable
while an edge-swipe pop is in flight, in any app using the baseline `Navigator`/`Router`.

**Why accepted, not fixed**: the eager side has a real, non-cosmetic cost — at depth 2, a held
swipe already publishes `compute_back_interest(1, Pop) == false`, so a back press read mid-drag
claims no interest and escapes to the platform (activity finish on Android) even though releasing
below the commit point restores the page. A fix needs `depth`/`can_pop`/`back_interest` to also
gate on the transition's `interactive` flag the way the route-state observable's publish already
does — real follow-up work, not something dismissed as harmless. The route-state observable
itself was deliberately designed to publish only on settle (a committed-mutation contract, not a
live drag readout); frame-accurate drag chrome already has `NavigatorController::transition()`
(`is_pop`/`progress`/`interactive`) for that window. This is a pre-existing `depth`/`can_pop`/
`back_interest` gap the new observable documents rather than fixes.

**Evidence**: `crates/frust-widgets/src/nav/route_state.rs` module docs' Staleness contract;
`crates/frust-widgets/src/nav/navigator.rs`'s `depth`/`can_pop`/`back_interest` doc comments.

---

### `nav-back-wiring-order-case9` — an inner navigator's explicit `BackHandler`, wired before its outer host exists, loses the innermost-first tie-break

**Observed**: `Registrant::key` ranks `Role::Navigator` peers by raw wire sequence (`birth`), not
structural nesting depth. When an inner navigator's explicit `BackHandler` wires (and keeps
tracking) before its outer host's first view exists — the `Component::init` shape — it loses the
innermost-first back-press tie-break to the outer navigator instead of winning it, inverting the
documented "structurally innermost wins" rule for that one wiring order.

**Applies to**: an app that constructs a `BackHandler` for a nested navigator in `Component::init`,
ahead of the outer navigator's own first `build`. `shell_route`'s nested-navigator composition is
**not** affected: its inner navigator's own `navigator(...)` call is always mounted by the shell
page's builder, so it necessarily wires after the outer (enclosing) navigator already has a view —
the pathological order cannot arise from `shell_route` alone.

**Why accepted, not fixed**: pinned by an `#[ignore]`d test
(`wiring_order_does_not_flip_the_innermost_navigator_when_it_is_still_poppable`) as the acceptance
criterion for a future ranking fix (nesting depth instead of birth order); not blocking because the
only known reachable trigger is a hand-built `Component::init` pre-registration, not anything the
shipped `shell_route`/`navigator` composition produces.

**Evidence**: `crates/frust/src/back_glue.rs`'s `#[ignore]`d test and its inline design-claim
rationale (search `wiring_order_does_not_flip`).

Not affected: sibling navigators under an `IndexedStack` (only the selected child is reachable, so no
tie-break arises between siblings) and a `BackFallback` (it ranks after every navigator by role; wire
order only orders fallbacks among themselves).

---

### `nav-go-undelivered-results` — a `go` does not deliver every result callback it makes unreachable

**Observed**: `Router::go` fires `on_result` once with an empty result for each page it removes from a
navigator. Two cases get nothing: a page inside a nested navigator torn down with its host page, and a
push still queued on a shell controller the new chain does not cross, whose queue the `go` discards.

**Applies to**: code that clears state only in `on_result` for a page inside a `shell_route`, when a
`go` can leave that shell.

**Why accepted, not fixed**: `NavigatorView::teardown` has no `&mut State` pass to flush results in, and
Flutter loses a nested navigator's route results the same way. The queued push never became a page.

**Evidence**: `NavigatorView::teardown` in `crates/frust-widgets/src/nav/navigator.rs`; `forget_queued_outside`
and `a_go_out_of_a_shell_discards_a_stale_push_before_a_push_back_in` in `crates/frust-widgets/src/nav/router.rs`.

---

### `nav-go-closes-routeless-overlays` — a `go` closes a dialog even over a page it keeps

**Observed**: a route-less page (a dialog or sheet pushed through the controller) has no key, so a `go`
removes it even when the page beneath it is retained, including a `go` to the current location.

**Applies to**: an app that navigates while an overlay is open and expects the overlay to stay,
including a `BackPolicy::Veto` overlay and a `go` from a deep link. A security gate (app lock, consent)
must be a route redirect, not an overlay. Flutter keeps pageless routes attached to a retained page;
frust has no such attachment.

**Why accepted, not fixed**: the stack after a `go` is exactly the matched chain, which is the contract
apps rely on to leave nothing below a finished flow.

**Evidence**: `a_go_within_the_shell_closes_a_page_pushed_over_it` in `crates/frust-widgets/src/nav/router.rs`.

---

### `render-blurred-shadow-corner-collapse` — a per-corner blurred shadow rounds to its largest corner

**Observed**: `Command::BlurredRoundedRect` carries a per-corner `CornerRadii`, but the engine's
blurred-rect primitive (`vello_common` 0.2.0's `BlurredRoundedRectangle`) accepts only one radius.
The compiler collapses the four corners via `CornerRadii::largest()` before encoding, so a shadow behind geometry
with mixed corner radii (e.g. two sharp corners, two rounded) renders with all four shadow corners at
the largest configured radius instead of each corner independently.

**Applies to**: any glow/shadow paint (`ContainerView`'s `glow`, or a direct `PaintScene` caller)
whose `CornerRadii` are non-uniform. The fill, border, and clip paths stay exact per-corner —
`Command::RoundedRect`/`PushClipRounded` both lower every corner individually; only the blur is
affected.

**Why accepted**: the primitive has no per-corner form; matching one would
mean hand-rolling the blur pass rather than composing the existing one. The visual delta is confined
to the soft, low-opacity shadow rather than the crisp geometry, so it shipped as a v1 approximation
alongside the `CornerRadii`/`DashPattern` primitives rather than blocking on it.

**Evidence**: `crates/frust-engine/src/compile/blur_rrect.rs`'s decode of
`Command::BlurredRoundedRect` through `CornerRadii::largest` and its regression test.

---

### `material-from-seed-diverges-from-baked-baseline` — `theme_from_seed`/`from_seed(#6750A4)` is not pixel-identical to `baseline()`

**Observed**: `frust_material::from_seed`/`theme_from_seed` ports current
`material-color-utilities` 0.11.1, while `baseline()` transcribes Google's
published Material 3 reference palette (material-web tokens v0.192) — two
different sources of truth for the same seed color. Seeding `from_seed` with
`#6750A4` (the reference seed `baseline()` itself was built from) does not
reproduce `baseline()` exactly: the error ramp differs (`#BA1A1A` from MCU vs
`#B3261E` baked), and roughly half the remaining roles drift by ±1 RGB step.

**Applies to**: any app calling both `baseline()` and `theme_from_seed`/
`from_seed(#6750A4)` and expecting identical output, or switching a theme
from one to the other at runtime.

**Why accepted**: this is correct interop behavior, not a bug — `from_seed`
matches a Flutter app seeded identically via current MCU, which is the point
of exposing it. The divergence from `baseline()` is pinned deliberately by an
explicit table test (`from_seed_diverges_from_the_baked_baseline_only_as_recorded`,
`plugins/material/src/tokens/hct.rs`) rather than fixed, since "fixing" it
would mean rebaking `baseline()` off MCU output and breaking the documented
reference-palette contract instead. Consequence: an app switching
`baseline()` → `theme_from_seed(#6750A4)` sees a subtle recolor across roughly
half its roles plus a visibly different error red.

**Evidence**: `plugins/material/src/tokens/hct.rs`'s
`from_seed_diverges_from_the_baked_baseline_only_as_recorded` test.

---

### `material-elevation-single-shadow-layer` — M3E's two-layer shadow model collapses to one `ShadowSpec` per level

**Observed**: the Material 3 Expressive reference elevation model pairs a key
shadow (30% alpha, offset = dp/2, blur = dp) with an ambient shadow (15%
alpha, offset = dp, blur = dp*2) at every level. `frust-theme`'s `Elevation`
type contract carries one `ShadowSpec` per `ElevationLevel`, so
`frust_material::elevation()` ships the documented single-layer mapping
instead (`plugins/material/src/tokens/metrics.rs`) — the dp values themselves
match the reference exactly; only the two-layer shadow stack is unexpressible.

**Applies to**: any app using `frust_material`'s baseline elevation table;
the visual softness of a reference-accurate stacked shadow is approximated by
one shadow layer at every level.

**Why accepted**: `Elevation`/`ShadowSpec` is a shared `frust-theme` contract
also used by Cupertino and shadcn — widening it to a shadow list is a
cross-catalog type change, not something the port took on speculatively. The dp table itself is exact; only the shadow decomposition is
narrowed.

**Evidence**: `plugins/material/src/tokens/metrics.rs` (module doc's two-layer
model description and the single-`ShadowSpec` `elevation_level` constructor).

---

### `material-textfield-filled-wash-unpaintable` — the filled text field's `surfaceContainerHighest` container wash cannot be painted

**Observed**: `frust_material::text_field`'s filled variant is meant to fill
its container with the `surfaceContainerHighest` role per the M3 reference,
but the widget only paints decoration around the baseline
`frust::TextInputView` it wraps, and that baseline field draws its own opaque
background (the theme's `surface` role) with no transparency seam to override
or suppress it — a wash painted only in the gutters the child's rounded rect
misses would leave a visible two-tone seam instead. The filled and outlined
variants therefore differ only by their stroke (bottom indicator vs full
outline), not by their container fill.

**Applies to**: any app using `frust_material`'s filled text field expecting
the reference `surfaceContainerHighest` tint; the container reads as
`surface` instead.

**Why accepted**: closing the gap needs a transparency seam on the baseline
`TextInputView`'s background paint — a `frust-widgets` change, not something
the port took on. The same class of gap already exists
for `frust_shadcn::input`'s `dark:bg-input/30` wash, so it isn't
material-specific.

**Evidence**: `plugins/material/src/text_field.rs` module doc's "Container
fill: a documented fidelity gap" section.

---

### `material-textfield-slot-blur-on-interactive-children` — an interactive leading/trailing slot child blurs the field before its own tap registers

**Observed**: `frust_material::text_field`'s leading/trailing icon slots are
routed through the shared container contract
(`frust::authoring::route_event`), not a text-field-specific one. That
contract's blur-on-outside-tap rule fires on `Down`: "a `Down` that does not
re-establish focus on the child it hits clears every focused sibling." A slot
child that is itself interactive (a password-visibility toggle, a clear
button) has not yet claimed focus at the moment its own `Down` arrives, so
that `Down` reads as an outside tap and blurs the field first — before the
slot child's own tap handler ever sees the event. The module doc states this
plainly: "**A `Down` on an interactive slot blurs the field** ... not
something this widget adds or can suppress — another reason the slots are
meant for plain icons."

**Applies to**: any app placing an interactive view (not a static icon) in
`TextFieldView::leading`/`trailing`. Two mainstream M3 patterns are not
properly buildable in v1: a password-visibility toggle and a trailing clear
button both need their own tap to land without first blurring the field they
sit on.

**Why accepted**: the blur-on-outside-tap rule is the shared container
contract every multi-child `frust-widgets` container routes through
(`authoring::route_event`), not something `text_field.rs` opted into or can
override locally. A fix needs a slot-level opt-out — a way for a same-widget
slot child to be exempted from blur-on-outside-tap when it is itself the
`Down` target — which is a `frust-widgets`/`frust::authoring` seam change,
not a catalog-level fix. Deliberately left as a v1 scope line (icon slots are
"the intended content") rather than attempted inside the port.

**Evidence**: `plugins/material/src/text_field.rs` module doc's "Known
limitations" section (the "A `Down` on an interactive slot blurs the field"
bullet).

---

### `material-picker-strings-no-i18n-seam` — the date picker's typed-entry mode is a fixed `mm/dd/yyyy`

**Observed**: the date and time pickers take their labels, month and weekday names and the
first-day-of-week index from the theme's `MaterialStrings`, and `frust-material-i18n` provides
localized tables (months and weekdays from CLDR). What is not localized is the date picker's
input-mode parse/format (`MaterialDate::format_compact`, `mm/dd/yyyy`): it is the fixed `en_US`
compact form and is not overridable through the strings struct, so a non-US app's typed-entry
mode still reads and writes `mm/dd/yyyy`.

**Applies to**: `frust_material::date_picker` input mode in any locale whose date order differs
from month-day-year.

**Why accepted**: closing it needs a second, locale-parametrized compact-date format threaded
through `MaterialDate`, which no caller has needed yet; the calendar mode is fully localized.

**Evidence**: `plugins/material/src/date_picker/date.rs`'s `MaterialDate::format_compact`;
`plugins/material/src/date_picker/mod.rs` module doc's Localization section.

---

### `material-modal-staged-dismiss-private-to-host` — two components' content-owned close affordances still can't request the staged exit ramp

**Observed**: `frust_material::overlay::modal`'s staged-exit machine (scrim tap, Escape, close
button, drag, Android back all reverse-ramp before the app's dismissal fires) is now reachable from
caller-built content too: `ModalDismiss`, installed via `.dismiss_handle(handle)` on the modal host
view, stages the same reverse exit `stage_dismiss` runs for chrome gestures — one staging path
covers both, and it carries a `PopResult` so a staged "Apply"/"Save" still reaches the pusher's
`on_result`. It deliberately bypasses `dismissable(false)` (that flag gates *user* exits; an
app-wired dismiss is not one). The forwarding builder method has landed on `side_sheet`, all three
`dialog` variants, `date_picker`, `time_picker`, and `bottom_sheet` (`sheet.rs`). `search`'s
full-screen view back button and `navigation_drawer`'s app-driven dismiss-on-select `on_select`
handler are the two holdouts: neither `SearchViewView` nor `NavigationDrawerView` forwards a
`dismiss_handle` method, so both still call `NavigatorController::pop()` directly and dismiss
**unstaged** while every host-chrome gesture on the same panel stages one.

**Applies to**: `frust_material::search`'s full-screen view back button and
`frust_material::navigation_drawer`'s dismiss-on-select handler only — both still cite this id in
their own doc comments. Every other `overlay::modal` family (`side_sheet`, `dialog` and its
confirmation/full-screen variants, `date_picker`, `time_picker`, `bottom_sheet`) is closed.

**Why accepted**: extending the forwarding builder method to `search`/`navigation_drawer` is small,
mechanical follow-up work, not a new design problem.

**Evidence**: `plugins/material/src/overlay/modal.rs`'s `ModalDismiss` type and its "App-initiated
dismiss" module-doc section; `dismiss_handle` forwarding methods in `side_sheet.rs`, `dialog.rs`,
`date_picker/dialog.rs`, `time_picker/mod.rs`, `sheet.rs`; `search/view.rs`'s header doc (back
affordance still pops unstaged) and `navigation_drawer.rs`'s "Selecting does not dismiss" section
(both cite this id).

---

### `material-staged-pop-one-frame-race` — a staged modal/sheet pop's identity guard still has a one-frame race window

**Observed**: `StagedPop` (`plugins/material/src/overlay/modal.rs`, shared by `sheet.rs`'s bottom
sheet) arms itself against `NavigatorController::depth()` when an exit stages and refuses the
deferred pop if depth moved by the time the ramp settles, which narrows a
wrong-page-pop from the whole ~300ms exit ramp down to a single frame. Depth is still read as
published at the *last rebuild*, though: a push enqueued after the rebuild immediately preceding
the settling paint, and before that paint runs, is invisible to the guard. The same wrong-page-pop
the guard exists to prevent can still happen in that narrowed window.

**Applies to**: any `frust_material` app dismissing a dialog/alert dialog/bottom/side sheet/
full-screen search/picker while racing an app-driven push (deep link, async completion) against
that dismissal's settle.

**Why accepted / fix direction**: `NavigatorController` publishes no signal a widget can observe
between rebuild and paint — `depth()` and `route_generation()` share the same publish-at-last-
rebuild timing, so neither closes the gap from inside `StagedPop`. A real fix is a navigator-level
seam for identity-checked pops (e.g. `pop_if`/`pop_route(id)`) in `frust-widgets` that resolves
identity at the pop itself rather than off a paint-time snapshot.

**Evidence**: `plugins/material/src/overlay/modal.rs`'s `StagedPop` doc comment ("Both share the
same advisory staleness … narrows the exposure … does not erase it").

---

### `material-refresh-no-hold-offset-during-refresh` — `ScrollView` has no seam to pin scroll position while a pull-to-refresh is in flight

**Observed**: `frust_material::refresh_indicator` overlays the pull indicator at its resting offset
via paint-time compositing (`PaintScene::push_transform`) rather than by holding the underlying
scroll content there, because `frust::ScrollView`'s public API has no "pin the offset until
released" affordance — its own release-settle always eases back to the clamped edge once the
pointer lifts, independent of whatever a wrapping widget is doing. The reference instead ties the
pulled content's held position to the same controller driving the indicator, so upstream's content
visibly stays pulled down for the whole refresh; this port's content is free to settle and scroll
normally underneath the overlaid indicator instead. A second, narrower gap in the same area:
`ScrollView::on_scroll` delivers the release-settle one event late, so the indicator cannot ride the
baseline's own settle motion and instead runs independent `snap`/`retract` animation controllers
seeded from the pull distance observed at release/cancel.

**Applies to**: any app using `frust_material::refresh_indicator` — the pulled content underneath
does not visually "hold" at the indicator's offset the way the M3 reference does, and the indicator
never inherits the scroll view's own settle easing.

**Why accepted**: per the design-system charter, `plugins/material` may not patch `frust-widgets` to
add the missing seam (`docs/PLUGINS_ARCHITECTURE.md`'s Design-System Plugins charter). A real fix is
a `frust-widgets`/`ScrollView` addition — a hold-until-released offset seam, and/or a same-frame
settle-delivery fix — not something this crate can attempt.

**Evidence**: `plugins/material/src/refresh_indicator.rs` module doc's "Documented seam gap: the
scroll offset is not held during a refresh" section (incl. the `on_scroll` one-event-late note).

---

### `material-fab-fixed-tier-icon-centering` — a tight-constrained FAB paints its icon off-center

**Observed**: `FabWidget` lays its icon out against its `FabSize` tier's own container box, not the
box the widget was actually laid out at, so a FAB rendered at a tight-constrained size that differs
from its nominal tier paints its icon off-center inside that box. Measured on the toolbar's
scroll-hide FAB morph (80dp to 56dp, tight-constrained so layout, visual and hit rects stay one):
about 12dp of icon offset at the 80dp end. The baseline `IconWidget` scales and centers its glyph
inside its own laid-out box, so the offset comes entirely from the FAB's fixed container math.

**Applies to**: any FAB rendered at a size other than its nominal `FabSize` tier; today only
`toolbar.rs`'s 80dp-to-56dp scroll morph. `fab_menu`'s trigger avoids the class by hand-rolling its
morph through `shapes::Morph`.

**Why accepted / fix direction**: closing it needs a `FabView` seam that exposes the actual
container size continuously (a `container_size` prop) so icon layout can track the constrained box
instead of the nominal tier. Until then, avoid constraining a FAB away from its tier if the icon has
to stay centered.

**Evidence**: `plugins/material/src/toolbar.rs` module doc (FAB morph section);
`plugins/material/src/fab.rs` (`Widget for FabWidget::layout`'s `self.size.container()`);
`plugins/material/src/fab_menu.rs` module doc (trigger-morph rationale);
`crates/frust-widgets/src/icon.rs` (`IconWidget`'s layout and paint).

---

### `material-rail-modal-no-back-dismiss` — a modal `NavigationRail`'s expanded state dismisses on scrim tap only, no Escape or Android back

**Observed**: with `NavigationRailModality::Modal`, the expanded rail overlays content behind a
scrim and reports `NavigationRailView::on_dismiss_modal` on a scrim press — the only dismissal
wired. Neither Escape nor an Android back press closes it, matching the reference exactly: upstream
wires only `GestureDetector(onTap: widget.onDismissModal)` around its scrim, with no `Focus` and no
key handling, despite `M3ENavigationRailModality.modal`'s own doc comment falsely claiming it
"dismisses on tap/esc". This port matches upstream's code, not its comment.

**Applies to**: any `frust_material` app using `navigation_rail` with `NavigationRailModality::Modal`
— an expanded modal rail has no key-driven dismiss path; an app that wants Escape/back handling must
wire it at the mount itself (a `frust::BackHandler` or its own key handling) and flip the dismiss
prop from there.

**Why accepted / fix direction**: the modal rail's scrim is deliberately painted in-widget rather
than routed through `crate::overlay::modal`, the catalog's modal host — that host is route-like (a
transparent navigator page with its own `BackPolicy`-staged exit ramp turning Escape/Android back
into a dismiss), while the reference's modal rail is an ordinary in-tree widget whose lifetime is
entirely a function of the `modality`/`type` props the app already owns; routing it through the
modal host would hand the navigator authority over a lifetime the app is already controlling. Upstream
parity plus that app-owned-modality design is why the gap is accepted as-is rather than patched to
match its own doc comment. A real fix is either routing the modal rail through the modal host (and
accepting the lifetime-authority conflict that motivated staying in-widget) or adding a standalone
key-dismiss arm (a `BackHandler`-shaped Escape/back listener) alongside the existing scrim tap.

**Evidence**: `plugins/material/src/navigation_rail.rs` module doc's "Modal presentation: painted
in-widget, not through the modal host" section (incl. the upstream doc-comment-vs-code note).

---

### `material-dynamic-color-injection-only` — Material You dynamic color is unreachable; only a seed-color injection path exists

**Observed**: Material You's per-device wallpaper-derived dynamic color is an OS platform-channel
capability, unreachable from `frust_material` under the design-system plugin charter (frust +
kurbo/peniko only, no OS reach — `docs/PLUGINS_ARCHITECTURE.md`'s Design-System Plugins charter).
The only injection path that exists is
`theme_from_seed(seed: Color, brightness: Brightness) -> Theme` (`plugins/material/src/tokens/
hct.rs:966`): an app that obtains an OS accent color some other way (no such OS-facing plugin
exists yet) can feed it in as a seed and push the regenerated `Theme` through the framework's own
`set_default_theme`/`set_app_theme` seams — there is no seam that accepts a caller-built
`ColorScheme` directly, only a single seed color.

**Applies to**: any `frust_material` app wanting true Material You theming.
`examples/material3-demo`'s theme settings screen reflects the gap directly: its "Dynamic color"
row is shown, disabled, with a "Not available — frust has no device wallpaper palette to read"
supporting line, rather than wired live or dropped from the list.

**Why accepted / fix direction**: the plugin charter bars OS reach outright, so this crate cannot
close the gap itself; a real fix needs a separate OS-facing plugin (a wallpaper-palette or
platform-theme capability) to source the color, which `theme_from_seed` could then consume as its
seed.

**Evidence**: `plugins/material/src/tokens/hct.rs:966` (`theme_from_seed`); `examples/material3-demo/src/pages/
theme_config_page.rs`'s `toggles` (the disabled "Dynamic color" row and its supporting text);
`examples/material3-demo/src/theme/settings.rs` module doc ("Dynamic (device-sourced) coloring is
not modelled at all").

---

### `material-no-focus-traversal` — no material control is a Tab stop; overlays claim focus only on first interaction

**Observed**: the catalog ports per-widget focus **visuals** throughout (state-layer focus rings
and outlines, gated on `PaintCtx::has_focus`). The framework now has root Tab/Shift+Tab traversal
over `Widget::is_focusable` stops ([CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md)'s Keyboard
Shortcuts and Traversal), but no `frust_material` control opts in — none answers `is_focusable` —
so Tab never stops on a material control and focus still arrives only through a widget's own
claim from a `Down`; there is still no `autofocus` or claim-on-mount. `button/mod.rs` and
`split_button.rs` record the gap as an "Honest limitation" ("frust has no keyboard focus traversal
… no autofocus, no `FocusNode`, no Tab-ring" — wording that predates the root walk; the catalog's
missing opt-ins are what keep it true here); `slider/mod.rs`'s thumb ring is ported and painted,
but "[n]othing focuses a slider today" for the same reason.
`dropdown/mod.rs`'s Up/Down/Enter/Escape panel handling is gated on the identical claim-based
model — the panel only sees key events once it holds focus, which it claims on a press inside
itself, so a caller must complete one pointer interaction with the panel before the arrows do
anything; the module doc names this "no auto-focus-on-appear hook and no focus-traversal seam in
the framework."

**Applies to**: every interactive `frust_material` component — none of them is a Tab stop, so each
rides the claim-on-press model alone. Module docs call the gap out explicitly for
buttons, split button, sliders, and the dropdown panel; every other focus-visual seam in the
catalog (menus, dialogs, text fields) rides the identical model without its own restatement. See
`overlay-no-auto-focus-on-appear` for the sibling gap this entry does not re-cover: no widget
(material's `dialog`/`search` included) can claim focus programmatically **on mount** either —
that entry is the register's existing home for the on-appear half; this entry covers the
between-controls traversal half.

**Why accepted / fix direction**: the traversal half is now per-control catalog work — each
control answers `is_focusable`, activates on a bare Space/Enter while focused, and reads its ring
through `PaintCtx::is_focused_for_ring` for keyboard-only display — while the claim-on-mount hook
is still framework work that does not exist. The ported focus **geometry** is what those opt-ins
will light up.

**Evidence**: `plugins/material/src/button/mod.rs` module doc's "Focus ring" section ("Honest
limitation"); `plugins/material/src/split_button.rs` module doc ("Honest limitation, the same one
[`mod@crate::button`] records"); `plugins/material/src/slider/mod.rs` module doc's "Focus outline: geometry
ported, reachability limited" section; `plugins/material/src/dropdown/mod.rs` module doc's
"Keyboard, and the focus gap" section; `overlay-no-auto-focus-on-appear` (this register, above).

---

### `material-descoped-flutter-isms` — two Flutter-framework mechanisms the port doesn't attempt

**Observed**: two Flutter-framework-level mechanisms the M3E reference leans on have no
counterpart in this workspace; each affected component's own module doc records the descope in
place rather than silently dropping the behavior.

- **Form-field validation + state restoration** (`dropdown`/`dropdown_menus`): the reference wraps
  its tree in a Flutter `FormField` for `validator`/`autovalidateMode`, rendering an error line and
  recoloring the border through `formState.hasError`; this workspace has no `Form`/`FormField`
  equivalent to register with, and Flutter's state-restoration plumbing has no counterpart at all.
  An app that needs validation renders its own message and drives `DropdownFieldView::error` itself.
- **Native platform menu style** (`split_button`): upstream's third menu style
  (`M3ESplitButtonMenuStyle.native`, Flutter's own `showMenu` platform route) is descoped — this
  framework hosts no platform menu to route to. Only the popup and bottom-sheet styles ship.

**Applies to**: `frust_material::dropdown`/`dropdown_menus` (no validator/autovalidateMode, no
restored field state); `split_button` (no native platform menu style — an app on a platform whose
OS ships one sees the popup or bottom-sheet style instead).

**Why accepted**: a deliberate v1 scope line recorded in each component's own source, not a bug —
closing either needs framework-level work (a `Form`/`FormField` primitive plus restoration
plumbing, or a platform-menu host), neither of which has a caller beyond this port yet to justify
building ahead of need.

**Evidence**: `plugins/material/src/dropdown/mod.rs` module doc's "Descoped: form-field validation
and restoration" section; `plugins/material/src/split_button.rs`'s header comment ("upstream's
third menu style … is descoped").

---

### `scroll-physics-stretch-affine-approx` — `OverscrollEffect::Stretch` is an affine approximation, not Android's shader

**Observed**: `OverscrollEffect::Stretch` paints a whole-viewport, scroll-axis-only affine scale
about the held edge (`stretch_about_edge`/`stretch_intensity`, `scroll.rs`) — the same approximation
Flutter's own non-Impeller `StretchingOverscrollIndicator` makes of Android 12's overscroll shader,
not the shader itself. A single affine scale cannot reproduce the shader's per-pixel falloff, and
release motion rides the widgets' existing settle decay (`SETTLE_DECAY`) rather than Android's own
release spring (ω = 24.657, ζ = 0.98), which is not ported. Roughly 60-70% visual fidelity to the
real effect.

**Applies to**: any surface painting under `OverscrollEffect::Stretch` — Android's platform-adaptive
default today; reachable on any platform as an explicit `.overscroll_effect(Stretch)`.

**Why accepted**: the same approximation Flutter itself ships (non-Impeller); a per-pixel shader
port is a materially larger render-path change than the physics seam this stretch effect rides on.

**Evidence**: `crates/frust-widgets/src/scroll.rs` module docs' *Overscroll visuals*
(`stretch_about_edge`/`stretch_intensity`, the accepted-fidelity note and the ω/ζ citation).

---

### `scroll-physics-shadcn-carousel-unmigrated` — two catalog carousels still hand-roll their own scroll engines, neither on `crate::physics`

**Observed**: `plugins/shadcn/src/components/carousel.rs` (the shadcn/ui Carousel port) implements
its own drag capture, snap-to-nearest-item settle, and flick detection directly — it does not sit on
`crate::physics::ScrollPhysics`/`Simulation`, `ScrollView`, or `ListView`. It predates and is
untouched by the scroll-physics seam. `plugins/material/src/carousel/` (the M3E hero/contained/
uncontained carousel, BSD-3-Clause-derived from Flutter's `CarouselView`) is a second, independent
occurrence of the same pattern: its own weighted-slot layout (`layout.rs`) plus its own release/snap
math (`physics.rs`'s `snap_offset`/`SETTLE_DURATION` ramp, advanced by hand from the shell frame
clock) drive drag capture and settle directly, with no dependency on `crate::physics`, `ScrollView`,
or `ListView` either — it was ported after shadcn's carousel and made the identical v1 choice
independently rather than adopting shadcn's engine or waiting on a shared one.

**Applies to**: `frust-shadcn`'s `carousel()` and `frust_material`'s `carousel`/`extended_carousel`;
each one's feel (snap timing, flick threshold, settle curve) is tuned and tested independently of
every other scroll surface in the framework — including each other — and neither can pick up a
`ScrollPhysics` change, including `PageSnap` (the snap physics they could now compose over),
automatically.

**Why accepted**: `PageSnap` (`crates/frust-widgets/src/physics/page_snap.rs`) now ships the page/
fixed-extent snap on the `ScrollPhysics` chaining seam, but neither carousel has been moved onto it, and
neither upstream's scroll engine (embla for shadcn, M3E's
own weighted-carousel math for material) has a frust-side package to wrap — both ports' own module
docs record the re-implementation as a deliberate v1 choice. Migrating either onto the physics seam
is future work on top of `PageSnap`; the two hand-rolled engines are not required to converge
with each other first.

**Evidence**: `plugins/shadcn/src/components/carousel.rs` module docs (*Drag, and what ends it*,
*Motion*); `plugins/material/src/carousel/mod.rs` module docs ("Snapping, and the velocity that
picks the target") and `plugins/material/src/carousel/physics.rs` module docs ("Ramp shape: a
duration, not a spring") — no `crate::physics`/`ScrollView`/`ListView` dependency in either carousel.

---

### `page-view-eager-children` — `page_view` builds and keeps every page alive; no builder, no `keepPage`

**Observed**: `page_view` takes its pages up front: every page is built, laid out and kept alive
(paint and semantics skip pages outside the viewport). There is no `PageView.builder`-style lazy
constructor, no `keepPage`, no `PageView.custom`/`reverse`/`allowImplicitScrolling`, and an unbounded
main axis lays pages out at zero extent. `on_page_changed` fires once the page has settled, not
mid-drag as Flutter's does.

**Applies to**: any app using `page_view` for many or heavy pages (a long feed, full-screen live
screens that each keep their own state — those are a navigator or `IndexedStack` job).

**Why accepted**: v1 targets small swipeable sets (onboarding, carousels); a lazy page sliver is
additive follow-up work on the same crate-private page sliver.

**Evidence**: `crates/frust-widgets/src/page_view.rs` module docs (*v1 limits*).

---

### `card-switcher-blocking-bounded` — the card switcher blocks input only within its own bounds

**Observed**: while open (or animating), `card_switcher` swallows pointer input to the shrunken live
child and the stage, but only inside the pattern's own bounds; chrome laid out outside it (an app bar,
a bottom bar hosted around the pattern) stays interactive. Focus-routed input (Key, IME,
EditCommand) is suspended while open: the open flip clears the live child's focus
(`CardSwitcherWidget::suspend_child` saves and clears its focus link) and the close flip restores it,
and focus-routed events reach only a pod that `holds_live_focus()`. `InputEvent::Shortcut` is a
broadcast, though, and `CardSwitcherWidget::event` forwards every broadcast to the live child
regardless of `engaged()`, so a shortcut registered by the live page still fires while the stage is
open.

**Applies to**: apps that place the pattern below app chrome rather than around the whole shell page.

**Why accepted**: the pattern is a paint-and-input stage over one child, not a window-level modal;
wrapping the whole shell page gives full blocking.

**Evidence**: `crates/frust-widgets/src/motion/card_switcher.rs` module docs (*Input blocking*
paragraph), `blocked_pointer`, `suspend_child` (open) / the close-flip restore, and the broadcast
branch of `CardSwitcherWidget::event` in the same file; `InputEvent::is_broadcast` and
`is_focus_routed` in `crates/frust-core/src/event/input.rs`; `holds_live_focus` in
`crates/frust-core/src/widget.rs`.

---

### `card-switcher-peers-no-semantics` — the card switcher's peer cards are absent from the accessibility tree

**Observed**: `CardSwitcherWidget::semantics` emits the live child and, while open, the live-label,
dots and action-bar slots, but never the peer faces or their labels. Peer selection is therefore
pointer-only, and the blocked live child's controls stay in the tree while the stage is open.

**Applies to**: screen-reader and switch-access users of `card_switcher`.

**Why accepted**: v1 scope; peer semantics and hiding the blocked child are follow-up work.

**Evidence**: `crates/frust-widgets/src/motion/card_switcher.rs` (`fn semantics`) and the dots
`Role::Status` node in `plugins/glyph/src/page_dots.rs`.

---

### `card-switcher-tap-only-selection` — peer cards are selected by tap only; the strip has no wheel, vertical or nested-claim behaviour

**Observed**: a fling on the peer strip never selects a card; selection is a tap that the strip did not
take over, and taps during a centring tween are ignored (every pointer event is swallowed until the
callback fires). Wheel input does not scroll the strip, the strip registers no nested scroll claim, and
v1 has no vertical gesture (e.g. swipe-up to dismiss a card).

**Applies to**: any app using `card_switcher` or the Glyph skin; desktop wheel users must tap or drag.

**Why accepted**: tap-only selection keeps the centring tween and the `expanding` handshake
single-writer; the missing gestures are v1 scope, not a design gap.

**Evidence**: `crates/frust-widgets/src/motion/card_switcher.rs` module docs (tap rules, strip), the strip's
`forward_wheel`, and the `CardSwitcherView::expanding` / `on_select` docs (the selection handshake).

---

### `card-switcher-android-strip-ends-clamp` — on Android the card strip's ends stop dead

**Observed**: the peer strip runs `PageSnap::extent(stride)` over `default_physics()`, so on Android
(`Clamping` + `Stretch`) the first and last card stop dead at the ends and no stretch is drawn on the
strip, while iOS and desktop (`Bouncing` + `Translate`) rubber-band. Matches the platform default
elsewhere in the framework; found at the Pixel 5 / iPhone 13 mini / desktop gate.

**Applies to**: Android builds of `card_switcher` and the Glyph skin.

**Why accepted**: it is the platform-adaptive default the scroll surfaces already ship; naming a
physics on the strip would diverge from it.

**Evidence**: `crates/frust-widgets/src/motion/card_switcher.rs` module docs (strip physics);
`crates/frust-widgets/src/physics/mod.rs` `default_physics` / `default_overscroll_effect`;
`PageSnap::extent` in `crates/frust-widgets/src/physics/page_snap.rs`.

---

### `glyph-fab-no-keyboard-activation` — the Glyph `fab` takes no keyboard focus and cannot be activated by key

**Observed**: `frust_glyph::fab` is pointer- and semantics-activatable (`Role::Button`) but is not a Tab
stop and does not answer Enter/Space; press feedback is an instant fill tint.

**Applies to**: keyboard-only desktop users of `fab`.

**Why accepted**: the fab shipped for the touch switcher flow; focus and key activation are
follow-up work.

**Evidence**: `plugins/glyph/src/fab.rs` (no focus or key handling in `FabWidget`).

---

### `scroll-physics-claim-down-snapshot` — nested-scroll arbitration is a `Down`-time snapshot with no mid-gesture handback

**Observed**: the ambient inner-scroll claim (`InnerScrollState`, `scroll.rs`) is written once, as a
nested scrollable reports itself while its host forwards the gesture's `Down`, and the outer's own
defer decision (`deferring`) is sticky for the rest of that gesture. Content that becomes scrollable
mid-drag (e.g. new rows load in) or an inner surface reaching an edge it wasn't at on `Down` never
changes the arbitration outcome; a gesture the outer already took over can never hand it back to a
newly-eligible inner, and vice versa.

**Applies to**: any nested-scrollable layout using the baseline `ScrollView`/`ListView`'s built-in
arbitration; a single stationary `Down`-time state decides the whole gesture.

**Why accepted**: the same Down-time-snapshot tradeoff the navigator's own edge-swipe arming already
lives with (`nav::ambient`'s `SWIPE_CLAIM`) — arbitrating once at `Down` is what
keeps the takeover decision a single read rather than a per-`Move` re-evaluation; the fix is a
strictly larger seam (a live-updating claim), not a bug in the one shipped.

**Evidence**: `crates/frust-widgets/src/scroll.rs`'s `SCROLL_CLAIM`/`InnerScrollState` module docs
(*Nested scrolling: innermost wins*); the `deferring`/`inner_at_down` fields of `ScrollSurfaceCore`
in `scroll_core.rs`.

---

### `scroll-physics-rubber-band-legacy-path-and-easeback` — `RubberBand` keeps the pre-seam fling/settle path, and its ease-back is now path-dependent

**Observed**: `RubberBand::create_ballistic_simulation` always returns `None` by design, so a
`RubberBand`-installed surface still runs both widgets' pre-seam hand-rolled fling/settle code
rather than the generic `Simulation`-driven ballistic path every other physics uses — unifying the
two is recorded, unstarted follow-up work, not a behavior-preserving detail of the extraction.
Separately, under the surfaces' per-move drag convention (every `Move` mapped against the *live*
position), a pull-and-return within one gesture no longer lands exactly at rest: `RubberBand` has no
easing/tensioning split of its own, so it resists the past-edge part of a *returning* delta too,
leaving the surface slightly scrolled after the finger comes all the way back.

**Applies to**: any app opting into `.physics(RubberBand::new())`; every shipped one-shot pull
number (refresh trigger, stretch intensity) is unchanged — only a pull-and-return within a single
gesture lands off-rest.

**Why accepted**: `RubberBand` exists to preserve the exact pre-seam feel where the mapping is
linear, not to gain the generic ballistic driver — unifying it is future work. The ease-back delta
is the one place the new per-move convention and the old whole-excursion one genuinely part ways,
pinned rather than papered over.

**Evidence**: `crates/frust-widgets/src/physics/rubber_band.rs` module docs (*What lives here, and
what deliberately does not*, *Under the surfaces' per-move drag convention*) and its
`per_move_deltas_telescope_to_the_pre_seam_pull` test.

---

### `scroll-physics-min-fling-ladder-disagreement` — a release below a physics' fling minimum still glides, on the legacy fallback's own threshold

**Observed**: a release velocity below the installed physics' `min_fling_velocity()` (e.g. below
`Bouncing`'s 100 px/s) makes `create_ballistic_simulation` decline, but the release does not simply
stop — it falls through to the widgets' legacy fling path, which is gated on its own, lower, pinned
`FLING_STOP` threshold (30 px/s) instead. A release between the two (e.g. 60 px/s under `Bouncing`)
therefore still glides, just on the legacy curve rather than the physics' own.

**Applies to**: any physics whose `min_fling_velocity()` sits above `FLING_STOP`, on a release that
lands strictly between the two thresholds — today `Bouncing` (100 px/s minimum) is the shipped case.

**Why accepted**: the two ladders serve different purposes — the physics' minimum gates its own
ballistic simulation, `FLING_STOP` gates the separate legacy fallback that every non-simulation-
returning physics (and any in-range release) still runs — and the disagreement is pinned by a test
rather than silently drifting; closing the gap is a product-feel call (should a sub-minimum release
glide at all), not a bug fix.

**Evidence**: `crates/frust-widgets/src/scroll.rs`'s `default_min_fling_gate_is_one_hundred` test.

---

### `scroll-physics-stationary-hold-momentum-not-ported` — a finger that pauses before releasing still carries momentum

**Observed**: Flutter's `ScrollDragController.end` drops carried momentum a third way beyond the
sign/magnitude gate — `_maybeLoseMomentum` — when the finger held still before letting go. Both
widgets' `fling_start_velocity` port only the sign and magnitude guards; a press that stalls live
motion, pauses, then releases slowly in the same direction still adds the interrupted motion's
momentum here.

**Applies to**: any release that interrupts an in-flight fling/ballistic, pauses, and then lets go
in the same direction the interrupted motion was travelling.

**Why accepted**: a smaller, explicitly recorded gap rather than an unnoticed one — the sign and
magnitude guards are the load-bearing pair (they prevent a flick back the other way from inheriting
momentum it should cancel); the stall guard is a refinement on top, not ported.

**Evidence**: `crates/frust-widgets/src/scroll_core.rs`'s `fling_start_velocity` doc comment
(*Accepted gap*).

---

### `scroll-physics-ballistic-early-stop-conservatism` — a partial boundary rejection or inward velocity still pumps to `is_done`

**Observed**: `ballistic_is_pinned_outward` (both widgets) only ends a simulation early when the
physics rejects the *whole* excess and the curve's velocity still points further out of range; a
*partial* rejection or an inward velocity each keep the simulation running the rest of its frames,
even though most such cases can never bring the offset back on screen either. Separately in
`ListView`, `max_offset` is a converging estimate in variable-extent mode, so a fling stopped
against an under-estimated end stays stopped rather than resuming once later row measurements push
the real end further out.

**Applies to**: any ballistic release ending near a boundary under a non-fully-rejecting physics
(the `Bouncing` family); the `max_offset` caveat applies only to a keyed `ListView` with
`.estimated_item_extent(px)` set.

**Why accepted**: deliberately conservative rather than guessed at — a physics whose rejection
merely rounds to the excess, or an edge spring released outward that crosses back within a few
frames, both need the simulation still running; only the fully-rejected-and-still-outward case is
one-way for every curve in `crate::physics::simulation`. The `ListView` estimate case is a strictly
smaller version of the same tradeoff windowed virtualization already accepts elsewhere.

**Evidence**: `crates/frust-widgets/src/scroll_core.rs`'s `ballistic_is_pinned_outward` doc comment.

---

### `engine-invalid-geometry-refusal` — the engine refuses a frame with non-finite geometry rather than drawing it

**Observed** (evidence: `crates/frust-engine/tests/proptest_strips.rs`'s
documented reproducer sites and the measured numbers recorded there):
`frust-engine`'s `SceneCompiler::compile` validates every lowered
command's geometry up front (rect extents, corner radii, path points, stroke
width, dash on/off/phase must be finite — and an effective dash cycle must
also be *normalizable*: its derived period `on + off` finite and positive with
the phase reducible into it, since two individually finite lengths can
overflow the period to `+inf` and spin kurbo's dash iterator forever) and
refuses the frame with `EngineError::InvalidGeometry` otherwise — required for totality: a `NaN`
corner radius on an unbounded rect made `compile` never return, and a `NaN`
stroke width cost ~420 ms/4.8 MB producing zero strips. There is no other
renderer to fall back to, so a scene that emits non-finite geometry (a `NaN`
dash pattern included) simply refuses that frame rather than drawing
anything.

**Accepted because**: the inputs a refusal fires on are already erroneous
(non-finite geometry describes nothing drawable); refusing loudly beats
hanging or panicking in kurbo. The up-front check cannot refuse finite values,
so magnitude is bounded downstream instead: compile cost for a fill, clip
mask, glyph outline, rounded rect or stroke is bounded for every finite input
(`compile/cull.rs`; pinned by `curve_carrying_commands_at_extreme_magnitudes_compile_within_bounds`
and `nested_scales_reaching_extreme_magnitudes_compile_within_bounds`). Output
is exact to the flattening tolerance up to the device extent `cull.rs` states
(about 9.3e27 pixels for a fill; a stroke's device reach about 3e10 pixels);
past it a depth-capped piece is drawn as its chord, with error confined to
that piece, and near `f32`'s range cost stays bounded but drawing is no longer
faithful. A dashed stroke is bounded the same way: it is dashed whole only
where the undashed stroke `fits_the_stroker` and no element exceeds
`MAX_DASHES_PER_ELEMENT` (65 536) periods; otherwise it takes the bounded
lowering, which dashes only the stretch that can reach the expanded viewport,
the pattern's phase carried across the skipped length (to within
`DASH_ARCLEN_RELATIVE_ACCURACY` of a piece's control-polygon length, which moves
where dashes fall, not whether the stroke draws). There a run is stroked solid,
losing the pattern's duty cycle, when the device period (period times the
transform's largest singular value) is under `MIN_DEVICE_DASH_PERIOD` (0.5 px)
or one of its spans is longer than `MAX_DASHES_PER_ELEMENT` periods; total dash
work is bounded by the kept spans, a few per element, times that budget. An
oversized stroke is expanded at a
device-space tolerance, so its outline stays within 0.25 px of the exact outline
under any affine until its device reach passes about 3e10 pixels.

**Trigger for removal**: a widget-visible need for a drawn-anyway fallback
semantics for non-finite input appears (none identified today).

---

### `engine-image-minify-to-fit` — an image wider than one atlas layer renders minified

**Observed** (evidence: `crates/frust-engine/src/cache/images.rs`'s
`fit_extent`/`minify` and `crates/frust-engine/tests/images.rs`): the engine's
image path is atlas-resident, and a source larger than one atlas layer
(tier-chosen per-layer extent — mobile 1024², `mobile_large` and desktop 2048² — overridable
via `FRUST_ENGINE_ATLAS_SIZE`) is downscaled to fit with an aspect-preserving
box filter at upload time, logged once per image. The filter averages
sRGB-encoded premultiplied bytes — exact for a uniform-colour source, very
slightly dark on high-contrast content.

**Accepted because**: cross-frame residency is the engine image path's whole
design, and refusing an oversized image outright would turn a working widget
blank on the engine tier for a size cliff the author cannot see; minifying is
what comparable atlas-based renderers do, and the display size of an image
that large is almost always far below its source resolution anyway. A
linear-light filter would need a transfer-function table this crate does not
carry.

**Trigger for removal**: tiled residency (splitting an oversized source across
atlas rectangles) or a dedicated non-atlas texture path for oversized sources;
either removes the ceiling rather than softening it.

---

### `engine-padded-bilinear-edge-blend` — a magnified Pad image's last row and column blend half-way into the neighbouring texel

**Observed**: in both strip program families (the manual four-tap path and the hardware sampler),
the engine's padded bilinear clamps the sample position to the image before taking its half-texel
offset, so the last row and column of a magnified Pad image blend half-way into the texel beside
them where the CPU oracle (`vello_cpu`) holds the edge texel. It is invisible at the corpus's 2x
checkerboard (two-pixel cells) and reaches a delta of 128 at 4x magnification on a one-pixel
pattern.

**Applies to**: the engine tier, a bilinear (Medium-quality) atlas image drawn magnified with the
Pad extend mode.

**Why not fixed**: measured, not yet ruled on: the delta only appears where a magnified image's edge
texel differs from its neighbour, which the corpus's two-pixel cells never exercise. Correcting it
changes the filter's edge rule in both program families at once.

**Evidence**: `unit-image-magnified` (`frust_testing::corpus::unit::image_sampler_cases`) is a
measurement case with no reviewed baseline; its source repeats the last row and column so the case
measures the filter rather than this edge rule. Shader: `sampled_bilinear` in `strip.wgsl` and
`bilinear_sample` in `helpers.wgsl`.

---

### `engine-scheduler-skip-on-escalation` — a layer shape the engine's scheduler cannot serve skips the frame rather than falling back

**Observed** (evidence: `crates/frust-engine/src/schedule/mod.rs`'s module
contract, `EngineRenderer::prepare`, and `SurfaceRenderer::encode` /
`TierBackend::present_refused_frame` in `crates/frust-render/src/renderer.rs`):
the engine tier carries no second renderer — a frame whose layer shape the
scheduler cannot serve is refused with `EngineError::SchedulerEscalation` and
the surface shows nothing new for that frame, a refusal counter increments
(`SurfaceRenderer::engine_refusals()`), and a rate-limited log names the
reason. Every plan-stage refusal is decided in `SurfaceRenderer::encode`
(`EngineRenderer::prepare`) before the swapchain is acquired, so
`encode` returns `Skipped` and no texture is held: the last presented frame
persists on every backend. Only a capacity refusal found while recording
(`AlphaCapacity`, `PaintCapacity`, when the held plan matches), or a re-plan
(missing or stale plan: a texture binding that changed an id's extent, a resize,
the retained-store or atlas setters, a different target; any `prepare` error can
then surface), lands after acquire. The main surface presents the acquired image
(`RefusalBackstop`): the previous frame re-converted on the straight-alpha arm, a
base-colour clear on the direct arm; no overlay is recorded that frame. An
overlay whose record is refused keeps its acquired texture un-presented in
`reserved`, so its previous frame stays and its next record draws into it.
A post-acquire refusal therefore costs at most one frame, but on the direct arm
that frame is the clear, so a sustained capacity refusal flashes the base
colour every frame (accepted; the alternative, dropping the texture, leaks a
swapchain image — `wgpu-vulkan-discard-texture-noop`). Since the shape is a
property of the layer tree, a shape that refuses once refuses every frame: the
surface holds its last presented image while the app keeps running.

The served set is wider still with the spill pool (`MAX_SPILL_PAGES` = 3 slots): isolated sibling fans of any width, a nested
isolated chain of any depth up to the chain bound, same-parity sequential page
reuse, a chain hanging off an isolated ancestor's later child (a full-screen
layer at fractional opacity holding a chip that itself carries a nested chip,
the navigation-scrub shape), and every tree whose isolated layers hold at most
two isolated children each (inlined (opacity-one) layers are transparent to the
count: an isolated layer's children include the isolated layers recorded inside
its inlined descendants; the nested translucent fan `P[Q, L[C1, C2]]` of a card
switcher's dimmed peers included) are all served within `MAX_LIVE_PAGES` (2 + `MAX_SPILL_PAGES` = 5)
live pages; a single-primitive Gaussian blur or a
shadow-only drop shadow is planned and executed as a filter-round sequence,
sized by `filter_page_size` (coverage plus the filter padding, quantized,
refusing with `IntermediateTextureTooLarge` past the adapter ceiling and
`IntermediateTextureLimitReached` past the pool's own budget). What still
refuses: a chain deeper than the chain bound; a wider fan under an ancestor
already holding a page, once its pending children outnumber the spill slots
(the smallest refused shapes have seven layers, `root[A, B[C1..C5]]` and
`P[Q, L[C1..C4]]`), which needs more than the 5 live pages; a
filter that is not one of the two served kinds (flood and the
composite-original drop shadow are reserved — the latter needs a third live
page); a filter layer recorded inside another layer; and non-default blend or
mask layers.

**Accepted because**: the engine carries no second renderer to fall back to on
one surface; skipping loudly beats rendering the shape wrong, and a refusal
decided before acquire leaves the previous frame on screen.

**Trigger for removal**: the served set must eventually cover the widget
tree's real shapes — the opaque-pass hoist and the bounded spill pool have
each narrowed the refused class in turn rather than closed it; the general
algorithm this narrows from is a stub behind the `full-scheduler` feature.

---

### `wgpu-vulkan-discard-texture-noop` — a dropped un-presented swapchain texture leaks its image on Vulkan

**Observed** (evidence: wgpu-hal 30.0.1's Vulkan `discard_texture`, a no-op; upstream
[gfx-rs/wgpu#5723](https://github.com/gfx-rs/wgpu/issues/5723); the frust-side backstops in
`crates/frust-render/src/renderer.rs` and `crates/frust-gpu/src/lifecycle.rs`): dropping an acquired
`wgpu::SurfaceTexture` without presenting it never returns the swapchain image to the
presentation engine on Vulkan, so each such drop permanently consumes one image and a few
of them freeze the swapchain (the Android symptom was an app frozen on its last frame).

**Applies to**: the Vulkan backend (Android and Linux), any path that acquires and then abandons
a frame.

**Why accepted**: the defect is upstream; frust works around it instead. No acquired
texture is dropped while its swapchain lives: a post-acquire refusal presents a
backstop frame on the main surface (`FrameOutcome::Skipped` with the texture still
presented) or keeps the texture un-presented on an overlay, and refusals the plan can foresee are
decided before acquire so nothing is held. An acquire `Timeout`/`Occluded` run reconfigures the
swapchain after `MAX_TRANSIENT_SKIPS` (3) consecutive skips (`decide_acquire_streaks`,
`next_transient_streak`; the main surface and overlays alike) so a wedged swapchain is rebuilt
rather than skipped forever. frust relies on a reconfigure or resize recreating the
swapchain to release the images it still holds (that reconfigure, and
`OverlaySurface::resize`, which drops pending/reserved/recorded first); whether a
rebuild also reclaims an image leaked before this fix is driver-dependent
(unverified on Adreno), and no normal path leaks one now. The
cost is at most one backstop frame per post-acquire refusal (see
`engine-scheduler-skip-on-escalation`), and an occluded window reconfigures on every fourth
acquire attempt.

**Trigger for removal**: a wgpu release whose Vulkan discard returns the image (for example via
`VK_EXT_swapchain_maintenance1`); then the present-anyway backstop can be revisited.

---

### `engine-glifo-atlas-experimental` — the engine's glyph atlas is built on a cache upstream labels experimental, wrapped in frust-owned policy

**Observed** (evidence: glifo 0.3.0 `glyph.rs`'s atlas-cache doc — "highly
experimental and not recommended for external use" — and
`crates/frust-engine/src/text/atlas_policy.rs`): the engine caches settled
glyph runs in a GPU atlas driven by glifo's cacher, which keys font size by
exact `f32` bits and leaves eviction and layout policy to the integrator.
frust's own routing policy decides which runs are offered, keyed on the
quantized (1/4 px) device-space size. A run goes to outline strips when its
device size changed inside an 8-frame window (a size that once held still is
remembered for 600 frames, at most 16 per font, so a sweep of ever-new sizes
keeps animating on outlines), when it is above the size ceiling, when its face
carries COLR, when its transform is not a positive uniform scale, or when it
would push the resident population past the glyph texel budget. Admission is
sticky: a run whose every glyph is already resident keeps being served, and
LRU eviction drains any excess, so a population over its share does not thrash
(iPhone 13 mini, S6 at 3x: 0 of 527 glyphs resident without it, 527 of 527 with
sticky admission on the `mobile_large` tier).

The budget is an over-estimate kept from admit and evict deltas, not a texel
guarantee: glifo exposes no release-build way to price the resident population,
a glyph wider than its em is under-charged, and a run is charged its distinct
glyph ids at admission, so subpixel-bucket variants can overshoot. A glyph the
atlas cannot place is reported as `unrasterizable` or `no_room`.

The `mobile_large` memory tier (taken on iOS from 3 GiB of `hw.memsize`,
elsewhere from 4 GiB) raises the glyph atlas from 16 to 64 MiB and retained
pages from 32 to 64 MiB; on an iPhone 13 mini it cost about 22 MiB of
`phys_footprint` on S6 (p50 229.9 MiB plain, 252.0 MiB `mobile_large`), the rest
of the 80 MiB being a lazy ceiling. A Critical memory event (the iOS memory
warning; on Android RUNNING_CRITICAL in the foreground, COMPLETE or
`onLowMemory`) drops retained pages to 32 MiB and clamps atlas growth to the
plain tier's bytes, one-way until the renderer is rebuilt. The atlas cannot give
a layer back (`vello_common` 0.2.0's packer only appends), so a trim only stops
growth and resident glyphs keep serving. Android 14+ itself sends only UI_HIDDEN
and BACKGROUND, so there the trim runs only on `onLowMemory`; macOS reports the
event without trimming. On-device delivery of the trim has not been exercised.

The resident glyph-run cache (`ResidentRuns`) restates glifo 0.3.0's private
placement arithmetic and mirrors its eviction schedule, which is part of why
glifo is exact-pinned; a mispredicted eviction costs one frame of probes, never a
pixel. `FRUST_ENGINE_NO_ATLAS` routes every glyph to the outline path, a correct,
slower fallback.

**Accepted because**: the outline fallback is always live (animating sizes,
oversized glyphs, refused allocations and the kill switch all take it), so an
atlas defect degrades to the slower correct path rather than wrong pixels, and
the policy layer pins every behaviour upstream leaves open.

**Trigger for removal**: glifo stabilising its atlas surface, or the engine
absorbing the atlas core itself.

---

### `engine-hvgl-atlas-pin-bound` — hvgl glyphs reach the atlas through code that restates private glifo 0.3.0 internals

**Observed** (evidence: `crates/frust-engine/src/text/hvgl_atlas.rs`; iPhone 13 mini and M4 Metal
device runs): glifo cannot read the outlines of an `hvgl`-only face
(Apple's PingFang), so the engine inserts those glyphs into the glyph atlas itself. `hvgl_atlas.rs`
restates, over glifo's public API, the private steps glifo runs for an outline it caches: the raster
extent (`raster_metrics`, glifo 0.3.0 `renderer.rs:591`), recording the black outline fill into the slot
(`record_outline`, `renderer.rs:342`) and drawing the slot as a tinted coverage mask (`draw_from_atlas`,
`renderer.rs:521` and `:311`). The restatements are correct for the exact-pinned `=0.3.0` only: a glifo
bump that moves one would put these glyphs on other pixels than glifo's own without failing to compile.
The tripwire is `crates/frust-engine/tests/hvgl_atlas_parity.rs` (PingFang atlas against outlines at
whole-pixel and subpixel origins, and a CFF Hiragino control glifo inserts itself); it needs macOS Metal and
is `#[ignore]`d, so CI on ubuntu does not run it: re-run it by hand with
`cargo test -p frust-engine --test hvgl_atlas_parity -- --ignored --test-threads=1` on every glifo bump.
`crates/frust-testing/tests/dup_identities.rs` (`hvgl_restatement_is_attested_for_the_locked_glifo`) fails on
ubuntu CI when Cargo.lock's glifo differs from `HVGL_PARITY_VERIFIED_GLIFO`, forcing the re-diff to be
acknowledged; it is an attestation, not proof the Metal run happened.
`SceneCompiler::set_hvgl_atlas(false)` routes the glyphs back to outlines
(`OutlineReason::Hvgl`). Measured: PingFang 61 atlas draws and 0 outline fallbacks per window on an iPhone
13 mini (S6, 3x), glyphs_us 5,377 -> 183, CPU work per frame 6,146 -> 1,437 us, presented cadence
unchanged; a 300-glyph PingFang paragraph compiles in 9.7 us instead of
613 on an M4.

**Applies to**: Apple targets drawing `hvgl`-only faces; any glifo version change.

**Accepted because**: the alternative is the outline route, which re-fills every glyph every frame; the restated code is small, each function names its
source line, and the parity test pins it.

**Trigger for removal**: glifo exposing an outline-provider seam for its atlas cache (so the insert can
use glifo's own), or an automated macOS run of the parity test on every glifo bump (the CI check above
acknowledges a bump; it does not run the test).

---

### `engine-bitmap-glyphs-gap` — some bitmap-strike emoji records and run shapes do not render on the engine tier

**Observed** (evidence: `crates/frust-engine/tests/text_bitmap_strike.rs`,
`crates/frust-engine/tests/apple_raster.rs`, and the Mac (sbix), Android (CBDT) and iPhone gates; the
iOS route is covered by iOS-simulator tests; the iPhone device gate passed):
bitmap-first faces (`sbix` with `DRAW_OUTLINES` clear, or `CBDT`/`EBDT` with no outlines) bypass
glifo for `text::bitmap_route`. CoreText draws every `sbix` glyph on iOS; elsewhere the engine
decodes png, dupe and flip records. Every PNG strike decode is the engine's own and refuses a strike
over 512 px before decoding (glifo's `png` feature is off). What remains missing, counted in
`skipped_glyphs` rather than drawn wrong:

- `emjc` and other non-PNG `sbix` records off iOS; non-PNG `EBDT` strikes; BGRA and Mask strikes.
- A bitmap run under a non-uniform scale or skew draws only already-resident strikes.
- A bitmap run past 1024 px.
- A face with outlines and strikes (`sbix` with `DRAW_OUTLINES` set, `CBDT`/`EBDT` with `glyf`/`CFF`,
  COLR with strikes) draws its outlines, not its strikes.

COLRv1 colour glyphs remain drawn by `crates/frust-engine/src/text/color.rs`. Within COLR, a glyph
whose paint graph needs a shape the engine cannot express exactly — a layer inside an isolated blend
bracket, or two nested outline clips at once — is dropped whole and counted, never half-painted.

**Accepted because**: a wrong-pixels approximation of an inexpressible shape would violate the
backend's never-wrong-pixels invariant, and the target platforms' emoji fonts ship PNG strikes,
CoreText-decodable records on iOS, or COLR.

**Trigger for removal**: an engine decoder for `emjc`/BGRA/Mask records, or a measured need for a
bitmap-strike font on a skewed or oversized run.

---

### `engine-text-hinting-policy` — hinting is desktop-only and vertical-only; the reference oracle never hints

**Observed** (evidence: `crates/frust-engine/src/compile/mod.rs`'s
`hint_text` from `for_caps`, `crates/frust-engine/src/text/mod.rs`'s
`lower_glyph_run`, glifo 0.3.0 `GlyphRunBuilder::hint`): the engine asks
glifo to hint only when the device class is desktop
(`hint_text = !is_mobile_tier(caps)`); glifo then applies vertical-only
autohinting, and only when the run's full transform is a positive uniform
scale without vertical skew — any other transform falls back to unhinted.
Mobile stays unhinted (≥2x DPR gains nothing and hinting doubles cache
keys), and there is no LCD subpixel rendering on any tier. The `vello_cpu`
golden oracle never hints, so on a desktop-class adapter the engine's text
pixels legitimately differ from the oracle's by a fraction-of-a-pixel grid
snap per glyph edge; the text goldens carry per-case escalation rows
measuring exactly that divergence rather than loosening the corpus budget.

**Accepted because**: hinted small text is measurably crisper on low-DPR
desktop displays, the divergence is deterministic and byte-stable
frame-to-frame, and the unhinted lowering stays fully exercised (mobile,
every oracle, and the dedicated hinted-vs-unhinted stability tests).

**Trigger for removal**: an `EngineRenderer` hint override letting parity
gates compare unhinted-vs-unhinted, or a
DPR-aware policy replacing the binary device-class switch.

---

### `engine-metal-postmultiplied-truth-bug` — wgpu-hal's Metal `PostMultiplied` composites premultiplied regardless of its name

**Observed** (evidence: `crates/frust-gpu/src/surface.rs`'s
`compositor_expects_premultiplied` doc comment, citing
`wgpu-hal-30.0.1/src/metal/adapter.rs:468-471` and
`.../metal/surface.rs:269-273`, checked against the 30.0.1 pin):
wgpu-hal's Metal adapter still advertises `CompositeAlphaMode::PostMultiplied`
but implements it as nothing beyond `render_layer.setOpaque(false)` — it never
asks Core Animation for straight alpha, and a `CAMetalLayer` has no such mode;
Core Animation only ever composites premultiplied. A Metal `PostMultiplied`
swapchain therefore reads back exactly like a premultiplied one even though
the mode's name and wgpu's advertised contract say straight: a real
capability wgpu-hal misreports, not a Frust defect. `frust-render::context`'s
`choose_engine_render_path` corrects for it: Metal + `PostMultiplied` (iOS's
sole translucent mode) routes to `RenderPathKind::EngineDirect` — the
engine's already-premultiplied output served as-is — rather than the
spec-correct straight-alpha conversion, which would double-correct
(over-brighten every partial-alpha pixel; an indigo/navy wash near a
translucent split). Every other `PostMultiplied` backend (e.g. Vulkan's
genuinely-straight `POST_MULTIPLIED` flag) keeps the ordinary conversion.

**Accepted because**: fixing the misreport belongs to wgpu-hal, not this
workspace; the workaround is a pure `(backend, mode)` predicate with no
per-frame cost and no user-visible caveat.

**Trigger for removal**: upstream trunk PR [gfx-rs/wgpu#9922](https://github.com/gfx-rs/wgpu/pull/9922)
(merged 2026-08-18) already fixes this — Metal will
advertise `PreMultiplied` and reject `PostMultiplied` with `UnsupportedAlphaMode`
— but it is not in the 30.0.1 pin. The next wgpu bump that carries it is the
trigger: `compositor_expects_premultiplied` collapses to nothing (and, being a
BREAKING wgpu-hal change, needs its own re-scrub of every backend/mode
combination this predicate routes on). No upstream issue is filed by this
workspace.

---

### `engine-punch-straddles-layer-composite` — an isolated layer whose content straddles a `ClearRect` composites whole after the punch

**Observed** (evidence: `crates/frust-engine/src/compile/clear.rs`'s module
doc, point 3's "One shape the cut cannot express"): the renderer orders a
`ClearRect`'s destination-out punch against the frame's own rounds by cutting
the surface round at the punch's painter-order depth. An isolated
layer reaches the surface as one composite carrying its deepest instance's
depth, so a layer holding content recorded both before and after a clear
composites AFTER the punch as a whole — the half recorded before the clear
survives, where a reference renderer that closes and reopens the bracket
around the punch erases it. The engine's one-composite-per-layer model, not
the punch pass's own ordering.

**Accepted because**: the shape frust's own widget tree does not record —
`ClearRect` comes from `platform_view`, never mid-animation inside an
isolated layer; serving it exactly needs per-sub-range layer compositing,
which the scheduler's one-pass-per-layer design deliberately does not carry.

**Trigger for removal**: the general scheduler (the `full-scheduler` feature), if it composites a
layer in more than one pass.

---

### `engine-wasm-single-thread` — the pipeline warm-up queue has no worker thread on `wasm32`, so warm-up drains synchronously there

**Observed** (evidence: `crates/frust-gpu/src/pipeline.rs`'s
`VariantCache::warm_up` — `std::thread::Builder::spawn` and its existing
`Err(err)` fallback that drains the queue inline instead;
`crates/frust-engine/tests/{present,atlas_render,encode_contract,
desktop_stress,scrub_served}.rs`'s `#![cfg(not(target_arch = "wasm32"))]`
gates, which already anticipate a wasm target for this crate):
`PipelineCache::warm_up` normally spawns one background OS thread to compile
a frame's variant descriptors ahead of first use, so a request still queued
when needed is stolen and built inline rather than waited on (the
"eager-steal" design `docs/RENDER_ARCHITECTURE.md`'s `frust-gpu::pipeline`
row describes). `std::thread::Builder::spawn` returns `Err` wherever real OS
threads are unavailable — `wasm32-unknown-unknown` without the unstable
`atomics`/`bulk-memory` target features and a Worker pool — and that path
already falls back to draining the queue synchronously on the calling
thread. Correct, but on wasm every "warm-up" call is a blocking compile with
no background overlap; the mechanism buys nothing there, it only avoids
silently skipping the work. **Measured** (evidence: `examples/web-spike/RESULTS.md`
§ 16.3, three cold runs per arm in Chrome): the first `submit` after bring-up —
the one that pays this synchronous fallback — costs 10.2ms median on WebGPU and
16.7ms median on WebGL2, against a 0.6-1.4ms steady-state `submit` on both
backends once every variant is compiled — roughly 13-19x, once, not a per-frame
cost. The console line this fallback prints (`frust-gpu: could not spawn the
pipeline warm-up thread ...; building the listed variants inline`) is recorded
verbatim in that section's transcripts.

**Accepted because**: single-digit-to-low-double-digit milliseconds, paid once
at bring-up and never per frame, is the same order of magnitude the desktop
tiers already pay for their own first-use pipeline compile — the fallback is
correct by construction and measured cheap.

**Trigger for removal**: a `SharedArrayBuffer`/Worker-backed pool that gives
`wasm32` a real background thread for this queue, if the measured cost above
ever needs to shrink further.

---

### `engine-scene-texture-always-blended` — an unhinted external texture is blended, and opacity is caller-asserted

**Observed** (evidence: `crates/frust-engine/src/compile/external.rs`,
`crates/frust-engine/tests/scene_texture.rs`): the engine never reads a
caller's texels. A `SceneTexture` bound with `SceneTextureHints::opaque` set
(`EngineRenderer::bind_texture_with`, `ExternalFrame::bind_texture_with`)
routes like an opaque image, but the claim is the caller's: an "opaque" texture
with alpha below one occludes instead of blending. Every `ShaderQuad` draw and
every `SceneTexture` bound without the hint (plain `bind_texture`, every
texture-bus frame, `beui`'s 3D faces) still takes the blended painter-order
path, paying the blend and forgoing depth occlusion of what lies beneath it.
Inside an isolated layer (alpha below one) even an opaque-hinted texture
blends offscreen, as an opaque image does.

**Accepted because**: claiming opacity the engine cannot verify would let a
translucent texture occlude what it should have blended over; the hint puts
that claim with the one party that knows, and the default draws correctly.

**Trigger for removal**: `ShaderQuad` and the texture bus gaining an opacity
statement of their own, or the engine verifying a texture's alpha. Colour-space
handling stays deferred: texture bytes pass through unconverted, as before.

---

### `engine-frame-clears-colour-unconditionally` — content painted into the target ahead of the frame keeps its depth and loses its pixels

**Observed** (evidence: `crates/frust-gpu/src/encoder.rs`'s caller rules;
`crates/frust-engine/tests/shared_encoder.rs`): a foreign pass may share the
frame's encoder and depth attachment — depth-clear ownership belongs to
whoever records first — but `EngineTarget` has no colour load-op axis, so the
frame always clears its colour target. A 3D pass recorded *before* the 2D
frame therefore still occludes the frame where it wrote nearer depth, yet its
own pixels are gone; the working shape for 3D-under-2D today is to record the
3D pass *after* the frame (nearer fragments draw over it).

**Accepted because**: the colour clear is what keeps a surface frame
self-contained; adding a load-op axis is a contract change that belongs with
the first real 3D consumer.

**Trigger for removal**: a colour load-op on `EngineTarget` proven by the
shared-encoder tests in both recording orders.

---

### `gpu-feature-request-forfeits-android-preinit` — an app that requests optional GPU features loses the Android pre-init overlap

**Observed** (evidence: `crates/frust-shell-android/src/ffi_support.rs`'s `resolve_preinit`,
`crates/frust-shell-android/src/jni_glue.rs`'s `preinit_has_requested_features`,
`crates/frust-gpu/src/context.rs`'s `missing_requested_features`): the load-time pre-init device is
built before the app's `@pre_init` request can shape it, so it is kept only when it already has every
requested feature its adapter offers. Otherwise the shell logs once at info and builds a fresh device
synchronously, which means an app using `app!(.., gpu_features = {..})` pays the full device build on
the first frame of every Android launch.

**Accepted because**: correctness (the requested feature is present) outranks the overlap, and an
app that requests nothing keeps the overlap.

**Trigger for removal**: building the pre-init device with the request already known (or an
asynchronous rebuild that does not block the first frame).

---

### `texture-bus-second-surface-keeps-a-stale-view` — with two live engine-tier surfaces, only the first to drain takes a texture-bus source's new frame

**Observed** (evidence: `crates/frust-render/src/texture_bus.rs`'s `SourcePass::record` calls the
source closure once per `record`; `replace_shown` retires a replaced native hold behind one fence and
then runs its release callback): each engine now clears its own binding when a registration is
dropped (per-surface `ExternalPassDrain`), but a source's *frame* is still consumed by whichever
surface drains first. Another surface's `record` sees no new frame and still holds a binding, so it
keeps sampling the view it last bound until it is itself first to take a fresh frame — with one
surface always draining first, never. For a native payload that view outlives the release callback,
so the other engine samples a buffer already handed back to its producer.

**Accepted because**: every shipping shell drives exactly one engine-tier surface
(`desktop-single-window`; Android and iOS have one surface), so no second drain exists today.

**Trigger for removal**: the first shell that keeps two engine-tier surfaces live at once; the fix is
per-surface binding of the current shown view keyed by `ExternalFrame::drain_id`, with a native hold
released only after every surface that bound it has moved on.

---

### `external-pass-panic-isolation-dev-only` — a panicking `ExternalPass` is only isolated in a build that unwinds

**Observed** (evidence: root `Cargo.toml`'s `[profile.release]` sets `panic = "abort"` (also recorded
in `docs/PERFORMANCE_BASELINES.md`'s Release-profile hardening); `crates/frust-render/src/external_pass.rs`'s
`run_external_passes` wraps each pass's `record` call in `catch_unwind`, reporting the first panic of
an id at `warn!` and every later one at `debug!`, retiring the pass so each engine's drain
unbinds it): `catch_unwind` keeps a panicking pass from taking the frame's encoder — and every sibling
pass, and the scene recorded after it — down with it, but only when the binary can actually unwind.
Under this workspace's `release` profile, `panic = "abort"` means the process is gone before
`catch_unwind` (or any other guard) ever runs, so a panicking pass aborts the process exactly like any
other release-build panic; the isolation this module provides is a debug/dev-build net, not a shipped
guarantee.

**Accepted because**: `panic = "abort"` is workspace-wide release-profile policy that predates this
seam; the shipped contract stays "a pass must not panic," and `catch_unwind` exists to make that
easier to debug during development, not to promise recovery in a release binary.

**Trigger for removal**: none planned — only a workspace-wide move off `panic = "abort"` in
`[profile.release]` would change this, and no such change is proposed.

---

### `external-pass-bind-not-transactional-with-frame` — a refused engine frame still keeps every pass's `bind_texture` side effect

**Observed** (evidence: `crates/frust-render/src/renderer.rs`'s `submit_impl`, both
`RenderPath::EngineDirect` (~1234-1249) and `RenderPath::EngineDirectUnpremultiply` (~1360-1375) arms:
on `EngineRenderer::encode_traced` returning `Err`, the arm returns `FrameOutcome::Skipped` and drops
the encoder un-submitted, so nothing that frame's passes recorded ever reaches the GPU): the refusal
variants are production-reachable, not test-only — `EngineError::AlphaCapacity`/`PaintCapacity` when a
frame's coverage or paint data outgrows its GPU texture, `EngineError::SchedulerEscalation` when a
layer shape exceeds what the scheduler serves (`crates/frust-engine/src/error.rs`). `run_external_passes`
drains every registered pass, including its `ExternalFrame::bind_texture` calls, *before*
`encode_traced` runs and can still refuse the frame; a bind is applied to the engine's
`ExternalTextures` registry immediately, and `ExternalFrame::frame_index` has already advanced, so a
refused frame leaves the registry holding a bind whose recorded GPU work was discarded with the
dropped encoder — the pass's side effect and the frame it was recorded for are no longer the same
frame from the registry's point of view.

**Accepted because**: the engine's own shader-quad pre-pass shares this exact non-transactional
shape (it also binds into the external-texture registry ahead of a frame that can still be refused),
so this is an existing property of the seam rather than one `ExternalPass` introduces; a refused frame
is followed by a retry on the next vsync, which re-runs every registered pass and re-applies its
binds.

**Trigger for removal**: staged binds applied only after a confirmed encode, or a pass-visible
frame-outcome signal a pass can use to redo work a refusal discarded.

---

### `shell-ios-first-frame-above-goal` — the iOS split executor's first frame still lands above the 85 ms goal

**Observed**: overlapping the render thread's GPU bring-up with the UI thread's font-preinit join
(see [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)'s iOS render-thread split) narrowed but did
not close the gap: the iPhone SE's first frame lands 64-176 ms (median 100) on the published 2026-09-06
pass in `benchmarks/RESULTS.md`, against a 110 ms pre-overlap baseline and an 85 ms goal (an
earlier 12-launch probe on the same build read 89-115 ms against a 99 ms baseline). `TextContext::new`'s font preinit is bimodal — 51-65 ms on the fast path,
100-159 ms on the slow one, the first frame trailing the join by 13-23 ms — and once GPU bring-up runs concurrently underneath it, the slow mode is
what sets the median first-frame cost. A render-thread-style QoS boost applied to the font-preinit
thread was measured and did not close the gap either.

**Re-measured 2026-10-20** (`benchmarks/RESULTS.md`, iPhone SE section): with the device open fast (`adapter_ready` 12 ms) the
join is the whole residue — `frust_bench` first frame 108.5 ms median [75–377], join 94.5 [52–322], iOS 26.7.1's Core Text enumeration. Deferring the system fonts for apps that register fonts was
tried and not shipped: it brought huddle's first frame to 42 ms, but until adoption (≈ 200 ms)
text whose style ends in the system sans draws in the app's registered face with different metrics, then the screen swaps
font and re-lays out, and a widget-cached `TextLayout` keeps the window's face after adoption.

**Accepted because**: the render/font overlap already recovers most of the achievable win without
adding synchronization risk to the split; the remaining cost sits inside `TextContext::new`/Parley's
own font matching, which this shell does not own.

**Trigger for removal**: `TextContext::new`'s bimodal cost is made deterministic, or font preinit
moves off the first-frame critical path without changing the faces the first frame draws.

---

### `engine-atlas-pressure-fit-model-blind-to-glyphs` — the pressure-eviction fit model cannot see glyph pages, so a committed plan can still be refused

**Observed**: `ImageResidency::allocate_under_pressure`'s fit model (`plan_pressure_eviction`/
`layer_admits`, `crates/frust-engine/src/cache/images.rs`) walks only the image entries it tracks —
the glyph pages sharing the same allocator are invisible to it — so a plan the model approves can
still be refused by the real packer, costing that plan's bounded evictions with no allocation
(`skipped>0 evicted>0` in the `frust-perf img` line). The search is also bounded per atlas layer, so
a request that recurs across several layers pays that per-refusal search bound once per layer.

**Accepted because**: the model is deliberately optimistic rather than exact — it never refuses a
request eviction could have satisfied — and the cost of a wrong plan is bounded by the same
candidate/area ceiling that bounds every plan, not unbounded churn.

**Trigger for removal**: device data showing this cost is sustained rather than occasional, at which
point a fit test that also accounts for glyph pages, or an incrementally-maintained LRU, would be
warranted.

---

### `engine-atlas-pending-clear-reallocation-race` — a rectangle with a pending clear can be re-allocated to the glyph atlas before that clear is serviced

**Observed**: `ImageResidency::release` (`crates/frust-engine/src/cache/images.rs`) frees an entry's
rectangle back into the shared `ImageCache` immediately, but only *records* the rectangle in
`evictions`, which survives until a consumer acknowledges the plan (`acknowledge_plan`). Nothing
stops the glyph atlas policy — which allocates through that same shared allocator during compilation
— from being handed that still-not-cleared rectangle on a later frame, ahead of the pending clear
ever reaching a live atlas. Pre-existing (the allocator has always been shared), and now reachable
more often since eviction runs routinely under atlas pressure rather than only on the rare
age-based reap.

**Accepted because**: no observed corruption to date — the window needs a frame between the eviction
and the clear being serviced to land a glyph allocation on the exact freed rectangle — and closing it
needs an allocator-level hold list this crate does not have today.

**Trigger for removal**: an allocator-level hold list that withholds a freed rectangle from
reallocation until its pending clear is acknowledged.

---

### `corner-insets-ios-26-only` — `WindowInsets::corner_insets` reports zero on all platforms except iPadOS 26+

**Observed**: `WindowInsets::corner_insets` is non-zero only on iPadOS 26+ under the system window control. Android, desktop, web, and iOS < 26 always report zero (by construction: no other shell reads a corner region), so the Glyph and Material app bar shifts never fire there. Bars do not consume the horizontal safe-area insets; corner widths are measured from the safe-area edge, so a hypothetical control on a notched horizontal edge would under-shift (no platform draws one there). Additionally, a bar hosted in a detail pane, sheet, dialog, or below other content cannot detect its window-space origin and still receives the window-wide corner values, causing over-shift; `corner_shift(false)` is exposed as the author's opt-out. Several bars do not honour corner insets at all: the `frust_cupertino::navbar`, `frust_material::selection_app_bar`'s contextual face, and shadcn/beUI header components.

**Applies to**: Android, desktop, web, and iOS < 26; the `frust_glyph::app_bar` and `frust_material::{app_bar, search_app_bar, sliver_app_bar}` widget implementations; also `frust_cupertino::navbar`, `frust_material::selection_app_bar`, and shadcn/beUI headers.

**Why not fixed**: on every other platform zero is the correct value, since no system control occupies a corner there, and the notched-edge case has no producer. Detecting a bar's placement automatically would need a scoped context that split views and sheets clear, which is a design change.

**Watch item** (not reproduced on iOS 26.2): a developer-forum report that UIKit's corner layout guide does not reset to zero when a window returns to full screen. On the iPad Pro 13-inch (M5) iOS 26.2 Simulator the guide reset to zero on entering full screen and came back non-zero on return to a window; `.minimal` reports zero corners because the control takes a safe-area strip instead.

**Evidence**: device gate PASSED 2026-10-01 on the iPad Pro 13-inch (M5) iOS 26.2 Simulator (Xcode 27.0; 12 legs — windowed readout TL 66x43 / TR 10x43, bar slot clear, full screen zeros, rotation, Stage Manager resize, automatic/unified/minimal styles, RTL, no-regression).

---

### `engine-parallel-walk-power-unmeasured` — the parallel strip walk spawns helper threads per frame; its tails are measured, its power cost is not

**Observed** (evidence: `docs/PERFORMANCE_BASELINES.md` § Engine close-out per device): `frust-engine::compile::parallel` spawns up to two
scoped helper threads inside every frame whose planned shape work reaches
`DEFAULT_PARALLEL_MIN_COST` (8000; a spawn+join costs 127-135 us on the
Pixel 4a), with no persistent pool. The threshold and the three-thread default
were chosen from the S1 stress scene and the host timing report
(`parallel_walk.rs`'s ignored `timing_report`), not from a sweep. On both
phones the CPU-side tails improve in every S1 run (per-run `submit` p95 Pixel 4a
9.7-9.9 to 7.0-7.8 ms, Pixel 4 7.7 to 6.0-6.3 ms; `total` p95 per run Pixel 4a
17.0-17.3 to 13.4-13.9 ms, Pixel 4 8.6-8.7 to 6.8-7.3 ms) and S6 is unchanged
(below the threshold, no helper spawns). Two things the measurement does not
settle: (1) on the Pixel 4a the GPU governor dropped from 430 to 355 MHz once
the CPU stopped pacing the frame (`gpu_main` p50 12.76 to 13.89 ms, p95 14.2),
and in 4 of 10 default-arm runs the swapchain `acquire` p95 rose to 11.4-12.5
ms — a second display period, i.e. an occasional dropped frame the four
off-parallel runs never showed (pooled `total` p95 17.15 vs 17.23 ms); (2) no
battery, CPU-energy or thermal arm was run on either phone, so the cost of two
extra threads per frame on the little cores is unknown.

**Accepted because**: the frame is byte-identical at every worker count (the
goldens run with `FRUST_ENGINE_PARALLEL_MIN_COST=0` and `FRUST_ENGINE_NO_PARALLEL=1`
alike), the median and the CPU-side tails improve on both phones, and
`FRUST_ENGINE_NO_PARALLEL` is a one-variable revert documented in
`RENDER_DEVELOPMENT.md`.

**Trigger for removal**: a power/thermal A/B of the on and off arms (the thread
policy measured neutral and the ADPF session ships opt-in, `android-adpf-opt-in-measured`, so
neither settled the governor interaction), or a parked persistent helper that removes the
per-frame spawn, whichever lands first.

---

### `engine-strip-curve-flattening-accuracy` — curved strip edges are accurate to the rasteriser's 0.25 px flattening

**Observed** (evidence: an analytic distance-field comparison on a Pixel 4a, S1, 2026-10-14): the strip rasteriser flattens
curves at 0.25 px, so on small circles a strip edge sits 33-70 levels (of 255) off the exact
shape's coverage, where an analytic distance field lands within 0.9-5.3. The CPU oracle flattens at
the same tolerance, so the goldens agree with the strips, not with the exact shape.

**Accepted because**: the error is confined to the anti-aliased edge pixels of small curves, it is
the same accuracy the reference renderer ships, and tightening it costs flattening work on every
curve.

**Trigger for removal**: a flattening tolerance change or an analytic edge path that ships.

---

### `android-pipeline-cache-persist-gaps` — the moments Android's split path persists the pipeline cache leave four known gaps

**Observed** (evidence: `crates/frust-shell-android/src/jni_glue.rs`'s `CachePersistence` and
`persist_pipeline_cache_if_changed`): the split render thread persists the Vulkan blob once after
the surface's warm-up is done (checked after each frame), on `SurfaceDestroyed` and on a stray
`Pause`. (1) A static first screen killed in the foreground before any later frame or surface
destroy leaves no blob, so the next launch is cold again. (2) The inline `FRUST_NO_RENDER_THREAD`
path and the desktop shell persist once, right after the surface install, when the warm-up has
barely started (the install-time write, measured on the split path before it moved, held 328 B and
zero pipelines on the Pixel 4a, against about 117 KB and 18 pipelines after the warm-up). (3) Writes are ordered by a per-process generation, so
a second `FrustSurfaceView` can land a smaller blob over a fuller one. (4) A writer thread the OS
refuses is retried on the next frame with no bound, so sustained refusal costs one attempt and one
warning per frame.

**Accepted because**: each gap costs a later launch its cold compile, never a wrong pixel, and the
common launch-use-background cycle persists the full list.

**Trigger for removal**: a persist on the first idle frame, the inline and desktop paths moved to
the after-warm-up moment, per-surface ordering, or a bounded retry.

---

### `android-adpf-opt-in-measured` — the opt-in Android performance-hint session covers one render thread per process

**Observed**: the ADPF session (`FRUST_ADPF=1`, default off, API 33+) is one session per process,
over the UI thread and one render thread, so a second `FrustSurfaceView`'s render thread is never in
a session.

**Applies to**: Android apps hosting more than one `FrustSurfaceView` with the opt-in set.

**Why accepted**: the shell drives one `FrustSurfaceView` per process today, and the session is
opt-in because no captured scene improved with hints on.

**Trigger for removal**: a per-surface session, or removing the opt-in.

**Evidence**: `crates/frust-shell-android/src/adpf.rs` module doc (*Shape*);
`docs/PERFORMANCE_BASELINES.md` § Engine close-out per device.

---

### `android-frame-sync-latch-per-process` — the per-frame JNI sync keeps one latch per process, and the soft-keyboard typing check has not run with gated IME polls

**Observed** (evidence: `crates/frust-shell-android/src/ffi_support.rs`'s `FrameSyncLatch`;
`docs/PERFORMANCE_BASELINES.md` § Engine close-out per device): `nativeFrameSync` cut the per-frame
JNI poll cost on an idle page from 166 to 36 us (Pixel 4a) and 140 to 30 us (OnePlus 9) by telling
Kotlin which polls have anything to report. Its latch is process-wide and remembers one handle: a
sync from a second `FrustSurfaceView` resets it, so two views alternating report every bit each
frame (the gate stops saving anything), and a clipboard value one view drained but had not yet
taken is dropped by that reset. A manual soft-keyboard typing pass (IME composition, caret moves
and field switches arriving only through the gated IME poll) has not been run on a device since the
polls were gated.

**Accepted because**: the shell drives one `FrustSurfaceView` per process today, the IME bit is
also raised on every IME-side input and on any published-state change, and `FRUST_NO_FRAME_SYNC=1`
restores the ungated polls.

**Trigger for removal**: a per-handle latch, and a recorded soft-keyboard typing pass with the gate
on.

---

### `cli-build-no-default-features-gap` — `frust build` forwards cargo `--features` only, never `--no-default-features`

**Observed**: `frust build`'s cargo passthrough (`crates/frust-cli/src/cli.rs`,
`crates/frust-cli/src/commands/build.rs`) carries `--features` only; there is no
`--no-default-features` passthrough. A no-db size measurement therefore cannot go through
`frust build apk` at all — the `frust_bench` no-db-feature APK used for that measurement is built
directly with `cargo-ndk` plus `./gradlew` instead — and `frust build apk --release` separately
refuses to run without real release-signing material
(`crates/frust-drive/src/android_build/signing.rs`) by design, so that measurement APK is a
debug-signed artifact outside the CLI's own release path.

**Accepted because**: the CLI's release-signing refusal is a deliberate guard against shipping an
unsigned release artifact; the missing `--no-default-features` passthrough is a real gap but has had
exactly one caller — this size measurement — to date.

**Trigger for removal**: a `--no-default-features` passthrough (or a dedicated measurement mode) on
`frust build`.

---

### `web-paced-30hz-straddle` — a nominal 30 Hz paced loop actually paces at 20-30 Hz on a 60 Hz display

**Observed** (evidence: `crates/frust-shell-web/src/pacing.rs`; measured in
headed Chrome 151 on the project's Linux GPU rig): a paced request naming a
30 Hz cadence resolves an interval that lands a hair above two 60 Hz refresh
periods, so the achieved cadence alternates between 33.9 ms and 50 ms
frame-to-frame rather than holding a steady ~33.3 ms — a rounding artefact of
composing two browser wake mechanisms (`ControlFlow::WaitUntil` plus
`requestAnimationFrame`), not a lost- or torn-frame defect. See
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md) for the pacing contract this
composes with.

**Accepted because**: every paced frame still lands on a real display
refresh; the straddle is `raf_quantized` rounding a deadline up to the next
rAF tick rather than to the exact requested interval.

**Trigger for removal**: a pacing computation that rounds to the nearest
rather than the next-above refresh boundary, if a future measurement shows
the straddle is visible to users.

---

### `web-a11y-devtools` — the web shell has no accessibility tree and no devtools loopback

**Observed** (evidence: `crates/frust-shell-web/src/app_handler.rs`'s
`push_semantics`/`pump_devtools`): AccessKit ships no web adapter, so the
semantics pass publishes nothing — a canvas app needs its whole tree
mirrored into real DOM/ARIA elements to be readable by a screen reader at
all, and no browser AccessKit backend exists to do that (a canvas-wide gap
across the industry — every canvas-rendered web framework hits the same
wall, not a frust-specific omission); and the in-app devtools service is a
loopback TCP listener a `wasm32` build has no sockets for, so
`frust-shell-web` forwards no `devtools` cargo feature at all
(`crates/frust/Cargo.toml`'s `devtools` feature list omits it).

**Accepted because**: each gap is a documented no-op with a stated call
site for the eventual real implementation, not a silent absence — see
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md).

**Trigger for removal**: a DOM/ARIA semantics mirror, and a WebSocket-based
devtools transport, each its own future change.

---

### `web-ime-residual-gaps` — the web IME bridge ships, but gaps remain

**Observed** (evidence: `crates/frust-shell-web/src/ime.rs`'s module doc, "Cases deliberately not
handled" and "A cut may not delete text it never wrote out";
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)'s Web IME bridge): the hidden-`<input>` overlay
bridge turns real browser composition (CJK marked text, dead keys, mobile autocorrect) into
`ImeEvent::Compose`/`Commit` and carries the DOM `copy`/`cut`/`paste` route. Four gaps remain:

1. **A soft keyboard's Backspace is lost.** It arrives unnamed like the letters, but the overlay
   element is emptied on every frame no composition owns, so the deletion finds nothing to delete
   and the browser raises no `input` for it. A hardware Backspace, and every deletion inside a
   composition, still work.
2. **The mobile visual-viewport jump is not followed.** The overlay is placed in layout-viewport
   coordinates, so when the browser shifts the visual viewport for its soft keyboard, the overlay
   drifts from the focused field until the next reposition.
3. **A `Password` field may get no composition.** A `type="password"` element may receive no input
   method at all (many browsers and keyboards switch secure entry to a plain layout), so a secret
   field takes text through the key path and the `input` signal only. A hint that moves under a
   focused field also replaces the element from the frame loop, outside a gesture, so a mobile soft
   keyboard may close until the next tap.
4. **One cut can destroy text it wrote nowhere.** An engine that raises no `cut` for the overlay's
   collapsed selection (one with no `beforecut` to claim the verb with) *and* withholds
   `navigator.clipboard` (a page that is not a secure context) leaves no route after the widget has
   already deleted.

**Accepted because**: real CJK/IME composition has not been driven through a live browser, so the
composition, commit and cancel contract rests on the bridge's own tests rather than a live run;
none of these gaps blocks ordinary text entry. (1) blocks only deletion from a soft keyboard. (4) needs three
conditions at once, and a page served over plain `http` has already lost the async clipboard for
every other purpose.

**Trigger for removal**: a sentinel character kept in the element for a deletion to consume (1),
`visualViewport` offsets threaded through the shell's coordinate conversion (2), and a delete
deferred until its write is confirmed (4).

---

### `web-generic-family-partial-fallback` — `Monospace`, `Serif` and `Emoji` still resolve no glyph on `wasm32`; `SystemUi`/`SansSerif` are covered by a bundled fallback face

**Observed** (evidence: `crates/frust-shell-web/src/fonts.rs`;
`crates/frust-text/src/context.rs`'s `register_generic_fallback`, re-exported
from `lib.rs`): a browser exposes no font files and no
installed-family list, so the web oracle backend (`oracle/web.rs`) never answers a
question and a generic `FontFamily` slot resolves no glyph on the web tier by
default; only app-registered faces and generic fallbacks answer.
`frust-text` now exposes `register_generic_fallback`, applied by every
`TextContext` through `sync_app_fonts`; `frust-shell-web`'s
`install_default_fonts` calls it once at start-up with a bundled Inter
Variable face (SIL Open Font License 1.1, `crates/frust-shell-web/fonts/`),
mapped to `GenericSlot::SystemUi` and `GenericSlot::SansSerif` only.
`Monospace`, `Serif` and `Emoji` remain unmapped on `wasm32` by design: a
proportional face substituted for `Monospace` would silently regress
`TextInput`/code-display layout, and the crate ships neither a serif nor an
emoji face. An app's own named-family registration still wins over the
fallback, since a named lookup always resolves before a generic one.

**Applies to**: any `frust-shell-web` app relying on the `Monospace`,
`Serif`, or `Emoji` generic family without registering its own face for
it — that text still resolves no glyphs. Every `frust-shell-web` wasm
binary also carries the bundled face's 879,708 bytes (~860 KB) via
`include_bytes!` with no opt-out today, whether or not an app ever uses the
fallback.

**Accepted because**: `SystemUi`/`SansSerif` — the default and by far the
most common generic request — are now fixed at the framework level rather
than left to a per-app workaround; extending the same mechanism to
`Monospace`/`Serif`/`Emoji` needs a bundled face for each (a further
per-binary size cost) or a page-side font-loading seam, neither of which
exists yet.

**Trigger for removal**: a bundled monospace/serif/emoji policy, or a
page-side font-loading seam that lets an app supply those faces without
paying for them in every binary.

---

### `web-no-plugins-native-widgets-platform-views-v1` — no OS-capability plugin, native-widgets control, or platform view works on the browser tier in v1

**Observed** (evidence: `plugins/clipboard/src/lib.rs`'s per-target
`set_text`/`get_text` arms, whose total-cover
`#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos",
target_os = "linux", target_os = "windows")))]` fallback — the one `wasm32`
actually compiles under — returns
`ClipboardError::NotAvailable(Unavailability::UnsupportedPlatform)` rather
than doing anything; `plugins/native-widgets/src/lib.rs`'s
`#[cfg(target_os = "android")]`/`#[cfg(target_os = "ios")]`-only modules,
with no third arm for any other target; `crates/frust-shell-web/src/`
naming no `platform_view` module at all, unlike every concrete shell in
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)): the OS-capability plugin
tier (shared-preferences, secure-storage, camera, clipboard, haptics, iap,
database, i18n), the native-widgets control plugin, and platform-view
embedding are each mobile/desktop-only today. A browser app either gets a
typed "unsupported" answer where a plugin bothers to report one (clipboard —
see `web-clipboard-unavailable-v1` below), or simply cannot depend on the
crate meaningfully at all (native-widgets has no non-mobile arm to compile).
(2026-09-08)

**Accepted because**: v1's browser tier scope is rendering, input, theme and
resize (see [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)'s Web tier); a
web backend for a specific plugin — clipboard's secure-context Clipboard
API, a File-System-Access-backed storage plugin, a DOM-based control set
standing in for native-widgets — is each its own future scope, not a
same-shape port of the mobile/desktop implementation.

**Trigger for removal**: tracked per plugin as each grows a real web
backend, rather than closed as one blanket entry.

---

### `web-clipboard-unavailable-v1` — `frust-clipboard` has no web backend; every call reports `UnsupportedPlatform`

**Observed** (evidence: `plugins/clipboard/src/lib.rs`'s `set_text`/
`get_text`/`set_text_sensitive`, each `#[cfg]`-arm-selected per target with a
`wasm32`-covering total-cover fallback returning
`Err(ClipboardError::NotAvailable(Unavailability::UnsupportedPlatform))`):
unlike Android/iOS/macOS/Linux/Windows, which each resolve to a real backend
(`ClipboardManager`/`UIPasteboard`/`arboard`), the crate compiles cleanly for
`wasm32-unknown-unknown` but every operation is a typed refusal. The
browser's own clipboard surface (the async Clipboard API,
`navigator.clipboard.readText`/`writeText`) is a **secure-context** API
gated behind a user gesture and, for reads, a permission prompt — a
materially different contract from every other platform's synchronous
plugin call, which this crate has not yet been reshaped to accommodate.

**The framework's own text fields are not affected.** Copy, cut and paste
inside a `frust::TextInputView` on the browser tier do not go through
this plugin at all: they ride the web shell's DOM route on the same
hidden `<input>` overlay [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)'s
Web IME bridge describes — a `paste` event carries its own text, and
`copy`/`cut` write through `clipboardData.setData` inside the gesture,
with `navigator.clipboard` only as the fallback. What this entry
describes is the *plugin* API an app calls directly. (2026-09-12)

**Accepted because**: a typed `NotAvailable` refusal is the same shape the
crate already uses for any unsupported platform (see `set_text`'s "Total-cover
fallback" comment) — a caller already has to handle it, and no app-visible
crash or silent no-op results.

**Trigger for removal**: a `wasm32` backend built on the async Clipboard API,
which will also need `ClipboardError`'s synchronous return type reshaped to
carry the API's own asynchrony and permission-prompt outcomes. Note what it
would *not* close: the shell's DOM route already serves the framework's own
fields, so this is about an app calling `frust_clipboard` directly, and any
such backend inherits the gates `web-toolbar-paste-permission-gated`
records.

---

### `web-toolbar-paste-permission-gated` — a toolbar paste on the web is permission-gated and may silently drop

**Observed** (evidence: `crates/frust-shell-web/src/app_handler.rs`'s
`read_clipboard_text` and `navigator_clipboard`): a paste the *tree* asks
for — the selection toolbar's Paste item, or any `EventCtx::request_paste`
— cannot ride the DOM `paste` event, which only a keystroke raises. The
shell answers it with `navigator.clipboard.readText()` instead, and that
call is permission-gated where the keyboard's own paste is not: Chrome on
Android prompts, iOS Safari shows its Paste callout, and Firefox refuses a
read it cannot tie to a gesture. A refusal is one dropped paste, logged and
never retried. On a page that is not a secure context (`http://` other than
`localhost`) `navigator.clipboard` is undefined outright, so a toolbar
paste and an app-driven copy no-op for the whole run — a keyboard paste is
unaffected there, since the `paste` event carries its own text.
(2026-09-12)

**Accepted because**: the async Clipboard API is the only route to a read no
DOM event will deliver, and its permission model is the browser's rather
than ours. The failure is a dropped paste with a console warning, never a
partial or wrong insertion, and the keyboard route the same field offers is
not gated at all.

**Trigger for removal**: nothing in frust closes it — the gate belongs to
the browser. What is owed is observation of how each engine actually
behaves: the browser gate so far has run on Safari only and only partially,
the Chrome and Firefox legs are unconfirmed, and Firefox is not installed on
the development machine.

---

### `video-mkv-webm-apple-unsupported` — Matroska/WebM plays on Android and not on Apple

**Observed**: `frust-video-player` forwards a source to the platform player
and reports what that player answers, so container/codec coverage is the
platform's, not the plugin's. Media3's ExoPlayer extractor set covers
Matroska/WebM (and the VP8/VP9 codecs usually inside them); AVFoundation's
does not, so the same `.mkv`/`.webm` source that plays on Android publishes
`VideoError::Decoder` on iOS and macOS. HLS runs the other way at the
artifact level: it is native on Apple and needs Android's separately
declared `media3-exoplayer-hls` artifact on the runtime classpath.

**Applies to**: iOS and macOS. The asymmetry is AVFoundation's own supported
format set, not something this plugin gates.

**Why accepted**: the alternative is bundling a software decoder (a very
large dependency, a battery cost, and no hardware path) or silently
transcoding — neither is a plugin-tier decision. MP4/H.264 and HLS are the
intersection that plays everywhere; an app targeting both platforms should
ship that and treat a `Decoder` error as a per-platform answer.

**Evidence**: platform format-matrix survey (Media3's extractor set vs
AVFoundation's); the crate's own `Backends`/format-asymmetry note. Not yet
reproduced on hardware — the device gate for this plugin is still owed. See
also `video-web-windows-linux-unavailable-v1` below.

---

### `video-web-windows-linux-unavailable-v1` — no video backend on the web, Windows, or Linux

**Observed**: `VideoPlayer::open` reports `VideoError::NotSupported` on every
target that is not Android, iOS, or macOS — Windows and Linux desktop and
`wasm32` included. It fails soft (never a panic), so an app can hide the
control and degrade rather than crash, and `VIEW_TYPE` is the empty string
there, reserving no platform-view slot.

**Applies to**: Windows, Linux, and the browser tier. Not uniform in nature:
Windows and Linux have real system players a future backend could reach
(Media Foundation, GStreamer), so those are **deferrals** on mobile-first v1
scope, the shape `iap-desktop-unavailable-v1` records. The browser is a
different gap entirely — the whole platform-view mechanism is absent there
(`web-no-plugins-native-widgets-platform-views-v1`), so a web backend would
need a `<video>` element composited against the canvas, not just a session
backend. Texture mode has producers on Android, iOS, and macOS only, so
Windows, Linux, and the browser have none either.

**Why accepted**: mobile-first v1 scope plus one real desktop host (macOS)
to prove the desktop platform-view path. The unsupported arm is a
dependency-free module, not a stub around an unfinished backend, so a
Linux/Windows preview build runs its real code path and simply gets a typed
refusal.

**Evidence**: the crate's target-gated backend selection and its
`unsupported` module; `cargo check` on each target. No device run is owed
here — there is nothing to run.

---

### `video-no-auto-pause-in-background` — playback keeps running when the app backgrounds

**Observed**: sending an app to the background does not pause a video
session on any platform. Audio keeps playing; on returning, playback has
advanced.

**Applies to**: Android, iOS, and macOS alike. Neither host registers a
lifecycle observer of its own — Android's host explicitly registers no
`ActivityLifecycleCallbacks`, and the Apple backend hooks no
`UIApplication`/`NSApplication` notification.

**Why accepted**: audio-only continuation is a legitimate app choice (a
podcast-style player wants exactly this), and the plugin has no way to tell
that intent from an app that wants a hard pause. The API an app needs is
already there and non-blocking: call `pause`/`play` from your own lifecycle
handling. Whether the default should flip — and whether a background-audio
mode belongs in `PlayerOptions` — is an open ruling;
until it is decided the behaviour stays
uniform across the three platforms rather than differing per host.

**Evidence**: the Android host's own contract note and the absence of any
lifecycle observer in either backend. Not yet exercised on hardware.

---

### `desktop-platform-view-mode-a-only` — a desktop hosted view is opaque, captures its own input, and has no Mode B

**Observed**: on macOS a platform-view slot hosts a native `NSView` above
the window's content view. Anything frust paints *under* that slot is
covered rather than blended, so chrome stacked over a hosted view (a
transport bar over a video, say) is invisible; and every pointer, wheel and
scroll event inside the view's bounds is consumed by the native view, so a
frust scroll view underneath it never sees them — scrolling with the pointer
over a video does not scroll the page.

**Applies to**: every desktop host. macOS is the only one with an
implementation at all; Windows and Linux have no platform-view host, so a
slot there hosts nothing. On macOS, frust paint above a hosted view is possible
behind `FRUST_OVERLAYS` (default off): an overlay `CAMetalLayer` view sits above
its slot (both paint orders verified on macOS), while input stays with
the native view. Does not apply in texture mode, where frames are scene content frust draws and layers freely.

**Why accepted**: Mode A (composite an opaque native sibling above the
surface) is the whole desktop v1 contract — no translucent window, no
punched hole, no input forwarding. Mode B on the desktop would need a
translucent swapchain plus per-host z-order and hit-test plumbing, which is
a future plan rather than a missing line: the shield-rect channel the differ
already carries is passed empty here, and that is the single place input
forwarding would attach. With the macOS overlay knob off the rule for callers
is the one the plugin READMEs state — put controls **beside** the picture,
never on top of it (`docs/CODE_STANDARDS.md`'s Platform-View Conventions).

**Evidence**: the macOS host's own z-order strategy (`addSubview:positioned:
relativeTo:` above winit's content view, nothing made translucent anywhere)
and the desktop host passing no shield rects. Runtime confirmation on a Mac
is owed with the video-player device gate. macOS native-widgets demo gate
(2026-10-01, macOS 27.0 on an Apple M4, debug build 2b3ae58b, human-observed):
confirmed as documented — with the pointer over a native control the page does
not scroll, and a frust overlay painted over a native control is covered.

---

### `desktop-platform-view-frame-lead` — a desktop hosted view can lead its frust surroundings by one frame

**Observed**: the desktop shell applies a platform-view command batch on the
UI thread immediately after the scene it describes is submitted, i.e. before
that scene is presented. A hosted view therefore reaches its new geometry up
to one display frame ahead of the frust content it is pinned to — visible as
a hosted view leading its surroundings while a scroll or resize animates,
and invisible while it is static.

**Applies to**: macOS (the only desktop host with a platform-view
implementation). Both mobile shells are unaffected: they gate their release
on a presented frame id through `FramePairing`. Does not apply in texture mode: texture frames are scene content and have no platform-view lead.

**Why accepted**: the lead is bounded at ≤1 frame by construction — the
batch always describes the very scene being submitted, never an older one —
and closing it is not a local change. `FramePairing` needs the id of the
frame the render side actually *presented*, and the desktop frame executor
publishes only a presented-frame **count**, with no id travelling with a
submission; pairing would mean threading a new id channel through the
render split, which is a larger seam than this one.

**Evidence**: the desktop host's own timing contract and the executor's
presented-frame counter. Not yet observed on a running Mac — the desktop
half of the video-player device gate is owed, and that is where it would
first become visible.

---

### `tui-physical-ios-app-death-undetected` — a physical iOS device session can outlive the app it is watching

**Observed**: `frust-tui` detects neither the app being stopped from the workbench nor the app dying
on the device for a physical-iOS session. Android gets a liveness prober (`spawn_liveness_prober`)
that asks `adb ... pidof` every `LIVENESS_PROBE_INTERVAL`, closing the session with `APP_GONE_NOTE`
after `LIVENESS_STRIKES` consecutive misses; the iOS Simulator needs no prober because its
`simctl launch --console-pty` bridge is a child of the app and its console pipe exits with it. A
physical iOS device has neither: `TerminationTarget` has no variant for it (`devicectl` exposes no
app-termination call this crate uses), so a workbench-initiated stop is stream-only, and no prober
exists for it because `devicectl`'s console behavior on app death is unverified on real hardware — a
physical-iOS session can therefore linger as `Running` after the app is gone until the user closes
its tab by hand.

**Applies to**: `frust-tui`'s `Supervisor` for a device session whose `TerminationTarget` is a
physical iOS device (the Simulator and Android targets are both covered as described above).

**Why accepted**: a prober needs a signal to poll, and `devicectl`'s behavior on app death has not
been measured on real hardware — building one on a guess risks a false "gone" on a device that is
still running fine. This is the same least-verified-target gap `devtools-ios-physical-forward-
deferred` and `tui-device-stop-app-termination-residual` already record for physical iOS. This entry
would be removed once a verified `devicectl` console-exit behavior (or a real-hardware process-list
probe) lands and a prober or termination call can be built on it.

**Evidence**: `crates/frust-tui/src/supervise/supervisor.rs`'s module doc ("Detecting the app dying
(Android liveness prober)" and the physical-iOS paragraph that follows it), `spawn_liveness_prober`/
`probe_liveness`/`TerminationTarget`.

---

### `tui-clipboard-osc52-terminal-support` — an SSH/headless copy depends on the terminal's own OSC 52 support

**Observed**: the workbench picks its clipboard backend once at startup (`clipboard::detect`): the
OS clipboard (`arboard`) when a display server is reachable, an OSC 52 escape sequence over SSH or on
a headless tty, and disabled (with a startup warning) when neither applies; `FRUST_TUI_CLIPBOARD=
system|osc52|off` overrides the detection. Over SSH the copy therefore relies entirely on the
terminal emulator relaying OSC 52 — tmux needs `set -g set-clipboard on`, iTerm2 needs Preferences →
General → Selection → "Applications in terminal may access clipboard", and GNU screen only accepts
the sequence DCS-wrapped (`write_osc52`'s `screen` chunking). A terminal that does not implement OSC
52 at all drops the copy silently — there is no error path back to the workbench, since the escape
sequence is fire-and-forget to stdout.

**Applies to**: every `y`/"Copy selection", `v`→`y` line-selection copy, and context-menu "Copy line"
/"Copy artifact path(s)" action running in `ClipboardMode::Osc52` (SSH sessions and headless ttys
under `ClipboardMode::Auto`).

**Why accepted**: OSC 52 is the only clipboard channel that can reach a local machine from inside a
remote SSH session at all — there is no local display server to hand `arboard` — so the alternative
to "silently ignored by an unsupporting terminal" is no remote-copy capability whatsoever. No removal
is planned: this is a property of the terminal the user chose, not something `frust-tui` can detect
or work around from inside the pty.

**Evidence**: `crates/frust-tui/src/clipboard.rs`'s module doc, `ClipboardMode`/`detect`/`write`/
`write_osc52`.

---

### `lean-android-hashbrown-split` — the lean Android release graph carries two `hashbrown` copies

**Observed**: `hashbrown` resolves to two versions on the lean Android release graph —
0.16.1 and 0.17.1. `parley_engine`, `fontique` and the registry `glifo`
(exact-pinned in the root `Cargo.toml`) already sit on 0.17.1; the 0.16.1 copy is held down by
`accesskit_consumer 0.38.0`, `gpu-allocator 0.28.0`, and `tree_arena 0.2.0`. `Cargo.lock` holds a
third version, 0.15.5, but only `object` (reached through the host-side `frust-drive` tooling) uses
it, so it is not on the Android graph.

**Applies to**: every Android release build (lean or full feature set) — the split is on the
resolved dependency graph, not behind a feature flag.

**Why accepted**: none of the three crates holding the graph at 0.16.1 is a pin this
workspace owns under [DEVELOPMENT.md](DEVELOPMENT.md)'s Version-Pin Policy, so closing the
split means bumping `accesskit`/`accesskit_consumer`, `gpu-allocator`, or `tree_arena` on
their own schedule rather than frust-text's. Measured cost of the second copy is negligible
under this workspace's `lto = "fat"`/`codegen-units = 1` release profile plus the Android
`--icf=all` link flag (`.cargo/config.toml`): two near-identical hashing implementations fold
heavily under fat LTO's dead-code elimination and identical-code folding, leaving the second
`hashbrown` monomorphization set well under the noise floor of a full release build.

**Trigger for removal**: `accesskit_consumer`, `gpu-allocator`, or `tree_arena` moving to a
`hashbrown 0.17` requirement.

**Evidence**: `Cargo.lock` (the `hashbrown` entries and their reverse dependencies); the lean Android graph in `benchmarks/frust_bench`.

---

### `android-legacy-gradle-cache-recreated` — `android/.gradle` can still be recreated by a Gradle run Frust did not launch

**Observed**: `frust build apk`/`frust run` always pass `--project-cache-dir <project>/build/android/.gradle` to `./gradlew`, so their own invocations never write Gradle's per-project cache under `android/`. A hand-run `./gradlew` from the `android/` directory — Android Studio's own build, or a developer invoking Gradle directly — gets Gradle's own default cache location instead and recreates `android/.gradle`.

**Applies to**: any project opened in Android Studio, or any manual `./gradlew` invocation from `android/`.

**Why accepted**: Frust cannot intercept a build it did not launch; `--project-cache-dir` only affects an invocation Frust itself constructs. `android/.gradle` is one of `build_dirs::LEGACY_CLEAN_DIRS`, so `frust clean` removes it wherever it reappears — the fix is cleanup, not prevention.

**Evidence**: `crates/frust-drive/src/build_dirs.rs` (`LEGACY_CLEAN_DIRS`); `crates/frust-drive/src/android_run/gradle.rs` and `android_build/mod.rs` (`--project-cache-dir`).

---

### `url-launcher-ios-open-failure-unobservable` — an iOS launch failure has nothing left to report back to

**Observed**: `UrlLauncher::open_external` on iOS dispatches the lookup-and-open sequence onto `dispatch_get_main_queue()` asynchronously (`DispatchQueue::exec_async`) and returns `Ok(())` immediately, before that closure has run. The closure's own call, `UIApplication::openURL_options_completionHandler`, passes `None` for the completion handler, so even a successful dispatch reports nothing back into Rust once it runs. A "no handler for this URL" or "app suspended" failure on this path is therefore unobservable to the caller — `open_external` always returns `Ok(())` on iOS regardless of what actually happens.

**Applies to**: iOS only; every call to `UrlLauncher::open_external` on that platform.

**Why accepted**: a synchronous main-thread bounce (blocking until the dispatched closure completes) would violate this crate's fire-and-forget, never-block-the-caller contract if `open_external` is ever called from a background thread — `exec_async` is the only shape that holds that contract regardless of caller thread, and there is no result channel back into an already-returned stack frame from a `dispatch_get_main_queue()` closure.

**Trigger for removal**: none anticipated without a callback-based `open_external` API (the crate currently returns before dispatch completes) — a redesign of the crate's synchronous shape.

**Evidence**: `plugins/url-launcher/src/apple.rs`'s module doc (*Main-thread dispatch*, *`unsafe`*) and its `open_on_main`/`open_external` functions.

---

### `url-launcher-desktop-no-deep-link-return` — a desktop browser launched by `frust-url-launcher` cannot hand a result back

**Observed**: `desktop::open_external` spawns `open`/`xdg-open` (macOS/Linux) or calls `ShellExecuteW` (Windows) and returns as soon as the OS accepts the launch; none of the three has any way for the resulting browser tab to hand a result back to the launching process. `frust-url-launcher` itself offers no return channel.

**Applies to**: macOS, Linux, and Windows alike; an app that uses this plugin alone to start a browser-mediated round trip (for example an OAuth authorization request) on desktop. A desktop OAuth round trip is covered by `frust-auth-session`'s loopback listener (`LoopbackSession`) with the protocol half in `frust-oauth-native`; use those instead of this plugin for that case.

**Workaround**: use `frust-auth-session`'s loopback session for a redirect-based flow; for anything else, a manually pasted code.

**Why accepted**: this plugin's charter is opening a URL, nothing more. Mobile's equivalent return leg goes through `frust::deep_links()`, which has no desktop analogue.

**Trigger for removal**: this plugin grows a completion path of its own, or a desktop deep-link channel ships in the framework.

**Evidence**: `plugins/url-launcher/src/desktop.rs`'s module doc; `plugins/url-launcher/README.md` §3 (*Desktop has no deep-link callback*); `plugins/auth-session/src/loopback.rs` (`LoopbackSession`).

---

### `url-launcher-desktop-no-association-unobservable` — a macOS/Linux opener that runs but finds no handler reports nothing back

**Observed**: `UrlLauncher::open_external` on macOS/Linux spawns `open`/`xdg-open` and returns as soon as the spawn itself succeeds. The only failure it can classify is spawn-time: `io::ErrorKind::NotFound` (the opener **binary** is absent from `PATH`) maps to `NoHandler`, other spawn errors to `Platform`. The opener's own verdict — `xdg-open` exits nonzero, e.g. `EXIT_FAILURE_OPERATION_IMPOSSIBLE`, when nothing is registered for the URL — arrives only as an exit status on the detached reaper thread, strictly after `open_external` has already returned `Ok(())`. A host with `xdg-open` present but no registered `http`/`https` handler therefore receives `Ok(())` while nothing opened.

**Applies to**: macOS and Linux only. Windows is unaffected (`ShellExecuteW` is synchronous and its return code encodes `SE_ERR_NOASSOC`/`SE_ERR_ASSOCINCOMPLETE`); Android is unaffected (`ActivityNotFoundException` is caught and mapped to `NoHandler`).

**Why accepted**: reading that exit status means waiting for the opener to exit, and `open_external` must not block its caller — the crate's fire-and-forget contract. There is no channel back into a call that has already returned; adding one (callback, channel, polled flag) would change the crate's synchronous shape for one platform alone. `UrlLauncherError::NoHandler`'s doc comment is narrowed to state exactly what each platform detects rather than promising a detection this backend cannot deliver. Directly parallel to `url-launcher-ios-open-failure-unobservable`.

**Related, same code path**: the reaper is a plain `std::thread::spawn`, which panics if the OS cannot create a thread. Under extreme thread exhaustion `open_external` can therefore panic where the pre-fix spawn-and-drop code could not. Judged acceptable (a desktop URL-open path is not a plausible thread-exhaustion site) and recorded rather than hidden; `std::thread::Builder::spawn` would degrade instead of panicking if this ever matters.

**Trigger for removal**: the crate grows an asynchronous completion shape (a callback or awaitable outcome) that can carry a post-return verdict to the caller — for example if the sibling `auth-session` work introduces one that this crate can share.

**Evidence**: `plugins/url-launcher/src/desktop.rs` (the `#[cfg(not(target_os = "windows"))]` arm and its module doc's *Why a reaped exit status can't sharpen `NoHandler` here* section); `plugins/url-launcher/src/lib.rs`'s `NoHandler` doc comment.

---

### `share-desktop-clipboard-fallback` — desktop has no text share sheet, so `frust-share` copies text and URLs to the clipboard

**Observed**: `Share::text`/`url` on macOS, Linux, and Windows copy the payload through `frust-clipboard` and return `ShareOutcome::CopiedToClipboard`; no sheet appears. Files are separate: macOS opens an `NSSharingServicePicker`, Linux and Windows are `share-files-linux-windows-unsupported`.

**Applies to**: macOS, Linux, and Windows; `Share::text`, `text_anchored` and `url`.

**Why accepted**: no desktop OS exposes a uniform share sheet worth a per-platform backend; a clipboard copy keeps the API usable in desktop previews, and the outcome variant tells the caller which path ran.

**Trigger for removal**: a desktop text share-sheet backend (e.g. a macOS `NSSharingServicePicker` for text) ships.

**Evidence**: `plugins/share/src/desktop.rs`; `plugins/share/README.md`.

---

### `share-ipad-popover-anchor-optional` — the iPad popover anchor is optional and the playground passes a fixed rect

**Observed**: on iPad the activity sheet is a popover; `Share::text_anchored` takes a `ShareAnchor` rect, and without one the popover is centred with `permittedArrowDirections = 0` (no arrow). The widget tier has no laid-out-rect seam, so the playground's URL page passes a fixed, known-position rect (`SHARE_ANCHOR_RECT`) that drifts if the page layout changes. Unverified on iPad until the share device gate runs.

**Applies to**: iPad only; apps whose share button position is not known in window coordinates.

**Why accepted**: the popover needs a source rect and the framework does not yet surface a widget's laid-out rect to app code; centred-without-arrow is a safe default that never raises.

**Trigger for removal**: the widget tier exposes a laid-out-rect seam the playground (and apps) can read.

**Evidence**: `plugins/share/src/apple.rs` (*iPad popover*); `examples/playground/src/pages/url_launcher.rs` (`SHARE_ANCHOR_RECT`).

---

### `url-launcher-native-maps-desktop-unsupported` — native map schemes have no desktop handler

**Observed**: `UrlLauncher::open_maps_native` (allowlisted `geo`/`maps`/`comgooglemaps`/`google.navigation`) reuses the Android `ACTION_VIEW` and iOS `openURL` backends and returns `NoHandler` on desktop. On Android, `geo:` resolves under Android 11+ package visibility (Pixel 5, Android 14, playground share/maps device gate).

**Applies to**: macOS, Linux, and Windows.

**Why accepted**: desktops have no registered handler for these schemes; `Maps::directions` / `directions_to` / `show` are the https path that works everywhere.

**Trigger for removal**: a desktop native-maps route.

**Evidence**: `plugins/url-launcher/src/deeplink.rs`; `plugins/url-launcher/src/maps.rs`.

---

### `url-launcher-mailto-not-launchable` — `mailto:` (and every other non-web scheme) is refused

**Observed**: `UrlLauncher::open_external` runs every URL through `url.rs`'s validator, which accepts only an `http`/`https` scheme immediately followed by `//`. A `mailto:` link — like `tel:`, `sms:` or any custom scheme — returns `UrlLauncherError::InvalidUrl` before any backend runs, and `open_maps_native`'s allowlist covers map schemes only.

**Applies to**: every platform; any app wanting to open a mail composer through the plugin.

**Why accepted**: a security posture, not an omission. The narrow allow-list is what keeps an advisory "open this in the browser" call from ever reaching a script, intent or file handler; widening it is a per-scheme decision with its own validation rules.

**Trigger for removal**: an explicitly allowlisted `mailto:` arm with its own validator, in the shape of `open_maps_native`.

**Evidence**: `plugins/url-launcher/src/url.rs` (rule table and the scheme check); `plugins/url-launcher/src/deeplink.rs`.

---

### `file-dialogs-web-unsupported` — `frust-file-dialogs` has no web backend

**Observed**: on web every terminal call returns `Err(FileDialogError::Unsupported)`; neither `rfd` nor the mobile backends are compiled for it, and the folder API in `frust-photo-library` does not exist on wasm.

**Applies to**: web; every `FileDialog` call.

**Why accepted**: the browser's pickers hand back file handles, not paths or copies, and need their own API shape.

**Trigger for removal**: a web document-picker backend ships.

**Evidence**: `plugins/file-dialogs/src/fallback.rs`; `plugins/file-dialogs/Cargo.toml` (the target tables).

---

### `file-dialogs-mobile-save-file-unsupported` — `save_file` answers `Unsupported` on Android and iOS

**Observed**: `FileDialog::save_file` returns `Err(FileDialogError::Unsupported)` on Android and iOS. The system save UIs hand back no writable path, so the plugin offers `export_file` (the platform's export UI, then a copy) instead.

**Applies to**: Android and iOS; apps that expect a destination path from a save dialog.

**Workaround**: `FileDialog::export_file(&source, Some("name"))`, which works on every platform.

**Why accepted**: a destination path does not exist on those systems; the content is written by the system's export flow.

**Trigger for removal**: none planned; the contract is deliberate.

**Evidence**: `plugins/file-dialogs/src/lib.rs` (crate docs); `plugins/file-dialogs/README.md` (*Platforms*); iPhone SE 2nd gen (iOS 26.7.1) and Pixel 5 (Android 14) device gates.

---

### `file-dialogs-mobile-open-returns-copies` — mobile `open_file` returns an app-private copy

**Observed**: on Android and iOS `open_file` and `open_files` copy each chosen file into the app's cache and return those paths, not the originals (Android: `<cache>/frust-file-dialogs/<unique>/`, iOS: `Caches/frust-file-dialogs/<unique>/`). The plugin does not remove iOS copies; Android copies stay until the OS clears the cache or the call fails or is cancelled. A cloud file is fetched in full first.

**Applies to**: Android and iOS; apps that expect the user's original path or write back to it.

**Workaround**: move or delete the copies when done; use a `FolderGrant` to reach originals.

**Why accepted**: neither system hands a stable file path to a picked document; a copy is the only portable result.

**Trigger for removal**: a platform path that survives the picker, or a plugin cleanup call.

**Evidence**: `plugins/file-dialogs/src/ios.rs` and `android.rs` (module docs); iPhone SE 2nd gen and Pixel 5 device gates.

---

### `file-dialogs-android-persisted-grant-cap` — Android caps the folder grants an app can keep

**Observed**: Android limits the persisted URI permissions per app. At the cap the system either refuses a new grant, which `pick_folder` reports as `FileDialogError::Platform` (distinct from a dismissal), or drops the oldest.

**Applies to**: Android; apps that persist many folders.

**Workaround**: call `FolderGrant::release` on folders no longer needed and keep few grants.

**Why accepted**: a platform quota the plugin cannot raise.

**Trigger for removal**: the platform lifts the cap.

**Evidence**: `plugins/file-dialogs/src/android.rs` (*Folder grants*); `FrustFileDialogsHost.kt` (persist failure path).

---

### `file-dialogs-dialog-not-closable-by-token` — a cancel cannot close a dialog that is already showing

**Observed**: `FileDialog::cancel` and `ListOptions::cancel` take effect before a dialog shows, while the plugin waits and while it copies or lists. A native dialog already on screen cannot be closed: on desktop the token is checked again when the dialog closes, and on Android the call returns `Cancelled` at once while the system dialog stays up until the user dismisses it. A cancel in the narrow window just after the Android host commits can report `Cancelled` for an export that was saved. The token is the plugin's own `CancelToken`, separate from `frust-photo-library`'s, because neither crate can depend on the other on every target.

**Applies to**: desktop and Android; apps with a Cancel button beside a dialog. iOS cancels the wait.

**Workaround**: offer Cancel for the loading phase (`on_progress` has started); convert between the two token types in app code.

**Why accepted**: `rfd` and the Storage Access Framework offer no dismissal call.

**Trigger for removal**: a platform dismissal API.

**Evidence**: `plugins/file-dialogs/README.md` (*Cancel and progress*); `plugins/file-dialogs/src/cancel.rs`; Pixel 5 (Android 14) device gate.

---

### `file-dialogs-ios-icloud-download-in-picker` — the iOS picker downloads an iCloud file before it closes

**Observed**: with copy picks the system picker fetches an iCloud file before it dismisses, so the wait is inside the picker: its own Cancel covers it and the app's Cancel and `on_progress` cannot be reached until it closes.

**Applies to**: iOS; `open_file` and `open_files` on cloud-only files.

**Workaround**: none; tell the user the picker is working.

**Why accepted**: the download is the picker's, not the plugin's.

**Trigger for removal**: a picker mode that returns before the download.

**Evidence**: iPhone SE 2nd gen (iOS 26.7.1) device finding; `plugins/file-dialogs/src/ios.rs`.

---

### `file-dialogs-android-listing-truncation-unflagged` — a long Android listing is cut short without a flag

**Observed**: the Android listing keeps one path component per provider name and stops after 16 times `max_entries` visits. A tree with many entries that do not match returns a shorter list with no marker that it stopped.

**Applies to**: Android; `FolderGrant::list` and `PhotoLibrary::scan_folder` over large trees. `FolderGrant::entry` does not depend on a listing and is not cut short by this bound.

**Workaround**: filter by extension and narrow the folder; page with `scan_folder`.

**Why accepted**: the bound keeps a slow provider from running unbounded; a flag needs an additive API.

**Trigger for removal**: a truncation flag on the listing result.

**Evidence**: `FrustFileDialogsHost.kt` (`WALK_VISIT_MULTIPLE`); `plugins/file-dialogs/src/android.rs`.

---

### `file-dialogs-android-entry-name-match` — an Android `FolderGrant::entry` lookup matches display names, one listing per segment

**Observed**: on Android `entry` resolves a relative path by listing each folder on the way and comparing provider display names exactly and case-sensitively. Where a provider returns two documents with the same display name in one folder, the first in name order wins and the other cannot be reached by path. The cost is one directory listing per path segment, so a lookup in a deep tree is slow on a slow provider. The lookup reads every child of a folder, so it is not cut short by the visit bound in `file-dialogs-android-listing-truncation-unflagged`.

**Applies to**: Android; `FolderGrant::entry`, and apps that store a relative path and reopen it. Desktop and iOS resolve through the filesystem.

**Workaround**: keep the `FolderEntry` from a listing when reading many files in one session; do not rely on two same-named files in one folder being told apart by path.

**Why accepted**: the Storage Access Framework has no path-to-document call; a document id comes only from the provider, so the plugin walks listings.

**Trigger for removal**: a provider-side path lookup, or an id-based entry API.

**Evidence**: `plugins/file-dialogs/src/folder.rs` (`FolderGrant::entry`); `FrustFileDialogsHost.kt` (`resolveTree`); OnePlus 9 (Android 15) run recorded in `examples/playground/README.md` (*Photos: Folders panel Open stored path*): a top-level and a nested file found with the listing's sizes, a missing path not found, found again after relaunch and reboot with no re-pick. That run used primary shared storage only. A non-primary provider and duplicate display names were NOT RUN, so neither the first-match rule nor a second provider is device-verified.

---

### `file-dialogs-android-work-pool-stall` — a hung provider can stall Android file-dialog work

**Observed**: the Android host runs copies and listings on a four-thread pool. Four calls blocked on a hung document provider hold every thread, and later calls wait behind them. A cancel stops the wait but not a read already blocked in the provider.

**Applies to**: Android; apps reading cloud or removable providers.

**Workaround**: cancel and retry later; avoid unresponsive providers.

**Why accepted**: a blocked provider read cannot be interrupted from the app.

**Trigger for removal**: per-call deadlines or isolated pools.

**Evidence**: `FrustFileDialogsHost.kt` (the work executor).

---

### `file-dialogs-export-stale-temp-after-kill` — a process kill can leave a hidden temporary export file

**Observed**: an export copies in 64 KiB chunks to a hidden temporary sibling of the destination (mode 0600) and renames it once complete. A failure or cancel removes the sibling; killing the process mid-copy leaves it. On Unix the source's access bits are applied best effort (setuid and setgid are dropped) and a volume with fixed modes keeps the private mode.

**Applies to**: desktop; `export_file` and `FolderEntry::copy_to`.

**Workaround**: sweep hidden leftovers beside export destinations after an abnormal exit.

**Why accepted**: no signal-safe cleanup exists; an existing destination is never touched.

**Trigger for removal**: a startup sweep or an exclusive-create staging directory.

**Evidence**: `plugins/file-dialogs/src/export.rs` (`copy_file_with`).

---

### `file-dialogs-entry-reopen-follows-links` — a listed entry is reopened by path

**Observed**: `list` neither follows nor lists links, but `copy_to` and `read_to_vec` reopen the entry by its path. A file replaced by a link between the listing and the read is opened through the link.

**Applies to**: desktop and iOS; folders other programs write to.

**Workaround**: list again just before reading; do not grant folders an untrusted program controls.

**Why accepted**: the platforms give no handle that survives the listing.

**Trigger for removal**: handles kept from the listing.

**Evidence**: `plugins/file-dialogs/src/folder.rs`, `walk.rs`.

---

### `file-dialogs-ios-entry-needs-grant` — an iOS entry is readable only while its grant is alive

**Observed**: a `FolderGrant` holds the folder's security scope while it lives. After `release` or a drop, `FolderEntry::copy_to` and `read_to_vec` fail with `AccessLost` where the system's denial can be told apart, otherwise `Io`.

**Applies to**: iOS; apps that keep entries after the grant.

**Workaround**: keep the grant alive while any entry is in use.

**Why accepted**: the scope belongs to the bookmark; the system ends it with the grant.

**Trigger for removal**: none planned.

**Evidence**: `plugins/file-dialogs/README.md` (*Folder access*); iPhone SE 2nd gen device gate.

---

### `file-dialogs-android-release-revokes-all-grants` — Android `release` ends access for every grant on that folder

**Observed**: `FolderGrant::release` revokes the persisted permission for the folder, including for any other `FolderGrant` on it; later calls on those return `AccessLost`.

**Applies to**: Android; apps holding two grants on one folder.

**Workaround**: keep one grant per folder.

**Why accepted**: the persisted permission is the grant, one per tree URI.

**Trigger for removal**: none planned.

**Evidence**: `plugins/file-dialogs/src/android.rs` (*Folder grants*).

---

### `file-dialogs-windows-app-modal` — the dialog has no owner window

**Observed**: the plugin never gives `rfd` a parent window, so on Windows `rfd` shows the `IFileOpenDialog`/`IFileSaveDialog` with a null owner. The dialog is not modal to the app's window: the window stays interactive (the call runs off the UI thread under `spawn_blocking`) and the dialog can fall behind it. The Linux portal request likewise carries no parent-window identifier. The Windows gate (2026-10-18) recorded the round trips only, not the dialog's modality.

**Applies to**: Windows (observably); every `FileDialog` call.

**Why accepted**: the plugin depends on no `frust-*` crate and has no handle to the shell's window, and the facade exposes no window-handle seam to pass one through. The round trip itself is unaffected.

**Trigger for removal**: a shell-provided window-handle seam the plugin can hand to `rfd::FileDialog::set_parent`.

**Evidence**: `plugins/file-dialogs/src/desktop.rs` (`build` sets title, directory, file name and filters, no parent); `rfd` 0.17's Windows backend passes a null owner to `IFileDialog::Show` when no parent is set.

---

### `file-dialogs-linux-runtime-deps` — Linux dialogs need a portal or `zenity` at runtime

**Observed**: on Linux `rfd` asks xdg-desktop-portal's file chooser over D-Bus (libdbus is loaded at runtime, nothing links at build time) and falls back to `zenity`. With neither present, `rfd` logs an error and returns no selection: the call yields `Ok(None)` or an empty `Vec`, indistinguishable from a user cancel. On a zenity-only desktop `rfd` splits a multi-selection on `|`, so `open_files` splits a file name containing `|` into two paths. Threading follows `rfd`: on macOS it marshals the panel to the main thread through dispatch when `NSApplication` is running (the main thread blocks while the panel runs), on Windows it initialises COM per calling thread, and on Linux the portal call blocks only the calling thread. The playground's Desktop page exercised all three threading legs from `spawn_blocking` on 2026-10-18 (Linux, macOS, Windows).

**Applies to**: Linux desktops without a running xdg-desktop-portal file-chooser backend (GTK, KDE, …) and without `zenity`; the `|` split to zenity-only desktops.

**Why accepted**: `rfd`'s synchronous API cannot report a backend failure, and the default features (`xdg-portal`, not `gtk3`) are what keep GTK out of the build. An app that must tell the two apart has to check for a portal itself.

**Trigger for removal**: `rfd` surfaces backend failures through its synchronous API, or the plugin probes for a backend before showing the dialog.

**Evidence**: `plugins/file-dialogs/src/lib.rs` (threading note, *Cancellation and failure*); `plugins/file-dialogs/README.md` (*Linux runtime requirements*, threading paragraph).

---

### `secure-storage-insecure-file-debug-only` — the plaintext file fallback is debug-desktop-only and not hardened

**Observed**: `frust-secure-storage`'s `insecure_file` backend (opt in via `StoreOptions::with_insecure_file_fallback()` or `FRUST_SECURE_STORAGE_INSECURE_FILE=1`) stores values as plaintext JSON at `data_dir()/frust-secure-storage/insecure-store.json` (0600 on unix, atomic writes). It is compiled only in debug builds on desktop; release builds and mobile log an error and use the native backend. The lock is in-process only, with no cross-process locking, and biometric-gated opens answer `NotAvailable(UnsupportedPlatform)`. Linux and Windows behaviour is verified by host tests and cross-target checks only, not on a device.

**Applies to**: debug builds on macOS, Linux, and Windows that opt in.

**Why accepted**: it exists so `cargo run` previews work on a box without a keyring; confining it to debug builds keeps plaintext secrets out of shipped apps.

**Trigger for removal**: none anticipated; the fallback stays a development aid.

**Evidence**: `plugins/secure-storage/src/insecure_file.rs`.

---

### `secure-storage-linux-no-secret-service` — Linux without a Secret Service daemon cannot open a secure store

**Observed**: `frust-secure-storage`'s Linux backend goes through `keyring-core` and a Secret Service daemon; on a host with none (a bare session or container), store init fails. Debug builds can opt into the plaintext fallback (see `secure-storage-insecure-file-debug-only`); the error text hints `FRUST_SECURE_STORAGE_INSECURE_FILE=1`. Release builds have no fallback.

**Applies to**: Linux hosts without a running Secret Service (e.g. gnome-keyring, KeePassXC).

**Why accepted**: a secure store without an OS keyring would be plaintext; the debug-only fallback covers development, and production hosts are expected to run a keyring.

**Trigger for removal**: a hardened non-keyring Linux backend, or a documented headless provisioning path.

**Evidence**: `plugins/secure-storage/src/desktop.rs`; `plugins/secure-storage/src/insecure_file.rs`.

---

### `auth-session-android-callback-rides-intent-filter` — Android cannot prove the OAuth callback scheme is bound to this app

**Observed**: `FrustAuthSessionHost` observes the redirect through the same ordinary custom-scheme `<intent-filter>` (`action.VIEW` + `BROWSABLE` + `android:scheme`) every frust app registers for deep links (`examples/playground/android/app/src/main/AndroidManifest.xml`'s `frustplay` filter) — a plain custom scheme carries no Android App Links-style domain verification, so any other installed app that claims the same scheme string can intercept or forge the redirect before this host ever sees it. The slot is also exposed to injected intents from the same app (a compromised Activity can call `startActivity` with the callback scheme), and to tapped links in the browser or another app that claim the same scheme. The session generation does not narrow any of this: the host keeps only the latest launch's generation in its single `pending` slot and stamps whichever callback-scheme `Intent` it observes on the next Activity resume with that value — the generation never travels through the browser or the redirect URL, so a superseded tab's late redirect, or an injected `Intent`, is attributed to the live session by timing rather than evidence. The generation only lets the Rust side discard a result for a session that is no longer live (dropped future, already resolved).

**Applies to**: Android only; any app using this plugin whose callback scheme collides with another installed app's registered scheme, or that starts a new session while a previous tab is still open.

**Why accepted**: this is RFC 8252's own documented risk for a custom-scheme redirect (its §8.1 recommends reverse-domain-unique scheme naming, which narrows but does not eliminate the window), not a defect this plugin introduces — callers MUST use PKCE and verify the state parameter to defend against forged/injected callbacks (see `README.md` §3.4 *Security*). HTTPS App Links (domain-verified) or Android's newer Auth Tab API would close the gap, but neither is in this plugin's v1 scope.

**Trigger for removal**: an HTTPS App Links or Auth Tab callback path exists and `AuthSessionRequest` grows a way to select it; or redirects observed after a superseded launch are refused (the host would need to remember that a tab it no longer tracks is still open).

**Evidence**: `plugins/auth-session/platform/android/src/main/kotlin/dev/frust/authsession/FrustAuthSessionHost.kt`'s class doc (*Why the outcome is read from `activity.intent`*, *The double-delivery note*) and its `pending` slot; `plugins/auth-session/src/android.rs` (*The generation round trip*); `examples/playground/android/app/src/main/AndroidManifest.xml`'s `frustplay` intent filter.

---

### `auth-session-android-ephemeral-browser-dependent` — `ephemeral` on Android is advisory to the Custom Tabs provider

**Observed**: `FrustAuthSessionHost` calls `CustomTabsIntent.Builder#setEphemeralBrowsingEnabled(true)` (present in the pinned `androidx.browser:browser 1.10.0`), but the flag is a request the provider may ignore: on the test device (Xiaomi 12, LineageOS 23.2) the provider selected was Fennec F-Droid 129.0.0 and two consecutive ephemeral sessions both saw the `frustauth=1` cookie set by an earlier session. In a later check the allow-listed provider was Chrome 153, which honoured the flag (an ephemeral session did not see the normal jar's cookie; a later non-ephemeral session did) — the behaviour is per provider, not per platform. Dropping the awaited future releases the session Busy slot immediately.

**Applies to**: Android only; apps relying on `ephemeral: true` for cookie isolation must not assume it holds on every browser.

**Why accepted**: the provider is chosen from a curated allow-list of well-known providers (default browser first if it supports Custom Tabs, then the first provider from the list that is installed and answers `CustomTabsService`); there is no portable way to demand ephemeral support short of binding to the service and checking `CustomTabsClient.isEphemeralBrowsingSupported`, which is outside v1.

**Trigger for removal**: the host probes ephemeral support before launching (or prefers a provider that reports it) and the gate records `no cookie (set now)` on a second ephemeral visit.

**Evidence**: `plugins/auth-session/platform/android/src/main/kotlin/dev/frust/authsession/FrustAuthSessionHost.kt` (`launch`); the two-session cookie check on the test device described under Observed.

---

### `auth-session-android-resume-means-cancelled-v1` — Activity resume while a session is pending is reported as Cancelled

**Observed**: An unrelated Activity resuming while `AuthSession::start`'s future is still live (e.g. a permission dialog, app switch, or system event) triggers an `onActivityResumed` callback whose resumed `Intent` carries no callback-scheme data; the host clears its pending session and reports kind `1`, so the future resolves `Ok(AuthSessionOutcome::Cancelled)` — indistinguishable from the user dismissing the Custom Tab. The tab itself is not closed by this: it stays on screen until the user dismisses it, and a redirect it delivers afterwards finds no pending session and is dropped (the app's own deep-link stream still observes it — `plugins/auth-session/README.md` § 5). Error kind `4` (`Platform`, "no resumed Activity") is a different, launch-time path: `start`'s posted runnable found no resumed Activity to launch from, so no tab was ever opened.

**Applies to**: Android only; any app using this plugin whose UI context allows or expects graceful recovery from interruption while an auth session is pending.

**Why accepted**: v1 scope — differentiating between user-dismissed tabs and system-level resume events would require tracking session state and callback origin more finely than the current host does. The one-session `Busy` guard ensures a stuck session never wedges a later one, so an app that retries the auth flow after an interruption will not deadlock.

**Trigger for removal**: session lifecycle is tracked granularly, distinguishing user dismissal (Cancelled) from Activity-level interruption (a new `Interrupted` outcome or a more specific error variant).

**Evidence**: `plugins/auth-session/platform/android/src/main/kotlin/dev/frust/authsession/FrustAuthSessionHost.kt` (`onActivityResumed`); `plugins/auth-session/README.md` §3 (the *Attribution is by timing, not evidence* bullet).

---

### `auth-session-loopback-first-match-wins-v1` — the loopback listener accepts the first well-formed callback request, not a verified one

**Observed**: `LoopbackSession`'s accept loop resolves on the first request to the reserved path that passes its structural checks (well-formed request line, an exact `Host: 127.0.0.1:<port>` match, `GET`, the reserved path, a URL that passes the crate's validator) — from *any* local client able to reach the loopback interface, another local process or a local web page, not provably the system browser tab this session opened. The listener has no way to tell that tab's own request apart from one any other local process sends to the same port first.

**Applies to**: every `LoopbackSession`, on Linux, Windows, and macOS alike.

**Why accepted**: this is RFC 8252 §8.3's own documented loopback-interface risk, not a defect this crate introduces; the defence is the same PKCE + `state` check every custom-scheme callback already requires (`frust_oauth_native::parse_callback`), binding the accepted request to the authorization request this app made — a forged or racing hit still fails that check, and the one success the session was ever going to report is consumed by whichever request got there first.

**Trigger for removal**: none anticipated — this is RFC 8252's own loopback-redirection trust model, not a deferral.

**Evidence**: `plugins/auth-session/src/loopback.rs`'s module doc (*What the listener answers*, *Security*); `plugins/oauth-native/src/callback.rs`'s `parse_callback` (re-exported from `plugins/oauth-native/src/lib.rs`).

---

### `auth-session-loopback-local-stall-v1` — a same-user local process can stall acceptance of the real browser redirect

**Observed**: `LoopbackSession`'s accept loop serves one connection at a time; a same-user local process that opens silent or slow connections to the port delays acceptance of the real browser redirect by up to one request-head budget (`CONNECTION_IO_TIMEOUT`, 2 s) per connection it opens, for as long as it keeps doing so. Since commit `5e686576` each connection's read runs in 25 ms polling passes rather than blocking for the full budget, so cancel, a dropped future, and `LoopbackOptions::timeout` still take effect within about one poll interval even mid-connection — the stalling process delays the real callback, it cannot wedge the `Busy` slot past the app's own timeout.

**Applies to**: every `LoopbackSession`, on Linux, Windows, and macOS alike.

**Why accepted**: RFC 8252 §8.3 already concedes any local process can reach the loopback port; serving one connection at a time keeps the hand-written HTTP handling minimal (no per-connection threads, no HTTP crate dependency), and a hostile same-user process that can open sockets can already do worse than this. `state` + PKCE still protect the result the session reports, and the interruptible per-connection read bounds the UX damage to the app's own cancel/timeout path rather than the full request-head budget.

**Trigger for removal**: a redesign that accepts a bounded number of connections concurrently, or rate-limits repeat connections from the same peer.

**Evidence**: `plugins/auth-session/src/loopback.rs`'s `accept_loop`/`serve`/`read_head` and its module doc's *Security* section; its tests `cancel_resolves_promptly_despite_a_silent_connection`, `timeout_resolves_promptly_despite_a_silent_connection`, `dropping_the_future_closes_the_port_promptly_despite_a_silent_connection`, `five_silent_clients_do_not_starve_a_later_request`.

---

### `auth-session-loopback-url-in-launcher-argv-v1` — the authorization URL is briefly visible in another local user's process listing

**Observed**: on Linux and macOS, `LoopbackSession::start` hands the full authorization URL (including `state` and `code_challenge`) to `frust-url-launcher`'s `open_external`, which passes it as a `Command` argument to `xdg-open`/`open` — readable by other local users through `/proc/<pid>/cmdline` on Linux or `ps` on macOS for the launcher process's short lifetime. The S256 `code_challenge` does not reveal the PKCE verifier, so a reader cannot complete the token exchange, but `state` is not secret from other users on a shared host.

**Applies to**: Linux and macOS. Windows' `ShellExecuteW` arm does not spawn an argv-visible helper process.

**Why accepted**: inherent to launching a browser by URL through an external opener; RFC 8252 already treats the authorization request URL as visible to the user agent. No part of this crate's threat model assumes privacy from other local users on a shared host.

**Trigger for removal**: none anticipated; an app on a shared host should treat the signed-in identity as the trust boundary and surface it to the user after the exchange completes.

**Evidence**: `plugins/url-launcher/src/desktop.rs`'s `open_external` (the `Command::new(program).arg(url)` call on the `xdg-open`/`open` arms); `plugins/auth-session/src/loopback.rs`'s `open_in_browser`/`start` (the `frust_url_launcher::UrlLauncher::open_external` call).

---

### `auth-session-no-cancel-v1` — no way to cancel a live session from Rust

**Observed**: `AuthSession` exposes no `cancel` method — once `start` returns a pending future, it only resolves when the identity provider redirects, the user dismisses the platform tab themselves, or a platform-level failure occurs; there is no programmatic way for an app to dismiss a live session (e.g. on its own timeout or navigation-away). Dropping the awaited future (not awaiting it, or awaiting and then discarding the result) releases the session Busy slot immediately without waiting for platform resolution — but it does **not** end the platform UI: on Android the Custom Tab stays on screen until the user closes it (a redirect it delivers afterwards finds no pending session and is dropped); on iOS/macOS the presented `ASWebAuthenticationSession` sheet stays up until the user dismisses it or the next `AuthSession::start` `-cancel`s it in favour of the new session (its `CanceledLogin` completion is then discarded as a stale generation).

**Applies to**: `AuthSession::start`'s platform backends only — Android, iOS, macOS (the in-app browser-tab path). `LoopbackSession` is unaffected: it exposes `LoopbackCancel::cancel` plus a mandatory `LoopbackOptions::timeout`, so a desktop loopback session always has a programmatic way out.

**Why accepted**: v1 scope — the one-session `Busy` guard (this crate's *Exactly one live session* doc section) means a stuck session still cannot wedge a later one forever, even with no cancel path and no forced cleanup — dropping the future frees the slot for a new session.

**Trigger for removal**: `AuthSession::cancel` exists (dismissing `ASWebAuthenticationSession` via `-cancel`, finishing the Android Custom Tab activity), resolving the live session with `AuthSessionOutcome::Cancelled`.

**Evidence**: `plugins/auth-session/src/lib.rs`'s public API (`AuthSession::start`/`is_supported` only); `plugins/auth-session/README.md` §2.

---

### `auth-session-ios-https-callback-not-supported-v1` — iOS 17.4's HTTPS App-Link callback form is unavailable

**Observed**: the Apple backend calls the deprecated `-initWithURL:callbackURLScheme:completionHandler:` initializer exclusively (`apple.rs`'s `make_session`), never iOS 17.4's `-initWithURL:callback:completionHandler:`, which can express an HTTPS App-Link callback (`ASWebAuthenticationSessionCallback`) as well as a custom scheme — only a custom `callback_scheme` redirect is supported, never an HTTPS callback URL.

**Applies to**: iOS and macOS alike (both route through the same `apple.rs` module).

**Why accepted**: this crate's deployment floor is iOS 15, where the newer initializer does not exist at all — supporting the HTTPS-callback form would need a second, floor-gated code path for a callback shape v1 does not offer. v1 is served instead by the private-use-scheme `AuthSession::start` path, or, on desktop, by `LoopbackSession`'s plain-`http` loopback redirect.

**Trigger for removal**: raising the crate's deployment floor to iOS 17.4, or adding a floor-gated second path that uses the newer initializer when available.

**Evidence**: `plugins/auth-session/src/apple.rs`'s module doc (*The iOS 15 floor and the deprecated initializer*) and its `make_session` function.

---

### `desktop-pinch-linux-windows-unavailable` — trackpad pinch has no desktop `Scale` source on Linux or Windows

**Observed**: winit 0.30.13 emits `WindowEvent::PinchGesture` only on macOS (and iOS, which has no desktop shell); it never emits that variant on Linux or Windows at this pin, so a two-finger trackpad pinch there produces no `InputEvent::Scale`. The ctrl/⌘+wheel mapping (`frust-shell-desktop`'s `map_wheel_scale_delta`) is the only desktop zoom-gesture source on those platforms.

**Applies to**: `frust-shell-linux` and `frust-shell-windows`; macOS is unaffected.

**Why accepted**: the gesture is winit's to emit, not this crate's; widening it would mean reading raw trackpad/touchpad events per platform (XInput2/libinput on Linux, a raw-input precision-touchpad API on Windows) outside winit's abstraction, which this unit does not do today.

**Trigger for removal**: winit gains `PinchGesture` support on Linux and/or Windows, or this unit adds a platform-specific trackpad-gesture source feeding the same `InputEvent::Scale`.

**Evidence**: `crates/frust-shell-desktop/src/app_handler.rs`'s `WindowEvent::PinchGesture` arm comment.

---

### `transformed-subtree-semantics-aabb` — a transformed pod's semantics subtree is reported as its axis-aligned bounding box, not its exact shape

**Observed**: `ChildPod::semantics_child` reports a pod under a `set_transform` at the axis-aligned bounding box of the transformed child rect; descendants are offset from that box's corner unscaled and unrotated, so a rotated or non-uniformly-scaled subtree's accessibility bounds are an approximation rather than its true painted shape.

**Applies to**: any widget placed under `ChildPod::set_transform` with a rotation or non-uniform scale — in-tree, `pan_zoom`'s child pod (uniform scale only, so this is exact there in practice) and any future rotating container.

**Why accepted**: v1 scope for the transformed-pod seam — hit-testing and paint already map exactly through the inverse transform; only the semantics tree, which accesskit's own node model expects axis-aligned, takes the approximation.

**Trigger for removal**: an accesskit node shape richer than an axis-aligned rect, or a documented need for exact rotated-bounds accessibility reporting.

**Evidence**: `crates/frust-core/src/widget.rs`'s `ChildPod::semantics_child` doc comment.

---

### `canvas-view-no-semantics-node` — `CanvasWidget` publishes no accessibility node

**Observed**: `canvas()` builds a `CanvasWidget` that implements no `Widget::semantics` override, so arbitrary painted content (a chart, a node-and-edge graph, a game board) is invisible to `RenderRoot::inspect()` and to an assistive-technology client.

**Applies to**: every `CanvasView`/`CanvasWidget` instance, including `pan_zoom`'s typical child.

**Why accepted**: there is no generic accessible role for arbitrary painted content — a `canvas` caller who needs one composes it from ordinary semantics-carrying widgets instead, or layers its own `Widget::semantics` implementation outside this helper.

**Trigger for removal**: `CanvasView` grows an opt-in semantics builder (a label/role/bounds callback) a caller can attach.

**Evidence**: `crates/frust-widgets/src/canvas.rs` — no `semantics` method on `CanvasWidget`.

---

### `secondary-contact-walk-overlay-fallback` — a captor inside a floated overlay surface is reached through the ordinary-delivery fallback, re-admitting ancestor visibility of the other contact

**Observed**: `ChildPod::walk_secondary`'s forward-only walk hands each container between the root and the captor an inert `InputEvent::Overlay` broadcast instead of its own pointer handling; when a container on the path does not forward that broadcast to its children (an overlay owner whose captured pod is a floated surface, which does not route an arbitrary-key broadcast to an arbitrary descendant), the walk falls back to delivering the real event to that one child directly — re-admitting that ancestor's ordinary pointer-handling visibility of the other contact for that configuration, the thing the forward-only walk otherwise exists to prevent.

**Applies to**: a capturing widget reached through a floated overlay surface (an overlay-hosted draggable or canvas) while another contact of the same gesture is live.

**Why accepted**: a pod cannot reach into its child widget's own pods, so the walk has no route into an overlay's content other than the broadcast channel every container already provides; closing the gap needs a route the overlay seam does not have today, not a bug in the routing shipped.

**Trigger for removal**: an explicit per-pod secondary-contact route, or a mutable child visitor the root can use to address an arbitrary descendant pod directly instead of riding the broadcast channel.

**Evidence**: `crates/frust-core/src/widget.rs`'s `ChildPod::walk_secondary`/`event_child` doc comments (the "On the path, above the captor" fallback case).

---

### `create-registry-needs-network-once` — registry-mode `frust create` needs the network once before it can write the Android/iOS wiring

**Observed**: `frust create` (the default, registry mode) resolves the embedding and plugin native-module locations with one `cargo metadata` run, and in a project that has not built yet cargo downloads the frust crates to answer it. Offline, that run fails, so `android/local.properties` and the `ios/<Package>` symlinks are not written; the project itself is created (the TUI scaffold shows a warning toast).

**Applies to**: `frust create` and the TUI's new-project flow with an empty or unseeded cargo registry cache and no network; `--frust-path` projects whose checkout is local are not affected.

**Why accepted**: the wiring points at the directories cargo resolves for the project, which only exist once cargo has fetched the crates; writing a guess would put a machine path back into the project. `frust create --no-sync` skips the step for offline use and defers it: the next `frust run`/`frust build` with `-d android|ios` (online) performs it, and `frust doctor`'s "Platform packages" section shows what a project currently resolves.

**Trigger for removal**: none planned; the wiring depends on the resolved crate directories.

**Evidence**: `crates/frust-drive/src/platform_wiring.rs` and `crates/frust-drive/src/packages.rs` module docs; `cli::CreatePlatformArgs` (`--no-sync`); `crates/frust-tui/src/runner.rs`'s `scaffold_messages`.

---

### `scaffold-0.5.0-path-dependency` — projects scaffolded by frust-cli/frust-tui 0.5.0 (yanked) carry checkout paths and are not migrated automatically

**Observed**: a project scaffolded by the yanked 0.5.0 `frust-cli`/`frust-tui` depends on `frust` by a path into a framework checkout, and its tracked `android/gradle.properties` (`frust.embedding.dir`) and `ios/Runner.xcodeproj/project.pbxproj` (the `FrustEmbedding` package `relativePath`) hold machine paths into that checkout, as do the `settings.gradle.kts` plugin-module lines (`projectDir = file("<checkout>/…/plugins/<p>/platform/android")`, written by 0.5.0 plugin apply). `frust run`/`frust build` (`platform_wiring::sync_with`) refreshes only an existing `gradle.properties` `frust.embedding.dir`; it never rewrites `Cargo.toml`, those `settings.gradle.kts` lines or the pbxproj `relativePath`.

**Applies to**: projects created by 0.5.0 only; 0.5.1 and later scaffold registry projects with no machine path in a tracked file.

**Why accepted**: the set of affected projects is small (0.5.0 was yanked) and the edit is mechanical; an automatic migration would rewrite user-owned manifests.

**Workaround**: re-scaffold with 0.5.1+ and move the app's sources over, or edit by hand: in `Cargo.toml` replace the `frust` path dependency (and any `frust-*` plugin path dependencies) with the registry form `frust = { package = "frust-ui", version = "<release>" }` and `frust-<plugin> = "<release>"`; in `android/gradle.properties` leave `frust.embedding.dir` to be refreshed by `frust run`/`frust build -d android` (it remains a tracked machine path); in `project.pbxproj` set the `XCLocalSwiftPackageReference "FrustEmbedding"` `relativePath` to `"FrustEmbedding"` so the `ios/FrustEmbedding` symlink that `frust run`/`frust build -d ios` creates resolves it; copy the `frustLocalDir` helper block (the `frustLocalPropertiesFile`/`frustLocalProperties` vals and `fun frustLocalDir(key: String)`) from a freshly scaffolded 0.5.1+ project's `android/settings.gradle.kts` above the plugin blocks, then replace each plugin `projectDir = file("<checkout>/…")` line with `project(":<module>").projectDir = frustLocalDir("frust.plugin.<module>.dir")` (or re-scaffold), add `/ios/Frust*` to `.gitignore`, then run `frust run` once.

**Trigger for removal**: a `frust` migration command, or 0.5.0 projects falling out of use.

**Evidence**: the 0.5.0 templates (`crates/frust-drive/templates/app/` before the platform-wiring change).

---

### `network-image-no-http-cache-headers` — the network image cache ignores HTTP caching headers

**Observed**: `frust-network-image` never reads `ETag`, `Last-Modified`, `Cache-Control` or `Expires`, and sends no conditional request. A disk-cached image is served until evicted by the LRU cap or cleared, however stale the origin copy is.

**Applies to**: every `network_image` view whose URL's content changes while the same URL stays cached (memory or disk).

**Why accepted**: the cache keys on the URL (plus a digest of the request headers) and treats a successful fetch as immutable, which is right for the content-addressed and versioned URLs the plugin targets; revalidation needs a conditional-request path through the `ImageFetcher` seam.

**Trigger for removal**: conditional requests (`If-None-Match`/`If-Modified-Since`) and expiry are added to the disk cache.

**Evidence**: `plugins/network-image/src/disk_cache.rs` (index stores only size and last-access time) and `src/fetch.rs` (`FetchedBytes` carries bytes and content type only).

---

### `network-image-cache-holds-private-bodies` — bodies fetched with credentials persist unencrypted in the app cache directory

**Observed**: an image fetched with request headers (for example `Authorization`) is cached like any other: decoded in memory and, with the disk cache enabled, as raw bytes in a file under the app cache directory. The cache key includes a digest of the headers, so another credential never receives it, but the file is not encrypted and stays until LRU eviction or `clear_disk`/`clear_all`. There is no TTL and no `Cache-Control: no-store`/`private` handling.

**Applies to**: `network_image` views and `ImageLoader` loads carrying credential headers, with the disk cache enabled (the default).

**Why accepted**: the app cache directory is sandboxed per app on Android and iOS, and the README tells apps to call `NetworkImages::loader().clear_all()` on logout or account switch; honouring cache directives needs the response-header path from the `network-image-no-http-cache-headers` entry.

**Trigger for removal**: encrypted storage or `no-store`/`private` handling for responses fetched with credentials.

**Evidence**: `plugins/network-image/src/disk_cache.rs` (`hex_key`, scope-folded file name) and `plugins/network-image/src/loader.rs` (`load`, `acquire_raw`).

---

### `network-image-full-size-decode-peak` — a large image is fully decoded before it is downscaled

**Observed**: `image` 0.25 has no JPEG DCT-scaled decode, so `target_px` shrinks the image only after a full-resolution decode; the native decoded bitmap is alive for the whole fit. The dominant transient is the resize's `f32` intermediate (source width x pass height x 16 bytes), held alongside the native image. `DecodeLimits::max_alloc_bytes` (default 512 MiB; `LoaderConfig::decode_limits`) budgets the decode-and-fit working set the header can predict (native bitmap plus the resize intermediate or the RGBA copy) and rejects with `DecodeError::OverBudget` before any pixel is decoded; declared dimensions over `max_width`/`max_height` (default 8192) are likewise rejected from the header with `DecodeError::TooLarge`. Only the downscaled result is cached in memory.

**Applies to**: decoding a high-resolution photo on a memory-constrained device; the fetch is capped (20 MiB by default) and the estimated peak is capped by `DecodeLimits`, but a within-budget photo still peaks near that budget. The bound is PER DECODE: concurrent decodes add up.

**Why accepted**: the cap bounds download size, decode runs off the UI thread, and the resident cost after decode is the fitted bitmap; a scaled-decode path needs a different decoder.

**Trigger for removal**: a decoder with DCT-scaled or tiled JPEG decode.

**Evidence**: `plugins/network-image/src/decode.rs` (`peak_estimate` and `decode_and_fit`: header pre-check, Triangle pre-shrink for >2x, then Lanczos3, after the full decode).

---

### `network-image-no-svg` — the network image view does not decode SVG

**Observed**: an `.svg` URL fetches successfully and then fails to decode, so the view shows its error slot. WebP decodes only with the opt-in `webp` feature; without it the failure is `DecodeError::UnsupportedFormat`.

**Applies to**: `network_image` with an SVG (or, without `webp`, a WebP) source.

**Why accepted**: the plugin decodes raster formats through `image`; vector rendering is a separate capability the baseline widgets do not provide.

**Trigger for removal**: a vector-image capability in the framework, or an SVG-rasterizing decode step.

**Evidence**: `plugins/network-image/src/decode.rs` (`DecodeError`).

---

### `network-image-android-cache-dir-late` — an Android load issued before platform init is memory-only for that process

**Observed**: `frust_paths::cache_dir()` is `None` until the shell runs `nativeInitPlatform`. A `network_image` load issued before then runs memory-only (one `log::info`), and the disk cache is not retried for the rest of that process.

**Applies to**: Android, a load started before native platform initialization (for example from a static initializer).

**Why accepted**: every shell initializes before the first frame, so views built from `build` are unaffected; the disk cache is opened lazily on the first miss and, once absent, stays absent.

**Trigger for removal**: re-resolving `cache_dir()` on a later miss, or a shell ordering guarantee that makes the case unreachable.

**Evidence**: `plugins/network-image/src/loader.rs` (lazy disk open on first miss); `crates/frust-paths` (shell-installed slot).

---

### `network-image-disk-hit-decode-failure-not-evicted` — a disk cache entry that fails to decode is not evicted

**Observed**: when bytes read from the disk cache fail to decode (corrupt file, unsupported format after the `webp` feature was turned off), the load fails and the entry stays; the view shows its error slot on every attempt until the entry is evicted by the LRU or `clear_disk`/`clear_all` is called.

**Applies to**: disk-cache hits whose bytes no longer decode.

**Why accepted**: fetched bytes are written only after a successful fetch and writes are atomic, so a corrupt entry is rare; automatic eviction would risk refetch loops for a genuinely undecodable origin image.

**Trigger for removal**: evict-on-decode-failure plus a single refetch in `ImageLoader`.

**Evidence**: `plugins/network-image/src/loader.rs` (disk-hit decode path); `plugins/network-image/src/disk_cache.rs`.

---

### `overlay-unservable-content-stays-on-main` — content the overlay engine cannot draw stays under the native view

**Observed**: with overlays on, a cut whose range holds retained content (a `DrawRetained`
composite, a `PushRetained` recording, or a retained bracket crossing the cut), an external-texture
frame (`SceneTexture`) or a `BackdropBlur` is not drawn on its overlay (`cut_is_servable` returns
`CutServiceability::Unservable` with an `UnservableReason`). The range stays in the main segment,
so that content draws under the native view, and the overlay records a transparent clear
(`OverlayCutOutcome.serviceability` reports each cut). Navigator transitions wrap pages in a
retained bracket, so any cut they cross stays on main for the transition. Servability is decided per frame from the cut's commands, so a retained widget above a native view moves between the overlay (clean frames composite its page: unservable, under the view) and the main surface the same way; content that alternates can therefore change z-order against the native view from frame to frame.

**Applies to**: apps with `FRUST_OVERLAYS` on (Android, iOS, macOS) that paint such content above a
native view. On the iOS Simulator a navigation transient logs `content kept on main: retained
content (a bracket crossing the cut)` once.

**Why accepted**: the overlay path projects a command range without the retained-page store, the
external-texture bindings or the backdrop copy; dropping the content would make it vanish, and
drawing it under the view keeps it visible.

**Trigger for removal**: an overlay engine that can serve retained pages and external textures.

**Evidence**: `crates/frust-render/src/overlay.rs` (`cut_is_servable`, `UnservableReason`),
`crates/frust-render/src/renderer.rs` (`encode_segmented`),
`frust_engine::renderer::open_retained_bracket`; `crates/frust-render/tests/segmented.rs`.

---

### `apple-overlay-limits` — Apple overlays cost a full-window layer per cut and stack in creation order

**Observed**: with `FRUST_OVERLAYS` on (iOS Mode A; macOS), each differ cut gets its own overlay
layer the size of the whole window, so cost scales with the number of cuts (the native-widgets
demo's Controls page with overlays on can reach 7; iPhone SE, 750x1334: per-overlay GPU cost
1.3-2.1 ms). Mode A slot views stack in creation order (`z_index` is unused), and a
transparent overlay stays above a hidden slot. Unservable content stays on the main surface
(`overlay-unservable-content-stays-on-main`). iOS overlays need the render-thread split, and Mode B
logs `overlays are Mode A only; a translucent surface ignores them`. With present-sync armed the
main/overlay skew measured avg 296-360 us, max 457-521 us. Device outcomes on the shared overlay
table (2026-10-21): an iPhone 13 mini (iOS 26.7.1) driven by an XCUITest runner and an iPhone 17
Pro simulator (iOS 26.2, Main Thread Checker loaded and silent) passed; on a macOS M4 the window
moved 2x to 1x to 2x with overlays sharp at each scale, and 11 rapid toggles, mid-frame withdraws
included, never waited; idle camera and video texture views stayed live without a ticker on the
iPhone. The default stays off.

**Applies to**: iOS and macOS apps with the knob on that cut a frame around native views many times.

**Why accepted**: the knob is off by default and the common case is one or two cuts. The Apple hosts insert each overlay directly above its own slot's
view, so interleaved content stays under a later native control, unlike
`android-overlays-cover-later-slots`.

**Trigger for removal**: overlay layers sized to their cut, and a `z_index`-aware slot stacking.

**Evidence**: `crates/frust-shell-ios/platform/ios/FrustEmbedding/Sources/FrustEmbedding/FrustViewHost.swift`;
`crates/frust-shell-macos/src/platform_view.rs`; SHELLS_ARCHITECTURE.md § Native-view hosting.

---

### `nv12-import-unsupported` — the Apple importer takes 32BGRA only

**Observed**: `MetalImporter::import` accepts 32BGRA IOSurface-backed `CVPixelBuffer`s only. NV12
(`420v`/`420f`) buffers answer `MetalImportError::Unsupported`, and a null, non-IOSurface or
wrong-type buffer answers `Failed`. Producers keep NV12 on their CPU path.

**Applies to**: texture-bus producers on iOS and macOS that would hand a decoder's native YUV buffer
straight on. The video-player and camera producers request 32BGRA, so they are unaffected.

**Why accepted**: sampling YUV needs a two-plane import plus a conversion pass the bus does not have.

**Trigger for removal**: a YUV (multi-plane) import path, shared with the Android YUV need in
`video-texture-android-gles-copy`.

**Evidence**: `crates/frust-gpu/src/import/apple.rs`.

---

### `texture-bus-no-wake-on-push` — the web shell does not wake on a texture push

**Observed**: a producer push into a registered texture sink owes the shell one frame
(`frust::texture::request_frame`); the mobile frame gates and the desktop loop take it. The web shell
neither takes the flag nor installs the frame-request hook, so a web page showing a `texture_view`
needs a painted frame requester of its own. The hook is process-wide, so it also assumes one shell
per process.

**Applies to**: web apps showing a `texture_view` fed by a custom producer (no shipped plugin
produces textures on web).

**Why accepted**: no shipped producer targets the web shell.

**Trigger for removal**: the web shell takes the owed-frame flag or installs the hook.

**Evidence**: `crates/frust-render/src/texture_bus.rs` (`request_frame`, `take_frame_owed`,
`set_frame_request_hook`); `crates/frust-shell-desktop/src/app_handler.rs`.

---

### `shortcuts-v1-scope` — `Mod` is fixed at compile time

**Observed**: `Mod` resolves at compile time — Command on macOS/iOS, Ctrl everywhere else, `wasm32`
included — so a web build running in a browser on a Mac binds and labels Ctrl (`Ctrl+K`, not `⌘K`).

**Applies to**: web builds running on macOS that bind chords through `shortcuts(..)` or
`PaintCtx::register_shortcut`.

**Why accepted**: a runtime platform probe on web is a separate seam no caller has needed yet.

**Trigger for removal**: a runtime `Mod` resolution on web.

**Evidence**: `crates/frust-core/src/shortcuts.rs` (`Chord::parse`, `Chord::display`).

---

### `baseline-controls-no-focus-visual` — baseline controls paint no keyboard-focus cue

**Observed**: the baseline `Button`, `Checkbox`, `Radio` and `Slider` are Tab stops and answer the
keyboard while focused, but none paints a ring or any other keyboard-focus cue, so tabbing across a
row of them shows nothing moving (a focused `TextInput`'s own border is the only visible stop).

**Applies to**: `frust_widgets`' baseline `Button`, `Checkbox`, `Radio` and `Slider`; design-system
controls paint their own (`frust_shadcn::button` reads `PaintCtx::is_focused_for_ring`).

**Why accepted**: the baseline set carries no design language to draw a ring in;
`PaintCtx::is_focused_for_ring` is the hook a catalog reads today.

**Trigger for removal**: a baseline keyboard-focus visual read through `is_focused_for_ring`.

**Evidence**: `crates/frust-widgets/src/{button,checkbox,radio,slider}.rs` (no `is_focused_for_ring`
read in `paint`); the Shortcuts gate row in `examples/playground/README.md` ("the baseline controls
paint no keyboard-focus ring").

---

### `android-first-frame-carries-shader-module-creation` — the first frame after a surface install waits for the shader modules it draws with

**Observed**: since the shader modules are created on first use (measured 2026-10-19), the first
frame after a surface install on Android waits in `first_submit_pipeline_wait` for the four modules
its own pipelines need, which a pool of threads starts at install: `first_encode_done →
first_frame_presented` is 32 ms median on the OnePlus 9 and 54 on the Pixel 5 against the ≤ 25 ms target
(11 / 14 when the install created every module itself) — while
`first_frame_presented` as a whole fell from 124 to 67 ms and from 214 to 108, with `adapter_ready`
back at September's 32 / 40 ms. Starting the modules on the GPU pre-init thread instead
(2026-10-20) measured 28 / 45: a `wgpu::ShaderModule` needs the device, the pre-init thread owns one
only at the end of its work, and the install follows that moment at once, so the modules never exist
before the install whatever thread starts them.

**Applies to**: the Android shell's first frame after every surface install (launch, and a surface
re-created after a destroy); iOS keeps the install-time pool with the same shape (`adapter_ready` 82
ms there, not yet re-measured). Every later frame is unaffected; the warm-up worker creates the rest
of the modules behind the first frame.

**Why accepted**: the whole first frame is 57 / 106 ms earlier than before the change on the two
phones measured, and the only levers that move the residue need WGSL parsed and validated without a
device (naga IR handed to `create_shader_module`, a `wgpu` surface the pin does not expose today) or
a smaller first-frame program set — a separate decision, not a tuning of where the threads start.

**Trigger for removal**: a first frame whose `first_encode_done → first_frame_presented` is back at
or under 25 ms on the OnePlus 9 and the Pixel 5 with the modules created ahead of the install.

**Evidence**: `benchmarks/RESULTS.md` — "Device-open fix re-measure (2026-10-19)" and "Pre-init
shader modules re-measure (2026-10-20)" under the OnePlus 9 and Pixel 5 S7 sections;
`crates/frust-gpu/src/shader.rs` (`ShaderLibrary::module`, created on first use),
`crates/frust-gpu/src/pipeline.rs` (`PipelineCache::warm_up_shaders`),
`crates/frust-engine/src/renderer.rs` (`first_frame_shaders`).

---

### `i18n-interval-hand-rolled` — date and time ranges are joined by hand, with no CLDR interval formats

**Observed**: `fmt::time_range`, `time_range_in_zone`, `date_range` and `datetime_range_in_zone`
are hand-rolled. ICU4X 2.4 (`icu_datetime`, `icu_experimental`, `icu_time`) has no interval
formatter and its baked data has no interval key, so each end is formatted separately and the
two are joined with a per-language separator (ja U+FF5E, zh U+2013, otherwise ` – `). In
12-hour locales a day period shared by both ends is collapsed onto the last end by string
post-processing. There is no month or year elision (`Mar 28, 2024 – Apr 2, 2024`), CLDR
`intervalFormats` are not consulted, and a reversed range is joined in the order given.

**Applies to**: every platform; languages whose CLDR interval pattern differs from
"start separator end" render in the simple joined form.

**Why accepted**: there is no upstream API to delegate to; the join covers booking windows and
slot pills, and the behavior is deterministic and pinned by tests.

**Revisit**: when ICU4X ships an interval formatter with baked data.

**Evidence**: (2026-10-23) `plugins/i18n/src/fmt/interval.rs`; the `formatting_matrix` range rows.

---

### `i18n-tzdb-bundled-size` — `frust-i18n` bundles the jiff time-zone database

**Observed**: `jiff` is pinned with `tzdb-bundle-always` so instant-to-civil conversion works on
Android and iOS, which expose no system zoneinfo. Measured: `frust-i18n`'s Android release rlib
grew from 1439872 to 1440312 bytes (+440 B), and the bundled database is the 599954-byte
`libjiff_tzdb` intermediate rlib. The final app-size delta is unmeasured.

**Applies to**: an app using `frust-i18n` zoned formatting on any platform.

**Why accepted**: correctness of DST and zone offsets on every platform outweighs an
unmeasured, linker-trimmed size cost.

**Trigger for removal**: the platform tzdb (`tzdb-bundle-platform`) proven reliable on Android
and iOS.

**Evidence**: (2026-10-23) `plugins/i18n/Cargo.toml`'s `jiff` pin; the rlib sizes above.

---

### `material-strings-machine-drafted-locales` — the non-English Material string tables are machine-drafted

**Observed**: the th, vi, ja, zh-Hans, es, de and fr `MaterialStrings` tables in
`plugins/material-i18n/src/tables` are machine-drafted and pending native review; only English
is reviewed. Month and weekday names come from CLDR; the other strings are hand-written.

**Applies to**: apps shipping those languages through `frust-material-i18n`.

**Why accepted**: a machine draft is better than English fallback and the tables are plain
data, replaceable without an API change.

**Evidence**: (2026-10-23) the native-review table in `plugins/material-i18n/README.md`.

---

### `material-date-picker-strings-static-str` — a runtime date-picker table is leaked once per language

**Observed**: `DatePickerStrings` is `Copy` with `&'static str` fields, so a runtime locale
table is converted once through `DatePickerStringsOwned` and leaked, once per language table.

**Applies to**: apps using `frust-material-i18n` (or any runtime-built date-picker table).

**Why accepted**: the leak is bounded by the number of supported languages, and keeping the
struct `Copy` and `const`-constructible avoids an API break.

**Evidence**: (2026-10-23) `plugins/material/src/strings.rs`'s module doc ("Picker tables").

---

### `material-open-dialog-ignores-live-locale-switch` — an already-open dialog keeps its labels after a locale switch

**Observed**: `SelectionDialogView::rebuild` (`plugins/material/src/dialog.rs`) diffs
`compose()` against `prev.compose()`, both read under the current theme, so a dialog that is
already open keeps its labels after `I18n::set_locale`; dialogs opened afterwards are correct.
Every other widget (snackbar, badge, FAB menu, chips and the selection app bar's count label
included) resolves its strings from the provided theme when its view is created, so
`provide_localized_strings` alone relabels it on the switch's rebuild.

**Applies to**: a dialog open at the moment the app language changes.

**Why accepted**: a language switch with a modal dialog open is rare, and closing and reopening
is correct.

**Evidence**: (2026-10-23) `plugins/material/src/dialog.rs`'s `SelectionDialogView` `rebuild`.

---

### `text-ellipsis-cursive-truncation` — a native ellipsis cut in a cursive script keeps the joining form

**Observed**: `TextOverflow::Ellipsis` cuts the last visible line at a grapheme boundary that is also a
cluster boundary and does not reshape (`frust-text`'s `layout/truncate.rs`), so the last kept glyph keeps
its contextual (joining) form and its kerning against the dropped neighbour. In a cursive script (Arabic)
a cut inside a word shows a medial form before the ellipsis.

**Applies to**: capped text (`.max_lines` with an ellipsis) whose cut falls inside a cursive-script word.

**Why accepted**: `layout/` never reshapes (a width change re-runs line layout on the cached shaping
result), and the cut is part of that pass.

**Trigger for removal**: reshaping the kept prefix of the cut word.

**Evidence**: the module doc of `crates/frust-text/src/layout/truncate.rs`.

---

### `text-faux-oblique-off` — synthetic italic is off by default; opting in draws outlines every frame

**Observed**: an italic request on a face with no italic or slant axis draws upright.
`SceneCompiler::set_synthetic_oblique(true)` opts in, and skewed runs then never enter the glyph atlas or the
resident-run cache: they draw as outlines every frame. Measured on scene S6 with an all-italic corpus
(Apple M4), submit time rose from 0.38 ms to 1.88 ms per frame. `EngineRenderer::set_synthetic_oblique`
forwards the same switch on the renderer.

**Applies to**: any target drawing italic text from a face without a real italic or slant axis.

**Workaround**: use a face with a real italic or a slant axis, which stays on the atlas route;
`CompiledFrame::synthetic_oblique_runs` counts the skewed runs when the switch is on.

**Evidence**: Apple M4 S6 all-italic measurement; Pixel 5 faux-oblique probe rows leaned with the switch
on; `crates/frust-engine/src/compile/mod.rs`.

---

### `text-hvgl-synthesis-not-applied` — faux bold and faux oblique are not drawn on the hvgl (CoreText) route

**Observed**: embolden and skew synthesis are not applied on the `hvgl` route (Apple targets, faces such as
PingFang whose outlines live only in an `hvgl` table). Weight on a variable hvgl face is carried by the handle's
variation coordinates, so it does change. Only a non-variable hvgl face requested heavier than it is draws at
its real weight.

**Applies to**: Apple targets, `hvgl`-only faces without a weight axis.

**Workaround**: use a face with a weight axis (the variable PingFang faces) or a real bold face; on the
`glifo` route the synthesis is applied.

**Evidence**: `crates/frust-engine/src/text/hvgl/mod.rs` (`glyph_transform`, :243-252, is size / `unitsPerEm` only;
the outline key in `HvglOutlines::outline`, :428-460, carries face, glyph id and variation instance, no synthesis);
on the atlas route `classify_hvgl_run` (`crates/frust-engine/src/compile/mod.rs`) keys with embolden 0 and no skew.

---

### `text-hvgl-identity-checks-are-samples` — the hvgl face checks prove less than they look like

**Observed**: a face opened by PostScript name is checked against the handle's bytes by glyph count, `unitsPerEm`,
`fvar` axis tags and a sample of 16 `cmap` ranges (format 12, else 4). A copy that differs from the installed
face only outside the sampled ranges passes; `GSUB`/`GPOS` and `cmap` format 14 differences are not compared.
The variation read-back proves CoreText recorded the requested instance on the copy, not that its outline code
applied it, and an axis CoreText does not report (and that is not at its default) is drawn unverified: counted in
`SceneCompiler::hvgl_variation_unverified`, warned once per open face, not refused. A copy that silently ignores an
axis without reporting it therefore draws the wrong instance, counted. Only a reported value that contradicts the
request is refused. The read-back is exercised on macOS 27 and iOS 27 only: on an iPhone 13 mini (iOS 27.2, tier
`mobile_large`, 358764edc) PingFang SC drew at three weights, ink 300/400/600 = 24,812 / 34,416 / 48,085, with no
refused, unverified or unavailable warning, so iOS 27's CoreText reports the `wght` axis as applied.

**Applies to**: Apple targets, `hvgl`-only faces (PingFang).

**Workaround**: none needed for the installed system faces; an app that registers its own copy under a system
face's PostScript name should keep glyph ids and `cmap` identical to the installed face.

**Why accepted**: a full `cmap` walk per open is the cost the sample avoids (0.83 us per open in release); the
sample plus the glyph-count, `unitsPerEm` and axis checks catch the realistic substitution, and the known-answer
test (`PingFangSC` Light and Semibold drawn at their coordinates) pins that the instance reaches the outline.

**Evidence**: `crates/frust-engine/src/text/hvgl/face.rs` (`cmap_disagreement`, `CMAP_SAMPLES`);
`crates/frust-engine/src/text/hvgl/outline.rs` (`read_back`, `classify_axis`); `crates/frust-engine/tests/hvgl_provider.rs`;
265aaa068.

---

### `text-hinted-faux-bold-unhintable-face` — a faux-bold request on a face skrifa cannot hint is almost invisible on the hinted tier

**Observed**: on the hinted (desktop) tier a face whose hinting program `skrifa` rejects is drawn by `glifo` at
`unitsPerEm`, while the faux-bold stroke is computed in device pixels, so the stroke comes out about 1/64 of the
intended width. Every bundled face hints at every engine size; the unhintable set is two macOS system faces.

**Applies to**: `/System/Library/Fonts/Supplemental/Brush Script.ttf` and `/System/Library/Fonts/Courier.ttc`
member 2, when drawn hinted with a synthetic bold; the probe's `#[ignore]` macOS test pins the set.

**Workaround**: use a real bold face or a variable weight axis.

**Evidence**: `crates/frust-engine/tests/hint_instance_probe.rs`; the `embolden_basis` rustdoc in
`crates/frust-engine/src/text/atlas_policy.rs`.

---

### `text-font-cache-flush-edges` — the font cache is flushed only at the Hidden edge and on exit

**Observed**: the persisted font-oracle cache is written once per lifecycle `Hidden` edge (iOS `didEnterBackground`,
Android `onStop`, desktop every window minimized or occluded, or suspended) and once at desktop exit. Six edges are not
covered. A crashed writer's temp file from another process lingers up to 24 hours, because the sweep does no PID
liveness check (this process's own are swept after 10 minutes). A desktop window that is merely behind another is
neither minimized nor occluded, so winit reports no hide and nothing flushes until it is. On Wayland the compositor
reports no occlusion and no minimized state, so the desktop hide flush fires only on suspend (the exit flush still
covers quit). iOS flushes inside UIKit's background grace, and the shell's own background flush still runs without a `BackgroundTask` assertion (the shell installs no backend): a first write measured 14.3 ms on
an iPhone 13 mini (iOS 27.2), then 0.1 ms. The web tier has no persistent font cache, so nothing persists across page
loads. Two live copies of one cache file exist only after a stamp or font-directory change mid-process; between them
the last writer wins, so a copy whose write was replaced still reports clean and its answers miss the disk until it
records a new one.

**Applies to**: the temp-file linger and the two-copy case, every platform that persists the cache; the hide edge,
desktop; the grace window, iOS; the web cache, web.

**Workaround**: none needed. A flush that fails leaves the latch unarmed, so the next hide retries, and the drop write
remains; `TextContext::font_cache_stats()` reports `write_failures` and `dirty` for an app that wants to watch.

**Why accepted**: the cache is a warm start only (a load expires answers whose font directories or face files changed,
so no wrong answer is served), and merging two copies' entry sets is not worth the code for a mid-process font install;
a liveness check would add a dependency to reclaim only crashed-writer temp files; flushing on every focus
loss or pause that stays visible costs a write per app switch for no durability gain; the iOS write fits the grace
window with a wide margin; the browser offers no file to persist to.

**Evidence**: `crates/frust-text/src/oracle/cache.rs` (`STALE_TEMP_AFTER`, `FOREIGN_STALE_TEMP_AFTER`);
`crates/frust-shell-desktop/src/app_handler.rs` (`font_cache_transition`, `font_cache_exit`);
`crates/frust-shell-ios/src/ffi_glue.rs` (`frust_did_enter_background`); `crates/frust-shell-web/src/app_handler.rs`
(`has_persistent_font_cache`). Device observations: on iOS one flush per background, none on foreground or for a
top-right Control Center pull (a Notification Center pull does report a background, by iOS design); on a Pixel 5 one
flush per `onStop`, 12.1 ms then 0.1 ms, none for the share sheet, and the cache survives `am force-stop`; on macOS
(M4) minimize 1 flush at 11.2 ms, occlusion 1, Quit 1 exit flush, focus loss 0; in Chrome a tab hide logs no flush.

---

### `connectivity-background-delivery` — no connectivity events arrive while the app is suspended

**Observed**: while the app is suspended (iOS) or frozen or cached (Android), the OS delivers no network callbacks, so a network change made then produces no `ConnectivityEvent`. A loss followed by a regain while suspended can produce no event at all, and the first event after the app returns may already describe a newer network than the last one the app saw.

**Applies to**: iOS and Android.

**Workaround**: register `AppLifecycleListener` `on_restart` and `on_resume` callbacks and re-read `Connectivity::current()` in them, as the crate docs prescribe, instead of trusting the last event.

**Why accepted**: the OS withholds network callbacks from a suspended process, and the crate cannot receive them then.

**Evidence**: `plugins/connectivity/src/lib.rs` (*No events in the background*); `plugins/connectivity/src/apple.rs` (*Suspension, entitlements, linking*).

---

### `connectivity-apple-synthetic-network-id` — two Wi-Fi networks on one interface look like one network

**Observed**: Network.framework exposes no stable network identity, so on iOS and macOS `NetworkId` is synthetic: the preferred interface's index with its interface type. Two Wi-Fi networks on the same interface share an id and a transport, so a move from one access point to another emits no `Available`, unless the path goes unusable in between. `current()` then keeps the previous `NetworkPath`, with only its `expensive` and `constrained` flags refreshed. This is derived from the translation and dedup code and has not been measured on a device.

**Applies to**: iOS and macOS. Android's `NetworkId` is the platform's `networkHandle`, which is per network.

**Workaround**: none in the crate. An app that must notice an access-point change treats the connectivity state as a hint and checks reachability itself.

**Why accepted**: Apple publishes no per-network identifier to key on, and the interface is the only stable handle the framework offers.

**Evidence**: `plugins/connectivity/src/apple.rs` (*Translation*); `plugins/connectivity/src/state.rs` (*Rules*).

---

### `connectivity-vpn-transport-other` — a VPN hides the link beneath it as `Transport::Other`

**Observed**: the Android host checks `TRANSPORT_VPN` before any other transport, so a VPN network is reported as `Transport::Other`, and the Wi-Fi or cellular link beneath it is not reported. On Apple, a path that uses no Wi-Fi, cellular or wired interface (a VPN-only or loopback path) is `Other` too.

**Applies to**: Android, and Apple paths of that kind. Any transport the platform does not name also maps to `Other`.

**Workaround**: `NetworkPath::expensive` and `constrained` still describe the default network. An app that needs the underlying link type under a VPN cannot read it through this crate.

**Why accepted**: the VPN is the default network the OS reports, and the crate reports that network as the platform names it rather than guessing the link beneath it.

**Evidence**: `plugins/connectivity/platform/android/src/main/kotlin/dev/frust/connectivity/FrustConnectivityHost.kt` (the `TRANSPORT_VPN` check); `plugins/connectivity/src/apple.rs` (*Translation*).

---

### `connectivity-android-validated-only` — a network Android has not validated reads as no network

**Observed**: on Android a network is usable only with both `INTERNET` and `VALIDATED`. A Wi-Fi network that is connected but not yet validated, or one that never validates (a captive portal, say), produces no `Available`, and `Connectivity::current()` answers `None`, the same answer as offline.

**Applies to**: Android only. Apple paths count as usable on Network.framework's `satisfied` status instead.

**Workaround**: treat `None` as "no validated network", not "offline", and run the app's own reachability check before a network-dependent step on a captive network.

**Why accepted**: `VALIDATED` is the OS's own verdict on whether the network reaches the internet. The crate reports that verdict rather than running a second probe.

**Evidence**: `plugins/connectivity/src/android.rs` (*What the host reports*); `plugins/connectivity/src/lib.rs` (*What it reports*).

---

### `device-info-desktop-headroom-unavailable` — desktop reports no per-process memory headroom

**Observed**: on macOS, Linux and Windows `DeviceMemory::process_headroom_bytes` is `None`. Only iOS exposes a process memory limit (`os_proc_available_memory`, unavailable on macOS). macOS also reports no `system_available_bytes`, because `host_statistics64` would need hand-written Mach FFI (deferred); Linux (`MemAvailable`) and Windows (`ullAvailPhys`) do report it. On Linux `total_bytes` and `system_available_bytes` come from `/proc/meminfo`, which inside a container, a systemd slice or WSL under a cgroup memory limit reports the host's memory rather than the process's ceiling, so apps there should treat both figures as an upper bound.

**Applies to**: macOS, Linux and Windows.

**Workaround**: use `total_bytes` (all desktop targets) and, on Linux/Windows, `system_available_bytes` as the budget signal.

**Why accepted**: desktop processes have no OS-imposed memory ceiling to report, and the macOS available-memory figure is not worth the unsafe Mach bindings.

**Evidence**: `plugins/device-info/src/apple.rs` (`headroom()` cfg arms); `plugins/device-info/src/linux.rs`; `plugins/device-info/src/windows.rs`.

---

### `device-info-web-unsupported` — `DeviceMemory::query()` returns None on web

**Observed**: on wasm32 `DeviceMemory::query()` always returns `None`.

**Applies to**: web (wasm32) and every target without a backend.

**Workaround**: none; treat `None` as "unknown" and fall back to a conservative budget.

**Why accepted**: `navigator.deviceMemory` is Chromium-only, a coarse power-of-two GiB bucket, and secure-context-gated, so it is not a reliable figure.

**Evidence**: `plugins/device-info/src/noop.rs`.

---

### `app-lifecycle-ui-thread-callbacks` — lifecycle callbacks run on the UI thread, inside the platform callback

**Observed**: every `AppLifecycleListener` callback runs synchronously on the UI thread, inside the shell's platform callback (`onPause`, `applicationDidEnterBackground`, a winit event). A callback that blocks stalls that platform callback and the UI thread with it. A panic is caught and logged, and the remaining callbacks still run.

**Applies to**: every app that registers an `AppLifecycleListener`.

**Workaround**: inside a callback, hand the work off (a channel send, `spawn`) and return. `on_exit_requested` is answered on the spot, so decide there and hand slow work off.

**Why accepted**: the platform callback that delivers a lifecycle step returns to the OS only after the app has run it, and a close or Quit that asked for an exit must be decided before it returns. A deferred answer, as Flutter's `Future` allows, cannot meet that.

**Evidence**: `crates/frust-reactive/src/app_lifecycle.rs` (module docs *Thread contract*; `request_app_exit`).

---

### `app-lifecycle-exit-request-desktop-only` — the exit request is asked only for a desktop close and the macOS and Windows Quit

**Observed**: `on_exit_requested` is asked for a desktop window close (including `WindowCommand::RequestClose`) and for the macOS and Windows menu Quit. It is never asked on Android, iOS or web. It is also not asked for a macOS Dock Quit, logout or shutdown, or a force quit: those reach the platform's application delegate, which does not route through the request. The answer is synchronous, unlike Flutter's `Future`.

**Applies to**: desktop only.

**Workaround**: on desktop, an app-initiated quit sends `WindowCommand::RequestClose`, which takes the close path and asks first. On mobile and web there is no cancelable exit to call.

**Why accepted**: Android and iOS exits are the OS's to decide and are not cancelable (`lifecycle.rs`, `ffi_glue.rs`), and a Dock, logout or force quit belongs to the OS. The request covers the exits frust owns.

**Evidence**: `crates/frust-shell-desktop/src/app_handler.rs` (`resolve_close_action`); `crates/frust-shell-macos/src/menu.rs` (*Quit is the exception*); `crates/frust-shell-windows/src/win32_glue.rs` (`post_close`); `crates/frust-core/src/window_control.rs` (`WindowCommand::RequestClose`).

---

### `app-lifecycle-no-exit-application` — the app cannot end the process itself

**Observed**: Flutter's `exitApplication(AppExitType)` has no frust counterpart. No `frust::` API lets app code end the process or the activity. The only app-initiated close is `WindowCommand::RequestClose` on desktop, which closes a window through the normal close path.

**Applies to**: every platform.

**Workaround**: on desktop, call `push_window_command(WindowCommand::RequestClose)`, which asks `on_exit_requested` first. On mobile, return control to the OS.

**Why accepted**: no mobile shell exposes an app-driven exit, and a desktop exit that skipped the close path would also skip the cancelable answer `on_exit_requested` exists to give.

**Evidence**: `crates/frust/src/lib.rs` (`push_window_command` and `WindowCommand::RequestClose` docs); `crates/frust-reactive/src/app_lifecycle.rs` (module docs).

---

### `app-lifecycle-ios-no-detached` — iOS never reaches `Detached`

**Observed**: the iOS shell pushes `Inactive`, `Paused` and `Resumed`, with `Hidden` synthesized between them. It never pushes `Detached`, so `on_detach` and an `on_state_change` for `Detached` never fire on iOS. The last state an iOS app is told about is `Paused`, the last state iOS announces before it may end the process.

**Applies to**: iOS only. Android pushes `Detached` from `onDestroy`.

**Workaround**: do teardown in `on_pause`, not in `on_detach`.

**Why accepted**: iOS gives the shell no notice before a process is ended, so there is no event to derive `Detached` from.

**Evidence**: `crates/frust-shell-ios/src/ffi_glue.rs` (the `pause` lifecycle mapping doc); `crates/frust-shell-android/src/lifecycle.rs` (`LifecycleFlags::target`); `crates/frust-shell-android/src/jni_glue.rs` (`destroyed` set in the `onDestroy` path).

---

### `gesture-drag-no-cancel-on-removal` — a `GestureDetector` removed mid-drag fires no `cancel`, or mid-press no `on_tap_cancel`

**Observed**: a started vertical, horizontal or pan drag owes its `cancel` callback to the next pass that carries application state after a platform `Cancel`. A detector removed from the tree while its drag is in flight (its subtree changing shape, or its page torn down) never reaches that pass, so its `start` and `update` callbacks are not followed by `end` or `cancel`. The same holds for press state: a detector removed between `on_tap_down` and its release fires no `on_tap_cancel`, so `on_tap_down` is not followed by `on_tap_up` or `on_tap_cancel`.

**Applies to**: apps that pair `on_*_drag_start` with state that `on_*_drag_cancel` or `on_*_drag_end` must release, or `on_tap_down` with state that `on_tap_up` or `on_tap_cancel` must clear (a pressed highlight).

**Workaround**: reset the drag's state from the code path that removes the detector, or treat a drag with no `end` or `cancel` as abandoned when the next one starts; clear the pressed state on unmount.

**Why accepted**: a torn-down detector has no `&mut State` to call back into, and the teardown path never carries one.

**Evidence**: `crates/frust-widgets/src/gesture.rs` (module docs, *Drag recognition* / Cancel).

---

### `gesture-second-contact-needs-a-contact-captor` — a bare `GestureDetector` never hears a second finger

**Observed**: a `GestureDetector` with no contact-capturing widget below it (`pinch_detector`, `pan_zoom`) does not see a second pointer contact, so two resting fingers on it can still long-press. The cancel-on-second-contact rule (`on_tap_cancel`, no long press, tap or double tap) applies only when such a widget is below the detector.

**Applies to**: every platform; the `frust-widgets` `GestureDetector`.

**Why accepted**: the root drops non-claimant contacts unless a widget opts in. The detector cannot safely opt in itself, because an outer opt-in makes it the widget every other contact is delivered to, hiding them from a recogniser below it. `ComponentWidget` forwards `capture_pointer` but not `capture_contacts`, so a hidden recogniser below a component boundary cannot be detected.

**Evidence**: `crates/frust-widgets/src/gesture.rs` tests `a_bare_detector_does_not_hear_a_second_finger`, `a_nested_pinch_keeps_its_second_finger_and_the_press_is_cancelled` and `a_pinch_behind_a_component_boundary_still_cancels_the_press_and_scales`; facade Tester test `a_bare_detector_never_hears_a_second_finger_so_the_press_runs_on` (`crates/frust/src/testing/gesture.rs`). A possible lift, not planned: mirror `contacts_requested` outward in `frust-core` `component.rs` the way `capture_pointer` is.

---

### `memory-pressure-not-reported-on-windows-linux-web` — no memory-pressure event on Windows, Linux or web

**Observed**: `on_memory_pressure` never fires on Windows, Linux or web, since those shells have no source. iOS, Android and macOS report it. The render caches are trimmed only on iOS and Android Critical, and the app is never told about a low-memory condition on the other hosts.

**Applies to**: `frust-shell-windows`, `frust-shell-linux`, `frust-shell-web`, and the shared desktop core.

**Workaround**: an app that must shed memory there trims on its own signal. `push_memory_pressure` is public, so an embedding host with its own platform hook can deliver the event itself (on the UI thread).

**Why accepted**: each needs its own platform signal (Windows `CreateMemoryResourceNotification`, Linux PSI) that no shell wires yet; the event and `push_memory_pressure` seam are ready for it.

**Evidence**: `push_memory_pressure` (`crates/frust-reactive/src/app_lifecycle.rs`) is called only from `crates/frust-shell-ios/src/ffi_glue.rs` (`memory_warning`), `crates/frust-shell-android/src/jni_glue.rs` (`native_on_trim_memory`) and `crates/frust-shell-macos/src/lib.rs` (`pump`); `frust-shell-desktop`, `frust-shell-windows`, `frust-shell-linux` and `frust-shell-web` have no caller.

---

### `text-scale-partial-coverage` — the OS text scale reaches only theme-driven, default-sized text

**Observed**: explicit pixel sizes and catalog constants outside `TypeScale` do not scale: the shadcn label and most shadcn text, Material `list_item` overline and supporting lines, and app bar titles. Material time-picker input labels overlap their digits at 2.0. Desktop and web have no OS source (the scale stays 1.0). iOS native-widgets controls keep Dynamic Type off. Editing one role on a theme read from context makes that whole scaled scale the new authored scale. The 1.5x display-role cap is unit-tested only. A theme push alone does not rebuild the component that calls `provide_localized_strings`; build-time readers below it update on the next rebuild (shells rebuild from the root on Run frames).

**Applies to**: apps on Android and iOS that set a large system font size; apps that edit a theme read from context.

**Workaround**: size text from the theme's `TypeScale` roles instead of pixel constants; edit the type scale on the app's own authored theme (`Theme::authored_type_scale`), not on one read from context.

**Why accepted**: scaling explicit sizes would break layouts that chose them on purpose, and each catalog's constants are its own design decision; the roles that matter most (body, label, title, display, headline) scale and clamp.

**Trigger for removal**: a catalog moving its remaining constants onto `TypeScale` roles, or a desktop OS text-scale source.

**Evidence**: (2026-11-07) `frust-theme` text-scale tests (`crates/frust-theme/src/typography.rs`); device checks on a Xiaomi 12 and an iPhone 13 mini.

---

### `i18n-likely-script-subset` — script-aware negotiation covers only zh and sr

**Observed**: `Locale::likely_script` is a frust-owned subset covering `zh` and `sr` only; other script-split languages negotiate on language and region alone. A `zh-US` tag saved by an earlier version stays English against Simplified-only bundles until the app sets an explicit script.

**Applies to**: apps shipping bundles split by script for a language other than zh or sr.

**Workaround**: name the bundles with a script and set the locale with an explicit script tag.

**Why accepted**: no shipped bundle pair is split by script for another language, and the full CLDR likely-subtags table would be unused data.

**Trigger for removal**: a shipped bundle pair split by script for another language.

**Evidence**: (2026-11-07) `plugins/i18n/src/locale.rs` (`Locale::likely_script`) and `plugins/i18n/src/engine/negotiate.rs`.

---

### `rich-text-no-span-hit-test` — no per-span tap, link or semantics

**Observed**: a `rich_text` is one paragraph with one `Role::Label` semantics node whose value is the concatenated text. A span cannot be tapped, cannot act as a link, and has no semantics node of its own.

**Applies to**: apps that want inline links, tappable mentions or per-span accessibility actions inside a `rich_text`.

**Workaround**: lay the tappable pieces out as separate widgets (`TextView`s in gesture detectors) instead of spans of one paragraph.

**Why accepted**: the layout API takes styled pieces without byte ranges, and a span hit-test and per-span semantics need a position-to-span query that no consumer has required yet.

**Trigger for removal**: a span hit-test API on the rich layout.

**Evidence**: (2026-11-07) `crates/frust-widgets/src/rich_text.rs`.

---

### `permissions-android-asked-flag-local` — Android's "never asked" state is a flag the plugin keeps, not one the system reports

**Observed**: Android has no never-asked state: an unasked permission and one refused for good both read as not granted with no rationale. `frust-permissions` tells `NotDetermined` from `Blocked` with its own flag, a boolean per Android permission in the `frust.permissions` `SharedPreferences` file (key `frust.permissions.asked.<permission>`). Granted and Limited always come from `checkSelfPermission`; the flag is only a never-asked hint. It lives in app data, so clearing the data or reinstalling resets it while the system's own record may differ (a permission refused earlier then reads `NotDetermined` until the next ask). A prompt dismissed without an answer, which Android 11+ does not count, is recognised only by inference: a first denial with no rationale before or after the request, answered after more than 500 ms, leaves the flag unset. Other requesters (another plugin such as `frust-camera`, the app) may not write the flag, so two rules recover from that: a permission that shows a rationale reads `Denied` and gets its flag written; a denial the system answers within 500 ms (`AUTO_DENY_MS`) with no rationale is taken as a system refusal and the flag is written, so the status reads `Blocked` after one request. The 500 ms rule is a heuristic in both directions: a person who dismisses the dialog faster than 500 ms reads `Blocked` although nothing was refused for good, and a system refusal slower than 500 ms (a cold permission-controller start on a slow or loaded device; measured at about 90 ms on a Pixel 5, Android 14) is taken as a dismissal, so the permission keeps reading `NotDetermined`. Media location is excluded from it.

**Applies to**: Android.

**Workaround**: treat `NotDetermined` after a data clear as "ask again"; the prompt then either appears or the system answers at once. An app or plugin that requests a runtime permission itself should write the documented key; the recovery rules cover it when it does not. A `Blocked` that follows a very fast dismissal is cleared from system settings.

**Why accepted**: the platform exposes no query for "has this app ever asked", and the flag is the standard way to tell the two cases apart; a lost flag only makes a refused permission read `NotDetermined` until the next ask; it never grants anything.

**Trigger for removal**: Android reports a never-asked state. Until then, a slower-device gate leg should confirm the 500 ms threshold.

**Evidence**: (2026-11-08) `plugins/permissions/platform/android/src/main/kotlin/dev/frust/permissions/FrustPermissionsHost.kt` (*The asked flag*); device run on a Pixel 5 (Android 14): Denied, then Blocked with no dialog; `pm clear` returns every row to `NotDetermined`; the recovery rules are `currentCode` and `isSystemRefusal` in the same file, with the re-check steps in `docs/PLUGINS_DEVELOPMENT.md` (*Permissions manual test*).

---

### `permissions-request-lost-on-process-death` — a permission prompt answered after the process died is lost

**Observed**: `request` launches the system dialog through the resumed Activity's `activityResultRegistry`, keyed by request id. If the process is killed while the dialog is up, the callback dies with it. A call still waiting in a surviving process gives up after 120 s and re-reads the status; a restarted process has no waiting call, so the answer is applied by the system but never delivered, and the next `status` call reports it. Rotation does not trigger this (the embedding handles `configChanges`, and the answer still arrived in the gate).

**Applies to**: Android.

**Workaround**: re-read `status` on resume, as for any permission that can change while the app is away.

**Why accepted**: a plugin cannot subclass the app's Activity to receive `onRequestPermissionsResult` after recreation, and a status re-read after the fact is complete.

**Trigger for removal**: a persisted request registry that survives process death.

**Evidence**: (2026-11-08) `plugins/permissions/platform/android/src/main/kotlin/dev/frust/permissions/FrustPermissionsHost.kt` (*Delivering the answer*); `plugins/permissions/src/android.rs` (`REQUEST_TIMEOUT`); Pixel 5 device run (rotation with a dialog up delivered the answer). Process death with a dialog up is derived from the code and has not been run on a device.

---

### `permissions-photos-macos-unsupported` — Photos permissions answer `Unsupported` on macOS

**Observed**: on macOS `Permission::Photos` (both access levels) returns `PermissionError::Unsupported` from `status` and `request`; camera and microphone work. `open_settings` works. The plugin's Apple arm reaches `PHPhotoLibrary` only on iOS.

**Applies to**: macOS (Linux, Windows and web answer `Unsupported` for every permission).

**Workaround**: none in the crate. A macOS app that reads photos goes through the system open panel instead.

**Why accepted**: the plugin's Photos support targets mobile; a macOS Photos library needs its own entitlement and sandbox review that no app has asked for.

**Trigger for removal**: a macOS Photos arm with its entitlement and a device gate.

**Evidence**: (2026-11-08) `plugins/permissions/src/apple.rs` (Photos and `MediaLocation` arms on macOS); the macOS host test `photos_and_media_location_are_unsupported_on_macos`; a Mac device run (Photos and Media location rows read `Unsupported`).

---

### `permissions-read-external-storage-no-maxsdk` — the registry declares `READ_EXTERNAL_STORAGE` without `maxSdkVersion`

**Observed**: the `photos` registry feature (and `photo-library`'s `library` feature, which adds the same `READ_MEDIA_*` set) writes `android.permission.READ_EXTERNAL_STORAGE` into the app manifest with no `maxSdkVersion` attribute, because `Contribution::ManifestPermission` carries only the permission name. Android 13+ ignores it for media (the `READ_MEDIA_*` permissions apply), and the plugin requests it only on API 29-32. A store scan of the manifest may still list the broad storage permission. The API 30 path (`READ_EXTERNAL_STORAGE` as the only photo permission) has not been run on a device.

**Applies to**: Android apps that add the `photos` registry feature (`frust-drive`'s `permissions` plugin entry) or `photo-library`'s `library` feature.

**Workaround**: edit the added manifest line to `android:maxSdkVersion="32"`.

**Why accepted**: the contribution model has no attribute slot, and the permission is inert on API 33+.

**Trigger for removal**: `ManifestPermission` gains a `maxSdkVersion` field and the registry sets it, and the API 30 emulator leg passes.

**Evidence**: (2026-11-08) `crates/frust-drive/src/plugin/registry.rs` (`PERMISSIONS_PHOTOS`, `PHOTO_LIBRARY_LIBRARY`); `plugins/permissions/src/codes.rs` (the photos permission lists); the API 30 emulator run is not yet done.

---

### `permissions-media-location-android-only` — `MediaLocation` needs full photo access on Android and is not a permission elsewhere

**Observed**: `Permission::MediaLocation` is the right to read the location already stored inside photos (`ACCESS_MEDIA_LOCATION`), never the device position. Android grants it only alongside full photo access: with "Allow all" it was granted with no dialog of its own; with partial access ("Select photos") or none, the system refuses it silently (the permission screen opens and closes in about 90 ms, nothing is granted), so the plugin reads `Denied` unless Photos read access is fully granted. It never becomes `Blocked`. It cannot be requested usefully until the user grants full access through the Photos request or system settings. On iOS it has no prompt of its own: it mirrors the Photos read-write status, so it reads `NotDetermined` until Photos is asked, then follows that answer. On macOS it answers `Unsupported`.

**Applies to**: all platforms; the refusal under partial access is Android 14+.

**Workaround**: ask for Photos first, offer `open_settings()` when `MediaLocation` reads `Denied` while Photos is not fully granted, and treat the location as absent otherwise.

**Why accepted**: the platform ties the grant to full photo access, and the plugin reports the system's answer rather than hiding it.

**Trigger for removal**: Android grants `ACCESS_MEDIA_LOCATION` under partial access, or the API adds a way to read photo location without it.

**Evidence**: (2026-11-08) `plugins/permissions/src/codes.rs` (`MEDIA_LOCATION`); `plugins/permissions/src/apple.rs` (`media_location`); `plugins/permissions/platform/android/src/main/kotlin/dev/frust/permissions/FrustPermissionsHost.kt` (*Media location*); device runs: Pixel 5 (full access Granted with no dialog; Limited access reports `Denied`); iPhone 13 mini (Granted, no prompt; recorded before `MediaLocation` was made to mirror Photos, not re-checked since); Mac (`Unsupported`).

---

### `permissions-android-status-needs-activity` — Android tells `Denied` from `Blocked` only while an Activity is resumed

**Observed**: the rationale that separates `Denied` from `Blocked` is read against the resumed Activity. With none (the app in the background, between Activities), a permission that was asked and is not granted reads `Denied`, even one that is blocked. `Granted`, `Limited` and `NotDetermined` do not depend on an Activity.

**Applies to**: Android.

**Workaround**: read `status` after the app resumes, and offer `open_settings()` for `Denied` as well when the status was read in the background.

**Why accepted**: the platform exposes the rationale only through an Activity; `Denied` is the safe answer, since a request on a blocked permission returns at once.

**Trigger for removal**: the platform reports a blocked state without an Activity.

**Evidence**: (2026-11-08) `plugins/permissions/platform/android/src/main/kotlin/dev/frust/permissions/FrustPermissionsHost.kt` (*Denied and Blocked need a resumed Activity*, `currentCode`); Pixel 5 device run (Blocked read with the app resumed). The no-Activity read is derived from the code and has not been run on a device.

---

### `testing-text-runs-engine-clips` — `Tester::text_runs` sees only clips a widget declares, and no text inputs

**Observed**: `text_runs` and `assert_no_text_overflow` read clips only from widgets that report `InspectDetail::Clip` (scroll, list and page views, viewports and their slivers, pan/zoom, the navigator). A clip pushed by any other widget (a design-system plugin widget, a rounded clip, a custom widget) is not consulted: a run it hides while inside its ancestors' boxes is not reported, and a run it legitimately keeps outside its own box reads as `outside_parent`. A `hidden` run (scrolled fully out of view) is not checked at all, so whether an over-wide or cut run is caught depends on the scroll position. Under `PanZoomWidget` the visibility judgement assumes an identity transform: inspect bounds ignore pan and zoom. `TextInput` runs are not reported at all.

**Applies to**: `frust::testing::Tester` text-run diagnostics, all hosts.

**Why accepted**: a clip exists only in the compiled scene unless its widget declares it through `inspect_detail`, and the viewports that lay content out past their box are the ones that do; a text input does not report a layout through `inspect_detail`. Both remaining gaps are outside the overflow class the API targets (a label cut or wider than its box).

**Evidence**: `crates/frust/src/testing/text.rs` (module docs), `crates/frust-core/src/tree.rs` (`InspectDetail::Clip`).

---

### `share-files-linux-windows-unsupported` — `Share::files` has no backend on Linux and Windows

**Observed**: `Share::files` and `files_anchored` return `ShareError::Unsupported` on Linux and Windows (and on web and other targets); only Android, iOS and macOS open a share UI for files. Argument validation (`EmptyPayload`, `TooManyFiles`, `InvalidFile`) still runs first.

**Applies to**: Linux, Windows; every file share.

**Workaround**: reveal the file or copy its path yourself; text and URLs still work through `share-desktop-clipboard-fallback`.

**Why accepted**: neither OS has a share sheet worth a per-platform backend, and a file cannot be meaningfully "shared" to a clipboard the way text can.

**Trigger for removal**: a Linux or Windows file-share backend ships.

**Evidence**: (2026-11-08) `plugins/share/src/lib.rs`, `plugins/share/src/files.rs`; `plugins/share/README.md`.

---

### `share-android-files-async-staging` — on Android `Share::files` returns `Ok` when the share is enqueued, not when a chooser shows

**Observed**: `Share::files` on Android validates the paths and returns `Ok(Presented)` without blocking on file size; a background thread then copies each file (a snapshot taken at that moment, never a link) into `cacheDir/frust-share/<uuid>/` and starts the chooser. A staging or launch failure is logged under tag `FrustShare` (error class only) and shows no chooser. A chooser Android refuses because the app moved to the background shows nothing and logs nothing. Staging copies persist up to 24 hours, until a later share prunes them, because a receiver may read the content URI late.

**Applies to**: Android; every `Share::files` / `files_anchored` call.

**Workaround**: none for the missing chooser; an app that needs a delivery signal cannot get one from the platform. Shares started while the app stays in the foreground are unaffected.

**Why accepted**: staging a large file before returning would block the caller for as long as the copy takes, and a receiver needs the copy to outlive the call.

**Trigger for removal**: an outcome callback for file shares, or a staging lifetime tied to receiver completion.

**Evidence**: (2026-11-08) `plugins/share/platform/android/src/main/kotlin/dev/frust/share/FrustShareHost.kt` (`shareFiles`, `stageAndLaunch`, `prune`); device gates on Pixel 5 (Android 14) and Xiaomi 12 passed for the enqueue-and-chooser path.

---

### `photo-library-picks-are-copies` — mobile picks are app-owned copies that grow until cleared

**Observed**: on Android and iOS every `PickedItem` is a copy in `<cache_dir>/frust-photo-library/picked` (`is_copy` is `true`), made eagerly during `pick`. The cache grows with every pick until `PhotoLibrary::clear_pick_cache()`, and a copy has no size bound because the user chose the file in the system picker. After a clear, earlier `PickedItem` paths are invalid. Desktop returns the user's original files (`is_copy` is `false`) and never copies.

**Applies to**: Android and iOS; apps that pick many or large items.

**Workaround**: call `clear_pick_cache()` when picks are no longer needed (it returns `Busy` during a pick).

**Why accepted**: the system pickers hand out transient access, so a path the app can read after the picker closes needs a copy.

**Trigger for removal**: a streaming or reference-based pick API that keeps no copy.

**Evidence**: (2026-11-08) `plugins/photo-library/src/pick_cache.rs`; `plugins/photo-library/src/lib.rs` (`clear_pick_cache`); device gates on iPhone 13 mini (iOS 27.2) and Pixel 5 (Android 14).

---

### `photo-library-picker-no-asset-id-without-library` — picks carry no asset identifier

**Observed**: `PickedItem::asset_id` is `None` on every platform except iOS built with the `library` feature. Without it the iOS picker is created without a photo library, which by design never returns asset identifiers; the Android Photo Picker and the desktop file dialog expose none either.

**Applies to**: Android, desktop, and iOS without the `library` feature; apps that want to match a pick back to a library asset.

**Workaround**: on iOS enable `library` (picks then carry the identifier; resolving it through browsing needs Photos access). None elsewhere.

**Why accepted**: returning identifiers needs photo-library authorization on iOS, which this crate deliberately avoids so a pick needs no permission or usage string.

**Trigger for removal**: Android returns a library asset identifier for a pick.

**Evidence**: (2026-11-08) `plugins/photo-library/src/apple.rs` (module docs, *No permission*); `plugins/photo-library/src/apple_picker.rs` (`initWithPhotoLibrary:` under `library`).

---

### `photo-library-ios-export-wait` — on iOS `pick` returns only after the Photos service exports each original

**Observed**: after the picker closes, `pick` waits until the Photos service has exported every chosen original: seconds for a fresh camera photo, as long as the download for an iCloud-only original. On iOS 27.2 a just-taken photo took about 3 s and an iCloud-only video more than 15 s; screenshots and local videos were instant.

**Applies to**: iOS; picks of fresh or iCloud-only items.

**Workaround**: pass `PickOptions::on_progress` to show loading and a `CancelToken` to let the user abort.

**Why accepted**: the app needs a readable file, and the export is the only way the platform produces one.

**Trigger for removal**: none planned; the wait is the platform's export time.

**Evidence**: (2026-11-08) iPhone 13 mini (iOS 27.2) device gate; `plugins/photo-library/src/apple.rs`.

---

### `photo-library-android-cancel-cannot-dismiss-picker` — cancelling a pick does not close the Android system picker

**Observed**: a `CancelToken` cancelled while the system picker is still open makes `pick` return `PhotoError::Cancelled` at once, but the picker UI stays up until the user closes it; Android has no API to dismiss it. Whatever the user then picks is discarded.

**Also**: right after a `Cancelled`, a new `pick` can briefly return `PhotoError::Busy` while the host copy thread stops (it checks between 64 KiB reads, and a read blocked on a slow provider delays that), and a re-pick made while the old system picker is still open opens a second one on top.

**Applies to**: Android; apps that cancel while the picker is showing.

**Workaround**: none; cancel after the picker has closed (during loading) removes the partial copies and needs no dismissal.

**Why accepted**: `ActivityResultContracts.PickVisualMedia` offers no dismissal call.

**Trigger for removal**: a platform dismissal API for the system picker.

**Evidence**: (2026-11-08) `plugins/photo-library/platform/android/src/main/kotlin/dev/frust/photolibrary/FrustPhotoLibraryHost.kt` (*Cancel and progress*); Pixel 5 (Android 14) device gate.

---

### `photo-library-heic-preview-needs-native-decode` — `frust::decode_image_async` cannot read a picked HEIC original

**Observed**: picked bytes are the untranscoded original, so an iPhone HEIC photo arrives as HEIC. `frust::decode_image_async` decodes PNG and JPEG only and fails on HEIC (and on WebP and GIF). With the `library` feature, native thumbnails show HEIC on both platforms (the Browse grid) and, on iOS, a pick that carries an asset id previews through one.

**Applies to**: apps that pass a picked file's bytes to `decode_image_async`, chiefly HEIC from iOS libraries.

**Workaround**: with the `decode` feature, `PhotoLibrary::decode_file` decodes the picked path natively into RGBA8 (HEIC on iOS, macOS and on Android devices with a HEIF decoder; see `photo-library-decode-unsupported-desktop` and `photo-library-android-heif-device-optional`), ready for `ImageSource::from_rgba8`; or preview through `PhotoLibrary::thumbnail`, send the file as is, or convert it outside frust.

**Why accepted**: `decode_image_async` is the facade's portable PNG/JPEG decoder and gains no native codecs; a transcoding pick would change the bytes the user chose.

**Trigger for removal**: `decode_image_async` reads HEIC.

**Evidence**: (2026-11-11) `plugins/photo-library/src/lib.rs` (`decode_file`); `examples/playground/src/pages/photos.rs` (the Picker preview, which falls back to `decode_file` when `decode_image_async` fails); `crates/frust/src/image_async.rs`. Earlier device gates on iPhone 13 mini and Pixel 5 covered the pre-`decode` behaviour only.

---

### `photo-library-macos-desktop-folder-only` — desktop and the browser have no photo library to browse

**Observed**: with the `library` feature, `albums`, `assets`, `thumbnail` and `manage_limited_selection` answer `PhotoError::Unsupported` on macOS, Linux and Windows (and web); only `pick` works there, through the native file dialog. macOS Photos is not read, even though the machine has a Photos library.

**Applies to**: macOS, Linux, Windows and the browser tier; apps that want an in-app grid of the user's photos.

**Workaround**: use `pick` (the file dialog) on desktop, or list a folder yourself.

**Why accepted**: browsing is a mobile feature of this release; a PhotoKit arm for macOS is separate work.

**Trigger for removal**: a macOS browsing arm ships.

**Evidence**: (2026-11-09) `plugins/photo-library/src/desktop.rs` and `unsupported.rs` (the `library` arms).

---

### `photo-library-limited-alert-plist-manual` — the iOS Limited-access alert key must be added by hand

**Observed**: iOS shows its own prompt to select more photos while access is Limited. Apps that run their own **Manage selection** UI turn it off with the boolean Info.plist key `PHPhotoLibraryPreventAutomaticLimitedAccessAlert`, but the registry's `PlistEntry` can only write a string value, so the `library` feature does not add it.

**Applies to**: iOS apps with the `library` feature that handle Limited access themselves.

**Workaround**: add `<key>PHPhotoLibraryPreventAutomaticLimitedAccessAlert</key><true/>` to the app's `Info.plist`.

**Why accepted**: the contribution model has no boolean iOS plist entry, and the system prompt is a safe default.

**Trigger for removal**: a boolean `PlistEntry` variant exists and the registry sets it.

**Evidence**: (2026-11-09) `crates/frust-drive/src/plugin/registry.rs` (`PHOTO_LIBRARY_LIBRARY`); `plugins/photo-library/src/apple_library.rs` (*Limited selection*).

---

### `photo-library-android-paging-cost` — each Android asset page counts and sorts the whole match

**Observed**: every `assets` (offset) page runs a `MediaStore` query that counts all matching rows and sorts them by a computed key (`DATE_TAKEN` when positive, else `DATE_ADDED`), and `albums` scans every row. API 29 pages by moving a cursor rather than by a query offset. `assets_after` (cursor paging) takes no count and no seek: each page is a keyset query that reads one row past its page, though it still sorts on the same key; offset paging still pays both. The cost grows with library size and has not been measured on very large libraries.

**Applies to**: Android; libraries with tens of thousands of items.

**Workaround**: page with `PhotoLibrary::assets_after` instead of `assets`, or request larger pages less often; load albums once.

**Why accepted**: acceptable for a first release on the devices tried; the sort key keeps the order identical to the reported timestamp.

**Trigger for removal**: a measurement on a very large library, and an indexed or cached path if it is slow.

**Evidence**: (2026-11-09) `plugins/photo-library/platform/android/src/main/kotlin/dev/frust/photolibrary/MediaStoreBrowser.kt` (`SORT_ORDER`, the paged query); Pixel 5 (Android 14) device gate. API 29 paging leg owed.

---

### `photo-library-thumbnail-cache-staleness` — cached thumbnails can outlive access or the asset

**Observed**: `PhotoLibrary::thumbnail` serves a cache hit (48 MiB by default) before any platform call, so it skips the access check. The cache empties itself on `manage_limited_selection` success and on a miss answering `PermissionDenied`. A `Delta` from `PhotoLibrary::changes_since` evicts every size of every id it names (inserted, updated and deleted) and a `FullRescan` clears the cache. An app that never calls `changes_since` still keeps a stale entry for an edited or deleted asset until `clear_thumbnail_cache` or eviction, and a selection changed any other way (a `frust-permissions` request on Android while access is Limited) is not noticed either.

**Applies to**: apps showing thumbnails after the user changed photo access or edited photos outside the app, and that do not apply the change feed.

**Workaround**: call `changes_since` (wake it from `on_library_changed`) and keep its `next` token; or call `PhotoLibrary::clear_thumbnail_cache()` after your own permission request or on app resume, or set the budget to 0.

**Why accepted**: a hit costs no platform call, which keeps a scrolling grid cheap; the cache is evicted by the change feed only when the app asks for it, because the wake carries no data.

**Trigger for removal**: none for the access case; the asset case is handled for apps that call `changes_since`.

**Evidence**: (2026-11-09, revised 2026-11-11) `plugins/photo-library/src/lib.rs` (`thumbnail`, `cached_thumbnail`, `changes_since`); `plugins/photo-library/src/changes.rs` (`changes_since_with` and its tests); `plugins/photo-library/src/thumb_cache.rs`. The eviction is established from code and headless tests. On a Pixel 5 (Android 14) an edited photo's thumbnail showed the new picture after the delta reported it; the iOS refresh has not been run on a device (`photo-library-change-feed-not-device-tested`).

---

### `photo-library-change-feed-full-rescan` — the change feed answers a full rescan where Photos or MediaStore keeps no history

**Observed**: `PhotoLibrary::changes_since` answers `LibraryChanges::FullRescan` (with a fresh `next` token) instead of a `Delta` on iOS 15 (Photos keeps a persistent change history from iOS 16; the token is a fixed placeholder), on Android API 29 (`MediaStore` keeps no generations), when the token expired (iOS) or the change details are unavailable, when the `MediaStore` volume version differs from the token's or the generation is below it (Android), and when the token bytes do not decode (another platform's, an older or newer format, empty or truncated). It is not an error; the app re-reads the library.

**Applies to**: iOS 15 and Android API 29 always; any app that stores a token across long gaps, a database rebuild or a device change.

**Workaround**: treat `FullRescan` as "re-list with `assets` or `assets_after`", store its `next`, and keep the rescan path as cheap as the first launch.

**Why accepted**: the platforms keep no older history to answer from; a full rescan is correct, only slower.

**Trigger for removal**: none; a floor raised to iOS 16 and API 30 would remove two of the cases.

**Evidence**: (2026-11-11) `plugins/photo-library/src/apple_changes.rs`, `plugins/photo-library/src/android_changes.rs` and `plugins/photo-library/src/changes.rs`. Code and headless tests. Device (2026-11-11): on a Pixel 5 (Android 14) a forged token answered a full rescan and a fresh token was stored. The iOS expired or forged token, iOS 15 and API 29 have not run on a device (`photo-library-change-feed-not-device-tested`).

---

### `photo-library-android-change-token-size` — the Android change token grows with the library and each call moves a full snapshot

**Observed**: an Android `ChangeToken` encodes the sorted id sets of the visible images and videos (delta-encoded varints), so it takes about 100 KB per 100,000 assets, and about 200 KB when the ids are very sparse (gaps up to 16,384). Each `changes_since` call also reads every visible row's id, kind and two generations through JNI, about 2.8 MB for a 100,000-asset library, once per call and never kept. Cost and token size grow with library size.

**Applies to**: Android; libraries with tens of thousands of items or more, and an app that persists the token or calls `changes_since` often.

**Workaround**: call `changes_since` off the UI thread in answer to `on_library_changed` rather than on a timer, and store the token in a file or database rather than a preference. The plugin does not coalesce wakes, so the app should coalesce a burst of them into one `changes_since` call; the cost of a call on a large library has not been measured on a device.

**Why accepted**: the id set is how rows that became visible without being added (a published pending item, a restored item, a widened selection) are reported as inserted, and how deletions are found without `MediaStore.DeletedFiles`.

**Trigger for removal**: a device measurement on a large library, and a cheaper snapshot if it is slow.

**Evidence**: (2026-11-11) `plugins/photo-library/src/android_changes.rs` (*The token*; the size is from its host test and arithmetic); `plugins/photo-library/platform/android/src/main/kotlin/dev/frust/photolibrary/MediaStoreChanges.kt`. Code and headless tests. Device (2026-11-11), Pixel 5 (Android 14): the saved token was 73 bytes at 13 assets and 217 bytes at 156 assets. A large library has not been measured, for size or for the per-call snapshot cost.

---

### `photo-library-ios-cursor-pins-snapshot` — an iOS paging cursor session sees one frozen snapshot

**Observed**: PhotoKit has no keyset fetch, so `PhotoLibrary::assets_after` on iOS pins one `PHFetchResult` snapshot for a cursor session, fetched by the first page (`cursor: None`). Inserts and deletes after that first page do not appear until a new session starts from `None`, and an asset deleted since can answer `NotFound` from `thumbnail`. Only the four most recently used sessions keep their snapshot; the cursor of an evicted session answers `PhotoError::InvalidRequest`. Assets that share a creation date keep PhotoKit's order within the snapshot.

**Applies to**: iOS apps that page with `assets_after`, especially several paging sessions at once (for example one per album) or a long-lived list.

**Workaround**: on `InvalidRequest` restart from `None`; combine paging with `changes_since` to pick up later changes; start a new session on `FullRescan`.

**Why accepted**: a position inside a snapshot is the only stable paging PhotoKit allows, and the bound on sessions keeps the held fetch results from growing.

**Trigger for removal**: PhotoKit offering a stable secondary sort or keyset fetch.

**Evidence**: (2026-11-11) `plugins/photo-library/src/apple_library.rs` (*Cursor paging*, `Snapshot`); `plugins/photo-library/src/library.rs` (`SnapshotStore`, `SNAPSHOT_SESSIONS`). Code and headless tests (the store). Device (2026-11-11): an iPhone 13 mini (iOS 27.2) paged 6,155 assets in pages of 60, equal to the library's Recents count, with Load more disabled at the end. A session with an item added part-way has not run on iOS (`photo-library-change-feed-not-device-tested`).

---

### `photo-library-change-feed-not-device-tested` — parts of the change feed and cursor paging have not run on a device

**Observed**: device gates ran on 2026-11-11 on a Pixel 5 (Android 14) and an iPhone 13 mini (iOS 27.2) and passed for the core paths: token save, empty, inserted, updated and deleted deltas on Android, an inserted delta and the wake counter on iOS, a token surviving a relaunch, and cursor paging on both. Still unverified on a device:

- iOS: a forged or expired token answering `FullRescan`; `cloud_identifiers`; anything on iOS 15 (the no-history path). An updated and a deleted delta ran on 2026-11-12 (iPhone 13 mini, iOS 27.2); the delete also reported one updated asset that was not touched, cause not looked into.
- Android: API 29 (always `FullRescan`); the observer's Stop; token size and per-call snapshot cost on a large library (only 156 assets were measured).
- Both: an iOS cursor session with an item added part-way; thumbnail refresh after a delta on iOS.

**Applies to**: iOS apps that use `cloud_identifiers` or run on iOS 15; Android apps on API 29, with a large library, or that rely on the observer's Stop.

**Workaround**: none; verify the listed paths on the target device before depending on them.

**Why accepted**: the remaining gates need hardware or a library not yet on hand; the verified paths have run.

**Trigger for removal**: the device gate records a run of each unverified item above.

**Evidence**: (2026-11-11) the device table in `examples/playground/README.md`; code and headless tests in `plugins/photo-library/src/changes.rs`, `apple_changes.rs`, `android_changes.rs`, `apple_library.rs`, `android_library.rs`.
---

### `photo-library-live-photo-still-only` — exporting a Live Photo returns its still, not the paired video

**Observed**: `export_original` on an iOS Live Photo writes the still image resource only; the paired video resource is never selected, so the exported file is a plain photo with no motion.

**Applies to**: iOS apps exporting Live Photos.

**Workaround**: none in the plugin; the paired video is not reachable through it.

**Why accepted**: one call returns one file, and an `ExportedOriginal` has no slot for a second resource.

**Trigger for removal**: an export result that carries more than one resource.

**Evidence**: (2026-11-09) `plugins/photo-library/src/apple_originals.rs` (*Choosing the resource*).

---

### `photo-library-addonly-no-album` — an iOS save into an album needs read-write access

**Observed**: `save` with `SaveRequest::album` returns `PhotoError::PermissionDenied` under add-only access, because add-only access cannot read or change albums. An album save needs read-write access and `NSPhotoLibraryUsageDescription` (the `library` registry feature writes it); a missing key returns `MissingUsageDescription`. Limited access allows an album save, but the fetch sees only what the user shared, so an existing album of that name may not be found and a new one is created. A bare save needs add-only or read-write access.

**Applies to**: iOS; apps that file saves into an album while holding only add-only access. Android has no such split.

**Workaround**: request read-write access (`permissions` `photos`) and add the `library` feature when saving into an album, or save without an album. iOS Settings lists *Add Photos Only* for an app only if its first Photos request was add-only.

**Why accepted**: the platform offers no album write under add-only authorization.

**Trigger for removal**: iOS lets add-only access change albums.

**Evidence**: (2026-11-09) `plugins/photo-library/src/apple_originals.rs` (*Save*); iPhone SE 2nd gen (iOS 26.7.1) device gate.

---

### `photo-library-android-location-redacted` — stored location survives only some pick and export routes (Android, measured against iOS)

**Observed**: whether an image's stored GPS location survives depends on the route, measured on a Pixel 5 (Android 14) with a camera photo and on an iPhone SE 2nd gen (iOS 26.7.1):

| Route | Location |
|---|---|
| Android browse + `export_original`, Media location granted | kept, byte-identical to the camera file |
| Android browse + `export_original`, Media location not granted | latitude, longitude and GPS date zeroed in place, same file size |
| Android Photo Picker (`pick`), even with Media location granted | latitude, longitude and GPS date zeroed |
| iOS `PHPicker` (`pick`), default options | kept |
| Android `scan_folder` / `FolderEntry` read through a Storage Access Framework tree | not measured |

Media location has no user-facing switch on Android 14: it is granted along with Photos.

**Applies to**: Android apps that need stored location from a pick, or from an export without Media location. iOS `PHPicker` keeps location with default options and is listed only for contrast.

**Workaround**: browse and export (`library`), with `permissions`' `media-location` feature requested and granted; that is the route known to keep location. GPS through a folder-grant read on Android has not been measured, so do not rely on it. An app can tell its user that picked photos lose their location on Android and that the library route keeps it.

**Why accepted**: Android redacts the location of Photo Picker items and of items opened without `ACCESS_MEDIA_LOCATION`; the export host falls back to the plain URI instead of failing when redaction is refused.

**Trigger for removal**: the Photo Picker stops redacting location, or the plugin gains a location-preserving pick.

**Evidence**: (2026-11-09) Pixel 5 (Android 14) and iPhone SE 2nd gen (iOS 26.7.1) device gates; `plugins/photo-library/platform/android/src/main/kotlin/dev/frust/photolibrary/FrustPhotoLibraryHost.kt` (*Exports and saves*). A manual denied test needs `adb shell pm revoke <app> android.permission.ACCESS_MEDIA_LOCATION`.

---

### `photo-library-android-no-rendition-split` — Android export ignores the version and network options

**Observed**: on Android `AssetVersion::Current` and `ExportOptions::allow_network` are ignored: `MediaStore` has no edited-rendition split and its files are local. Original and Current give identical bytes, and an export never fails with `NetworkRequired`.

**Applies to**: Android apps that expect an edited rendition or a cloud-download refusal.

**Workaround**: none; branch on the platform when the distinction matters.

**Why accepted**: `MediaStore` exposes one file per row.

**Trigger for removal**: a platform API for edited renditions on Android.

**Evidence**: (2026-11-09) `plugins/photo-library/src/android_originals.rs` (module docs); Pixel 5 (Android 14) device gate. A DNG export on an iPhone SE 2nd gen (iOS 26.7.1) produced a real DNG 1.4, size-identical to the source; an Android DNG export is not recorded.

---

### `photo-library-android-save-no-deadline` — one stuck Android save blocks later saves and its caller

**Observed**: saves run one at a time on a single executor and the Rust wait has no deadline, because the host answers every request exactly once. A save stuck in a slow or hung provider write blocks the saves queued behind it and the thread that called `save`; there is no cancel and no deadline.

**Applies to**: Android; apps that save large videos or save while storage is stalled.

**Workaround**: call `save` from a task the UI can abandon, and do not queue saves behind a large one.

**Why accepted**: a `MediaStore` row cannot be half-published safely, and a deadline would have to abandon a write the provider may still complete.

**Trigger for removal**: a cancel or deadline for `save`.

**Evidence**: (2026-11-09) `plugins/photo-library/platform/android/src/main/kotlin/dev/frust/photolibrary/MediaStoreWriter.kt`; `plugins/photo-library/src/android_originals.rs` (module docs).

---

### `photo-library-android-serial-exports` — Android exports run one at a time

**Observed**: all Android exports share one executor, so a slow export (a large video) holds the exports queued behind it. A cancelled queued export is answered at once without opening anything.

**Applies to**: Android; apps that export several assets in parallel.

**Workaround**: export the small assets first, or cancel the slow one.

**Why accepted**: one worker bounds memory and provider load; no measured need for more.

**Trigger for removal**: a measured case that needs parallel exports.

**Evidence**: (2026-11-09) `plugins/photo-library/platform/android/src/main/kotlin/dev/frust/photolibrary/FrustPhotoLibraryHost.kt` (`exportExecutor`).

---

### `photo-library-originals-no-cleanup` — exported originals accumulate until the app removes them

**Observed**: `export_original` writes into `<cache_dir>/frust-photo-library/originals` by default. `clear_pick_cache` does not touch it and the plugin has no cleanup API, so exports grow the directory until the app deletes them. On iOS a process kill during an export can also leave a hidden `.<name>.part` file in the destination. An Android save interrupted before publishing leaves a pending `MediaStore` row that MediaStore removes after its `DATE_EXPIRES`; the plugin runs no sweep.

**Applies to**: apps exporting many or large originals.

**Workaround**: delete exported files (and stale `.part` files) in the destination yourself, or pass a `dest_dir` you manage.

**Why accepted**: the app decides how long an exported file must live, and a sweep could delete files it did not create.

**Trigger for removal**: a cleanup API for exported originals.

**Evidence**: (2026-11-09) `plugins/photo-library/src/originals.rs` (`ExportOptions::dest_dir`); `plugins/photo-library/src/apple_originals.rs` (*Cancel and cleanup*).

---

### `photo-library-decode-unsupported-desktop` — native image decode answers `Unsupported` on Linux and Windows

**Observed**: with the `decode` feature, `PhotoLibrary::decode_file` and `decode_asset` return `PhotoError::Unsupported` on Linux, Windows and web; only iOS, macOS and Android decode. The `decode_asset` call also needs the `library` feature, and its export step answers `Unsupported` on desktop.

**Applies to**: Linux, Windows and the browser tier; apps that decode HEIC or other non-PNG/JPEG picks natively.

**Workaround**: on those targets decode PNG and JPEG with `frust::decode_image_async`, or convert the file outside frust.

**Why accepted**: decode is a mobile-first feature and the plugin carries no desktop codec; each arm wraps a system decoder.

**Trigger for removal**: a Linux or Windows decode arm ships.

**Evidence**: (2026-11-11) `plugins/photo-library/src/desktop.rs` and `unsupported.rs` (the `decode_file` arms). Not run on Linux or Windows.

---

### `photo-library-android-heif-device-optional` — Android HEIF decoding depends on the device

**Observed**: the Android arm decodes through `ImageDecoder`. A format with no installed decoder returns `PhotoError::Unsupported`, and HEIF additionally needs an HEVC decoder in `MediaCodecList`. HEIF decoding is optional on a device, so the same HEIC file can decode on one phone and answer `Unsupported` on another.

**Applies to**: Android devices without a HEIF or HEVC decoder; apps decoding HEIC from iOS libraries.

**Workaround**: treat `Unsupported` as "show a placeholder or send the file as is".

**Why accepted**: the platform decides which codecs exist, and the plugin bundles none.

**Trigger for removal**: a bundled HEIF decoder, or a minimum Android version that guarantees one.

**Evidence**: (2026-11-11) `plugins/photo-library/src/android_decode.rs` (*Errors*); `plugins/photo-library/platform/android/src/main/kotlin/dev/frust/photolibrary/ImageDecodeHost.kt` (*Missing decoders*). No Android device has run this feature yet.

---

### `photo-library-decode-not-interruptible` — a started decode cannot be cancelled mid-flight

**Observed**: a `CancelToken` on `DecodeOptions` is checked before the call and between stages, but the platform decode itself cannot be interrupted. On iOS and macOS a cancel that lands during the ImageIO decode takes effect when it returns. On Android the token reaches the host, which checks between the decode and the pixel copy, so the call still waits for the running `ImageDecoder` decode.

**Applies to**: apps that cancel a decode of a large image (for example when the user scrolls away).

**Workaround**: cap the work with `DecodeOptions::max_dimension`, and drop the result when the request is stale.

**Why accepted**: neither ImageIO nor `ImageDecoder` offers an abort for a decode in progress.

**Trigger for removal**: a platform decode API with an abort.

**Evidence**: (2026-11-11) `plugins/photo-library/src/apple_decode.rs` (*Cancel*); `plugins/photo-library/src/android_decode.rs` (*Cancel*).

---

### `photo-library-apple-decode-truncated-pixels` — on iOS and macOS a file cut inside its pixel data decodes with the missing part blank

**Observed**: ImageIO reports a file whose pixel data stops early as a complete source, so `decode_file` returns an image with the missing part left blank instead of an error. Only a file cut inside its headers, or one with no image, is `PhotoError::Platform`. The behaviour is documented for JPEG, PNG and HEIC in the decoder's module docs.

**Applies to**: iOS and macOS; apps that decode partly downloaded or damaged files.

**Workaround**: check the file's length or checksum against its source before decoding.

**Why accepted**: telling a short file apart from a complete one needs a per-format container check that the plugin does not do.

**Trigger for removal**: a container-length check in the Apple arm.

**Evidence**: (2026-11-11) `plugins/photo-library/src/apple_decode.rs` (module docs, *Errors*); `plugins/photo-library/src/lib.rs` (`decode_file` rustdoc).

---

### `photo-library-no-local-availability` — an asset row does not say whether its original is on the device

**Observed**: `Asset` has no cloud or locally-available field, on either platform. An iCloud-only original on iOS looks like any other row; it surfaces as `PhotoError::NetworkRequired` only when `asset_metadata`, `export_original` or `decode_asset` is asked to read it with `allow_network` off.

**Applies to**: iOS; apps that want to badge or skip cloud-only photos before the user opens them.

**Workaround**: treat `NetworkRequired` as the answer and show the asset as not downloaded.

**Why accepted**: PhotoKit offers no per-asset flag for this without a resource lookup per row, the cost `AssetQuery::with_details` documents for iOS pages.

**Trigger for removal**: a cheap per-row availability signal from Photos.

**Evidence**: (2026-11-11) `plugins/photo-library/src/library.rs` (`Asset`); `plugins/photo-library/src/apple_metadata.rs` (*Network*); `plugins/photo-library/src/apple_library.rs` (`asset_of`). Not run on a device.

---

### `photo-library-ios-metadata-reads-original` — on iOS `asset_metadata` reads the whole local original

**Observed**: Photos has no call that returns an asset's EXIF from its header alone, so `asset_metadata` reads the original photo resource in full into one in-memory buffer (never to a file) and ImageIO reads that buffer in place, with no second copy. Peak memory is that one buffer plus growth slack (its capacity is at most twice the bytes received while it grows). Cost grows with the original's size, and an iCloud-only original is downloaded when `MetadataOptions::allow_network` is set. A read that receives neither data nor its completion for the inactivity interval (`INACTIVITY_TIMEOUT`, 30 s) is cancelled and returns a `PhotoError::Platform` timeout, with or without a cancel token; each chunk restarts the interval. An original above `MAX_ORIGINAL_BYTES` (256 MiB) is refused with `PhotoError::InvalidRequest`. A cancel through the token stops the transfer sooner.

**Applies to**: iOS; apps that read metadata for many assets, or for very large originals, in a row.

**Workaround**: read metadata for the asset the user opened rather than for a whole page, one original at a time, keep `allow_network` off unless the user asked, and cancel through `MetadataOptions::cancel` when the view goes away.

**Why accepted**: no header-only path exists in Photos or ImageIO for a library asset; Android reads through an opened stream with `ExifInterface`. The ceiling and deadline bound the cost; iOS can still end the app under memory pressure while a large buffer is held.

**Trigger for removal**: a Photos API that returns EXIF without the full resource.

**Evidence**: (2026-11-11) `plugins/photo-library/src/apple_metadata.rs` (*Byte ceiling and memory*, *Inactivity deadline and cancel*, `INACTIVITY_TIMEOUT`, `MAX_ORIGINAL_BYTES`); `plugins/photo-library/src/apple_decode.rs` (`exif_from_data`); `plugins/photo-library/src/lib.rs` (`asset_metadata` rustdoc). Memory and time not measured on a device.

---

### `photo-library-android-metadata-not-cancellable` — an Android `asset_metadata` cancel does not interrupt the `ExifInterface` read

**Observed**: the cancel token is checked before the Kotlin host call and once after it, not during. `AssetMetadataHost.read` opens the stream and runs `ExifInterface` to the end, so a slow provider stream is read in full; if the token was cancelled meanwhile, the result is discarded and the call returns `PhotoError::Cancelled`.

**Applies to**: Android; apps that cancel metadata reads as views go away while the provider is slow (for example a cloud-backed `MediaStore` item).

**Workaround**: none needed for correctness; the wasted work is bounded by one metadata read per such cancel, and the caller's thread is held until it ends.

**Why accepted**: `ExifInterface` over a stream exposes no interruption point, and the result is correct either way.

**Evidence**: (2026-11-11) `plugins/photo-library/src/android_metadata.rs` (`asset_metadata`); `plugins/photo-library/src/android.rs` (`asset_metadata` arm); `plugins/photo-library/platform/android/src/main/kotlin/dev/frust/photolibrary/AssetMetadataHost.kt` (`read`). Code paths only; not run on a device.

---

### `photo-library-android-asset-fields` — `is_favorite` is unknown below API 30 and `burst_id` is never set on Android

**Observed**: `Asset::is_favorite` is `None` below Android API 30, where `MediaStore` has no favourite column, and `Asset::burst_id` is always `None` on Android, which has no burst identifier. iOS fills both.

**Applies to**: Android; apps that group bursts or show a favourites filter.

**Workaround**: treat `None` as unknown, never as false; group bursts on iOS only.

**Why accepted**: the platform does not report either value.

**Trigger for removal**: a `MediaStore` burst identifier, or dropping API 29 support.

**Evidence**: (2026-11-11) `plugins/photo-library/src/android_library.rs` (`assets_from`); `plugins/photo-library/src/library.rs` (`favorite_from_code`); `plugins/photo-library/platform/android/src/main/kotlin/dev/frust/photolibrary/MediaStoreBrowser.kt` (`FAVORITE_UNKNOWN`). Not run on a device.

---

### `photo-library-content-type-platform-shaped` — `Asset::content_type` is a MIME type on Android and a type identifier on iOS

**Observed**: with `AssetQuery::with_details`, `content_type` is a MIME type such as `image/jpeg` on Android and a uniform type identifier such as `public.heic` on iOS. The raw field is not interchangeable between the two; `Asset::mime_type()` converts through a fixed table and gives `None` for an identifier it does not list. `file_name` is the platform's original name and is untrusted.

**Applies to**: apps that branch on the type of an asset on both platforms.

**Workaround**: call `Asset::mime_type()`, which returns a MIME type on both phones (an iOS identifier outside its fixed table gives `None`, as does a missing `content_type`), or classify by `Asset::kind` and the file extension.

**Why accepted**: each platform's own identifier is lossless, so the raw field stays as is; the mapping table is a convenience that answers `None` rather than guess.

**Evidence**: (2026-11-11) `plugins/photo-library/src/library.rs` (`Asset::content_type`, `Asset::mime_type`); `plugins/photo-library/src/apple_originals.rs` (`primary_names`); `plugins/photo-library/src/android_library.rs` (`assets_from`). Not run on a device.

---

### `photo-library-android-thumbnail-early-cancel` — an Android thumbnail cancel that lands before the host starts the load does not stop it

**Observed**: `thumbnail_shared` cancels an Android load through a `CancellationSignal` the Kotlin host registers under the request id. A cancel that arrives before the host has registered the signal finds nothing to fire, so that one load runs to its end and its pixels are dropped. The call still returns `PhotoError::Cancelled` (the token is checked on both sides of the host call), and nothing is cached. A cancel during the load stops it only where `MediaStore` honours the signal.

**Applies to**: Android; apps that cancel thumbnail loads as cells leave the screen, in a fast fling.

**Workaround**: none needed for correctness; the wasted work is bounded by one thumbnail per such cancel.

**Why accepted**: the signal exists only once the host call is under way, and the result is correct either way.

**Evidence**: (2026-11-11) `plugins/photo-library/src/android_library.rs` (module docs, `thumbnail`, `cancel_thumbnail`); `plugins/photo-library/platform/android/src/main/kotlin/dev/frust/photolibrary/MediaStoreBrowser.kt` (`cancelThumbnail`). Not run on a device.

---

### `time-no-firing-while-backgrounded` — a `frust::time::Timer` gets no regular ticks while the app is backgrounded (Android, iOS)

**Observed**: Android removes its frame callback at `onPause` and pumps nothing at `onPause`/`onStop`; iOS
pauses its display link at `willResignActive` and pumps the local queue at `didEnterBackground` and on
memory warnings; Android also pumps in its trim-memory callbacks. So there are no regular ticks while
backgrounded: a due `Timer` normally runs at the first tick after resume but may run earlier in one of
those pumps; do not rely on either. `flush_on_background` runs the callback synchronously at the first
of the app going Resumed to Inactive or Inactive to Hidden (a timer armed while already Inactive runs at
the Hidden edge: Android `onStop`, iOS `didEnterBackground`). It therefore also fires on focus loss with
no foreground exit: losing window focus on desktop and web, the notification shade, a system dialog or
the unfocused half of split-screen on Android, Control Center or a call on iOS. It is one-shot and is
consumed, so do not flag a timer whose delay must be honoured while the app stays visible. Only
synchronous work in the callback is guaranteed to complete before suspension, work it spawns is not,
and only a `BackgroundTask` with the plugin installed extends it (best effort on Android). On desktop a window that lost focus keeps running, so a
plain timer fires on time.

**Applies to**: Android, iOS.

**Why accepted**: the shells' wakers are no-ops by design because the display tick already pumps;
background execution belongs to OS work managers.

**Evidence**: (2026-11-09) device gate on a Pixel 5 and an iPhone 13 mini: a plain 10 s timer stayed
silent in the background and fired on resume (16744 ms on the Pixel 5, 17557 ms on the iPhone); a flagged
timer's marker file written at backgrounding survived killing the process about 2 s later on both. On
desktop a plain timer fired on time (10001 ms measured). `examples/playground/src/pages/network.rs`
(Timers block).

---

### `background-task-no-op-platforms` — `BackgroundTask` protects work only with the plugin, on iOS and Android

**Observed**: without the `frust-background-task` plugin installed, `frust::BackgroundTask` is a no-op on every platform
that reports itself granted and unexpired, so a write begun when the app leaves the foreground races process death.
With the plugin it is real on iOS and Android and still a no-op on desktop and web (`install()` returns `false`);
`spawn_blocking` panics on web, so the flush pattern does not apply there. On iOS Apple documents no numeric budget
(about 30 s per Apple DTS), and the system may revoke the assertion early; `expired()` reports that loss, and also a refusal.
Android limits are in `background-task-android-refusal-cases`.

**Applies to**: all platforms without the plugin; desktop, web (no protection even with it); iOS (unspecified, revocable budget).

**Workaround**: install the plugin, keep the guarded write short, write it atomically (`frust::paths`' atomic write), and
check `expired()` when the work can be abandoned or retried.

**Why accepted**: only iOS and Android offer an in-process mechanism of this kind; background execution elsewhere belongs
to OS work managers.

**Evidence**: (2026-11-09) device gate on an iPhone SE and a Pixel 5: a 200 ms database write begun from a flush
callback finished and survived killing the process about 5 s after backgrounding, on the iPhone with the guard and with
no `task expired`. A control without the guard also survived, because a write this short fits the normal suspension
delay, so the gate shows the guard works rather than that this write needed it. (2026-11-10) iPhone SE with the plugin: a
40 s guarded write begun at Home read `task expired` and finished only when the app resumed at about 48 s, so the budget
ended before 40 s; the unguarded twin also finished only on resume. `examples/playground/src/pages/network.rs` (Timers
block); `plugins/background-task`.

---

### `background-task-android-refusal-cases` — Android refuses or limits the `BackgroundTask` foreground service

**Observed**: the Android backend is a `shortService` foreground service. A flush-on-background callback normally runs at
the Inactive edge (`onPause`, app still visible), where a start was allowed. A timer armed while already Inactive, or
armed by an Inactive flush, runs at the Hidden edge (`onStop`); a start there right after leaving the foreground was also
allowed, under the system's `ACTIVITY_VISIBILITY_GRACE_PERIOD`. Later than that a start is expected to be refused on
targetSdk 31+ (not exercised); `begin` then reports `is_granted() == false`, never throws, and the work proceeds
unprotected. API 29-33 have no system `onTimeout`, so only the plugin's watchdog (about
150 s) ends the service there. On Android 12+ a short guard normally shows no notification (display may be deferred up to
10 s, not guaranteed); on API 29-30 a brief one may flash. Apps targeting Android 14+ must declare the foreground service
type in the Play Console.

**Applies to**: Android (start refusal on targetSdk 31+; watchdog-only on API 29-33; Play declaration on targetSdk 34+).

**Workaround**: begin the guard while the app is still visible (the Resumed-to-Inactive edge), check `is_granted()` and
`expired()`, and keep the guarded work short and atomic.

**Why accepted**: the restrictions are platform policy; the plugin degrades to the no-op guard rather than failing the app.

**Evidence**: (2026-11-10) Pixel 5, Android 14, targetSdk 36, debug and profile builds. A 40 s write begun in `onPause`
was granted and finished with the service foreground (`types=0x800`). A begin in `onStop` was granted. Twenty 25 ms
save-shaped legs plus twenty rapid ones and ten shade pulls were all granted, with no `ForegroundServiceDidNotStartInTime`,
`RemoteServiceException` or ANR. `begin` took p50 3.6 ms, max 17.1 ms in profile. A 200 s write expired at 150.04 s, the
service stopped, no ANR. `am force-stop`, `cmd activity stop-app` and `kill -9` during an in-transaction hold left
`integrity_check` ok and no row. Unguarded, 20 of 20 25 ms writes and a 40 s write also landed on this phone, so the sample
shows the guard works, not that it was needed. Notification display was not observed: the test app does not declare
`POST_NOTIFICATIONS`, so none can appear on API 33+. API 29-33 and a start after the grace period were not exercised.
`plugins/background-task`.

---

### `list-view-leading-buffer-edit-not-anchored` — a keyed `ListView` does not correct rows edited just above the viewport

**Observed**: a keyed exact-extent `ListView` keeps its anchor row in place when rows change above the viewport, but rows inserted into the gap between the window's first buffered row (the leading buffer is 2 rows) and the viewport-top row, or the one row in that gap removed, go uncorrected. The scroll offset stays put and the content moves by their summed extent: up on removal, down on insert, however many rows were inserted. Removing the row that holds the top edge itself is likewise uncorrected: the next row takes its on-screen place. An edit at the window's first row or anywhere above it is corrected. The same probe serves the estimated keyed mode, so it is expected there too, but it was not observed there.

**Applies to**: keyed `ListView`s (`builder_keyed`); for example a message deleted just off-screen.

**Workaround**: none in the framework. An app can compensate with `ScrollController::jump_to` by the summed extent of the rows edited.

**Why accepted**: it predates the exact-extent mode, the anchor probe only confirms rows by the first surviving window row (it cannot see a row it never materialized), and changing the probe affects all three extent modes. A removal is bounded to one row's extent, but a batch insert just above the screen moves the content by the whole batch.

**Evidence**: originates from a phase review finding of the exact-extent `ListView` work (2026-11-11); established from the code path and headless tests, not from a device observation: `crates/frust-widgets/src/list_view.rs` (`apply_exact_anchor_shift`, `anchor_shift_items`) and `crates/frust-widgets/tests/list_item_extent.rs` (`a_single_row_edit_inside_the_leading_buffer_is_not_corrected`, `two_rows_inserted_in_the_leading_gap_move_the_content_by_their_sum`, `removing_the_row_holding_the_top_edge_falls_back_to_a_row_above`, `editing_the_windows_first_row_is_corrected`).

---

### `a11y-wrapper-phone-exposure-unverified` — the `semantics(child)` wrapper is checked in the phones' accessibility trees only; what VoiceOver and TalkBack speak is not observed

**Observed**: the wrapper's nodes reach the platform accessibility tree on both phones. A `uiautomator` dump on Android and an XCUITest accessibility snapshot on iOS of the playground's A11y page each show one element for a heading, a labelled image, a merged cell (label and value), a button tile and a labelled group with its two children, and none for a hidden subtree; on iOS an accessibility tap on the tile runs its handler. Not observed on either phone: what the screen reader speaks, the hint, and the heading trait (neither tool reports them); on Android, activating a tappable wrapper through the accessibility action.

**Applies to**: apps that rely on `semantics(child)`, `TextView::heading` or `ImageView::label` for screen-reader users on Android and iOS.

**Workaround**: check the screen with VoiceOver and TalkBack before relying on the heading level or the hint; prefer a widget's own semantics where it has one.

**Why accepted**: the trees are verified on both phones; a spoken pass has not been recorded yet, and the entry stays until it is.

**Trigger for removal**: a recorded VoiceOver pass and a TalkBack spoken pass over a screen using the wrapper.

**Evidence**: (2026-11-12) `uiautomator` dumps on a Pixel 4a (Android 16) and a OnePlus 9 (Android 15) and an XCUITest snapshot on an iPhone 13 mini (iOS 27.2), recorded in `examples/playground/README.md`; the headless tests of `crates/frust-widgets/src/semantics.rs`.

---

### `a11y-click-is-positional` — an accessibility tap action lands at the centre of the node

**Observed**: a click action from a screen reader (an activate on TalkBack or VoiceOver) is delivered as a pointer tap at the centre of the node's bounds, not as a call on the widget that owns the action. A tappable child that does not cover the node's centre, or a node whose centre is covered by another view, receives no tap.

**Applies to**: nodes made `tappable(true)` or `button()` on the `semantics(child)` wrapper, and any node with a click action.

**Workaround**: make the tappable child cover the wrapper's centre, and keep the centre of a tappable node unobstructed.

**Why accepted**: actions are delivered through the same hit-testing path as a finger, which keeps one dispatch route; a direct call into the widget would need a per-widget action contract.

**Trigger for removal**: a per-widget action contract that does not go through hit testing.

**Evidence**: `crates/frust-core/src/app/semantics.rs` (click delivery) and `crates/frust-widgets/src/semantics.rs` (`tappable`). Code and headless tests; not run on a device.

---
