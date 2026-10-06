# Frust - Known Limitations

A register of accepted, known limitations an app author can actually hit —
not a bug list and not a TODO list. Open bugs and unshipped fixes are tracked
separately; this doc is for a degrade that is **measured, understood, and
deliberately shipped anyway** (accepted by Ed, or blocked on a documented
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
cupid.

**Why not fixed**: the shape-aware sync tail's regime detector keys off an
acquire-wait EWMA that cannot rise until the GPU queue actually builds under
fresh load — so shortening the confirm window or pre-seeding the depth (the
two cheaper ladder rungs) measurably do **not** shorten onset: an A/B showed
no resolved improvement (bootstrap difference +0.43 frames, 95% CI
[-0.56, +1.46], spanning zero). The one remaining lever — a gesture-primed
arm that speculatively latches the regime on touch-down via a new
Kotlin→Rust signal — is more invasive and awaits a ruling; it is not
automatic.

**Evidence**: on-device A/B measurement (Android, cupid) — see Why not fixed
above for the result.

---

### `cam-tail-p90-residual` — shape-aware sync tail does not clear its own bar (Android)

**Observed**: with the shape-aware sync tail active, cupid's steady-state
platform-view lead has a p90 of 1.78-1.98 display frames (~15-17 ms) against
a ≤0.25-frame p90 target that it does not clear. Median lead is 0 at every
measured velocity and overshoot is 0 everywhere.

**Applies to**: Android, acquire-bound devices in the same class as cupid
(the regime the tail activates for at all).

**Why accepted**: strictly better than the gate-only control on the same
device/session (p90 4.8-4.9 frames), zero overshoot at every velocity.
Accepted by Ed on 2026-07-26 partly because a camera preview is expected to
be placed on a static surface rather than inside a scroll view, which makes
this residual unobservable for that use case — but the sync machinery itself
stays load-bearing regardless: playground's own camera page deliberately
scrolls the slot to exercise the dispose/revive cycle, and `platform_views`
remains a scrolling Mode B testbed.

**Evidence**: on-device measurement (cupid); closed and accepted per Ed's
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

**Evidence**: `plugins/camera/src/barcode/engine.rs` (the private
`RqrrEngine`, `rqrr` the sole v1 implementation); `rqrr`'s own
`quirc`-derived detection algorithm; barcode plugin implementation (task 01).

---

### `barcode-1d-deferred-rotation` — 1D symbologies are deferred until rotation tracking lands

**Observed**: `CameraSession::start_barcode_stream` always requests
`ImageFormat::Yuv420` at whatever rotation the platform reports. On Apple,
the backend currently pins that rotation to a constant portrait 90° rather
than tracking live device orientation (`plugins/camera/src/apple.rs`'s
`STREAM_ROTATION_DEGREES`/`PORTRAIT_ROTATION_ANGLE`, both hardcoded). QR — the
v1 engine's only symbology — is rotation-invariant, so this is unobservable
today; a 1D symbology (Code128, EAN, UPC, …) is orientation-sensitive and
would decode unreliably off-axis under the same fixed rotation.

**Applies to**: every platform this crate targets, once a non-QR 1D engine
is added — not reachable today since v1 decodes QR only (see
`barcode-qr-only-v1` above).

**Why accepted**: adding 1D coverage ahead of live rotation tracking would
ship a symbology that only decodes correctly in one physical orientation.
Deliberately sequenced: rotation tracking is a prerequisite for 1D support,
not a parallel-track improvement.

**Evidence**: `plugins/camera/src/apple.rs` (`STREAM_ROTATION_DEGREES`/
`PORTRAIT_ROTATION_ANGLE`, both hardcoded 90°); barcode plugin implementation
(tasks 01-02).

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
lensFacing)` contract carries no resolution parameter"); barcode plugin
implementation (task 02).

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

**Why accepted**: the CPU and thermal cost on ARM mobile is unverified — no
on-device benchmark exists for any pure-Rust decoder. Phase-4 device-gate
metric: playground's scan strip's timing mode exists to measure exactly this.

**Future lever (documented option only, NOT implemented)**: advance the
absence counter on skipped/undecoded frames too, decoupling counter cadence
from decode cadence and allowing a slower-than-camera-rate re-arm delay.

**Evidence**: `plugins/camera/src/barcode/stream.rs`'s `should_decode`
(NoDuplicates and Unrestricted always return `true`); barcode plugin
implementation (tasks 01-02).

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
surveyed for a custom-format write (none); flagged during review-r1's
`set_text_sensitive` fix. See `plugins/clipboard/README.md`'s `## 2.
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
system edit menu, `ios-native-edit-menu-device-status`); desktop and web have
their own gates.

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
this plan's code (`nativeFocusGeneration`, `readClipboardTextAsync` and
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
the framework's focus *session identity* (`RenderRoot::focus_epoch`, advanced
once per honoured focus claim and once per session release), so the check is
an identity rather than a proxy for one: focus cannot move from one field to
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
counter beside it stands still is pinned by `crates/frust-core/src/app.rs`'s
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

### `ios-native-edit-menu-device-status` — the iOS system edit-menu route has run on an iPhone; the hardware-chord half of the paste exemption has not

**Observed**: iOS *locks* `SelectionToolbarPolicy::Native` at `frust_init` —
an app's later `set_selection_toolbar_policy` is refused rather than obeyed,
because the paste exemption below depends on the native route being the only
one — so a focused field floats no toolbar of its own and publishes where a
menu would be anchored and which verbs apply. It publishes that whether or not
it has a selection: the verbs are a level the responder chain reads whenever
UIKit asks, including for a hardware `Cmd+V` with no menu on screen. `FrustView` answers the four
`UIResponderStandardEditActions` from that published set, presents UIKit's own
menu at the published anchor (`UIEditMenuInteraction` on iOS 16+, the
deprecated `UIMenuController` below it), and reads the pasteboard inside
`paste(_:)` — the system-initiated read that is exempt from the iOS 14+
"pasted from" banner and the iOS 16+ per-app permission alert, which is the
entire reason the native route exists rather than the framework toolbar.
**Run on an iPhone SE (iOS 26.6.1) on 2026-09-12** against a debug build of
`examples/glyph-catalog`, whose installed binary was confirmed to carry this
code (the `presentMenu` wire field and the locked-policy diagnostic were both
found in the shipped dylib, against positive controls). Observed: the system
edit menu appears over a selection, the framework floats no toolbar of its
own, the verbs act on the focused field — and **a paste from the system menu
raised NO per-app permission alert**, which is the exemption this whole route
exists to buy. That is the first hardware observation of it in this plan.

**What that run did NOT cover, and it is the half this plan changed most:**
no hardware keyboard was attached, so `Cmd+C`/`X`/`V`/`A` were never pressed.
Hardware `Cmd+V` is the *other* exempt route, and until this plan a focused
field published its verbs only while its own bar was open — so the chords were
unanswerable on the ordinary tap-to-focus path. The fix (a focused field
publishes its verbs as a level, whether or not a menu is up) is therefore
verified by unit tests and by compilation, and NOT on a device. The
`SHELLS_DEVELOPMENT.md` gate step for it deliberately says to run it before
any leg that opens the menu, because a build with the old gating passes every
menu-first leg. (2026-09-12)

**Applies to**: iOS only — `FrustView.swift`'s edit-menu and
`UIResponderStandardEditActions` surface, `FrustViewController.swift`'s
per-frame `frust_selection_toolbar_json` poll, and the Rust half in
`crates/frust-shell-ios`. It shares its unverifiability with
`ime-ios-content-type-unverified`, whose IME surface lives in the same view
and has the same absence of a `cargo`-reachable gate.

**Why accepted**: the Swift half of this shell has no gate short of a device
run — `cargo` cannot compile Swift, so no workspace gate reaches it. The menu
and exemption legs are now observed; the hardware-chord leg remains owed
rather than skipped, and is named here so it is not quietly assumed to have
passed alongside the legs that did.

**Evidence**:
`crates/frust-shell-ios/platform/ios/FrustEmbedding/Sources/FrustEmbedding/FrustView.swift`'s
"Clipboard / system edit menu" section with its `canPerformAction` and
`paste(_:)` overrides (the exemption is stated there in place);
`crates/frust-shell-ios/src/ffi_glue.rs`'s
`lock_selection_toolbar_policy(SelectionToolbarPolicy::Native)` at init and its
`selection_toolbar_json` body (exported as `frust_selection_toolbar_json` from
`crates/frust-shell-ios/src/lib.rs`);
`crates/frust-shell-ios/src/ffi_support.rs`'s anchor/verbs JSON shape and
edit-command wire codes.

**Trigger for removal**: a device run with a hardware keyboard attached,
observing `Cmd+V` paste into a field that was tap-focused (never long-pressed)
with no menu on screen, and raising no permission alert. The remaining legs
are done; that one closes the entry. Previously this said: a device run
observing the menu
presented at the selection, each of the four verbs applied to the focused
field, and a paste raising neither the "pasted from" banner nor the
permission alert.

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
content-type application (FINDINGS #31: suppressing the QuickType
suggestion bar and keyboard learning on a `"password"`-classified field),
the **mirror re-seed/reconcile** that keeps `FrustView.mirror` from carrying
a previous field's text across a focus move (`syncImeFocus`'s `seedMirror`
on the content-type branch and `FrustView.reconcileMirror(to:)` on the
steady-active branch), and the **per-frame `syncImeFocus()` call from
`renderFrame`** that makes non-touch focus moves (Return-to-next-field,
programmatic focus) reach any of it — has never run on an iOS device or
simulator. `cargo` cannot compile Swift, so no ordinary workspace gate
touches this path; it has been `swiftc -typecheck`-verified on a macOS host
twice, most recently alongside this round's iOS trait-matrix fix (F2, see
its commit message), which confirms the sources parse and type-check but
does not link, run, or exercise any of the behavior below. Only
`xcodebuild -scheme FrustEmbedding -destination 'generic/platform=iOS'
build` (a full build) or an on-device/simulator run validates that, and
neither has occurred since this path was introduced or since any of these
fixes landed.

**Also covers (fixes F3/F3b, review-fix-3)**: `syncImeFocus`'s per-frame
`becomeFirstResponder()` retry is now **bounded** (`imeFocusSatisfied`)
instead of fighting an intentional UIKit-originated resign (user swipe-
dismiss, a sibling native control taking first responder) forever, because
nothing on the Swift side can observe *why* first responder was resigned.
A user touch on the Frust surface (`onTouch`, user-initiated) re-arms the
bound — **but only while the surface is not already first responder**
(fix F3c). That is what lets a user bring back a keyboard they dismissed
themselves by re-tapping the field, since re-tapping an already-active field
changes neither Rust's `active` nor `contentType` signal. The
not-already-first-responder half is load-bearing: `imeFocusSatisfied` is set
back to `true` only in the `!isFirstResponder` branch, so re-arming during a
touch inside a *still-focused* field would leave it dangling at `false` and
reassert the keyboard on the next UIKit-originated dismissal — reopening the
very pop-back the bound exists to prevent. A touch inside a Mode B hosted slot never reaches `onTouch`
(`FrustView.hitTest` returns `nil` there), so a sibling native control
retaining first responder is unaffected by the per-frame tick. This
mechanism is Swift-only and is exactly as device-unverified as the rest of
this entry.

**Residual limitation, accepted (not a bug to fix here)** — and note F3c's
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
`FrustInputConnection.reconcileTo`, the model these fixes follow
property-for-property) is likewise device-unverified per its own review
record, but is a materially different code path (`EditorInfo`/
`restartInput`/`Editable` vs. UIKit `UITextInputTraits`/resign-become/
`NSMutableString`) and this entry makes no claim about it. One asymmetry is
worth stating: Android's `applyImeContentType` inputType/imeOptions flags
(fix F5) were Kotlin-compiled clean this batch (`compileDebugKotlin`,
32/32 tasks) — the first Kotlin change in three batches to clear that
gate — while the iOS Swift side has only been `swiftc -typecheck`-verified,
never built via `xcodebuild` (Kotlin's `compileDebugKotlin` equivalent).
Compile/typecheck-verified is not device-verified, so F5 narrows FINDINGS
#31's Android gap without closing the finding.

**API-level note — resolved by raising the floor to 26.**
`EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING` was added in **API 26**. While
frust's floor was `minSdk = 24` the keyboard-learning suppression was a
*silent* no-op on API 24 and 25 — it is an `imeOptions` bit rather than a
method call, so the constant inlines at compile time, an older IME simply
ignores the unknown bit, and neither a crash nor a `NewApi` lint would have
surfaced it. **Every frust Gradle module now pins `minSdk = 26`**, so the flag
is unconditionally honoured and the Android contract table holds at the floor
with no API caveat. Do not lower the floor below 26 without re-introducing
this caveat.

**Why not closed**: FINDINGS #31's only proof gate is on-device
verification, and that gate is currently blocked on device access. **The
leak is therefore not confirmed closed on any platform** — the Rust-side
wire encoding (`content_type_wire`) has unit coverage, but nothing has
exercised the UIKit trait application, the resign/become keyboard cycle,
the mirror re-seed, the per-frame reconcile, or the bounded-retry/touch-
re-arm mechanism above against a real (or simulated) keyboard. The
per-frame reconcile in particular has an unmeasured cost (one
`frust_ime_state_json` FFI call plus one `JSONSerialization` parse per
`CADisplayLink` tick, matching Android's per-frame `nativeImeState` budget)
and an unobserved interaction with UIKit autocorrect/composition, both
reasoned about but not instrumented. Treat the current implementation as
the best-reasoned fix available, not as a verified fix.

**Device-gate observables, for whoever runs it** — five separate runs:
1. *Traits across a focus move (#31).* Focus a `"normal"` field, type a few
   characters, then move focus directly to an obscured (`"password"`)
   field **without dismissing the keyboard in between**. Confirm (a) no
   QuickType suggestion bar appears while typing into the second field, and
   (b) nothing typed there is echoed anywhere (suggestion strip, autofill
   preview).
2. *Mirror staleness (N2).* Same two-field form, both fields **normal** so
   no content-type change fires. Type `hello` into A, move focus to empty
   field B without dismissing the keyboard, type one character. B must
   contain exactly that one character — **not** `hello` + the character.
   Repeat with the second field obscured to cover the content-type branch's
   re-seed ordering.
3. *Non-touch trigger (N4).* Same as (1) and (2), but move focus with the
   keyboard's **Return/Next key** rather than a tap, so no touch reaches the
   surface. Both observables must still hold; before this fix
   `syncImeFocus` never re-ran on that path.
4. *Bounded retry + touch re-arm (F3/F3b).* Focus field A, let the keyboard
   appear, then swipe-dismiss it — the keyboard must **stay down** (no
   per-frame pop-back). Tap field A again — the keyboard must **reappear**.
   With a Mode B sibling native control focused, confirm the per-frame tick
   does not steal focus back from it. Dismiss field A by swipe, tap a
   *different non-editable* part of the surface, then tap field A again — it
   must still recover (exercises the clear-on-touch design point, and its
   residual limitation above).
5. *Touch inside an already-focused field, then dismiss (F3c).* Focus field
   A, then **tap or drag inside A while it still holds the keyboard** —
   caret repositioning or a selection drag. Now swipe-dismiss the keyboard
   with **no further touch on the Frust surface**. It must **stay down**.
   This is a distinct run from (4): the touch there happens *after* the
   dismissal, this one *before* it. An earlier revision re-armed the bound
   on any touch, including one that never resigned the responder, which left
   `imeFocusSatisfied` dangling and reasserted the keyboard on the next tick
   — reopening (4)'s own guarantee for the most ordinary interaction there
   is. The re-arm is now gated on `!isFirstResponder`, but **that gating is
   reasoned, not device-measured** — this run is what would catch a
   regression.

**Evidence**: `swiftc -typecheck` exit 0 (most recently this round's F2
fix, see its commit message); no device or simulator run has occurred.

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
only: `ReorderableListView` (crates/frust-widgets/src/drag/reorderable.rs:202-220,
`self.inner: AnyView<State>`). The single-erasure case — `any(Wrap(concrete_view))`
and every in-crate container's own child list — detects swaps correctly and is
unaffected. `any(any(view))` is CLOSED by idempotent erasure (`AnyView::new`
unwraps an `AnyView` argument), so the residual is only wrapper views.

**Why not fixed**: closing it needs shared `TypeId` reporting through
`ErasedView` so nesting composes. Each reconciler today probes whatever boxed
element it holds; `ErasedView` would need to report the element's concrete
`TypeId` through the erasure boundary so `Wrap`'s element type is transparent to
the swap check. That is a `frust-core` trait-surface change landing on every
reconciler at once, and was deliberately not attempted during the focus/IME
review fixes.

**Evidence**: source inspection of `crates/frust-core/src/view.rs` (AnyView's
idempotent `new`), `crates/frust-widgets/src/drag/reorderable.rs` (the only
in-tree wrapper; the `inner: AnyView<State>` field (:106), and the `type Element = Box<dyn Widget>` in its
`impl View<State> for ReorderableListView<State>` block),
`crates/frust-widgets/tests/double_erasure_swap.rs` (tripwire test
`wrapper_view_erasure_swap_blind`), `crates/frust-widgets/tests/wrapper_view_guard.rs`
(source-scan guard failing on any new in-tree view of this shape), and `crates/frust-widgets/src/authoring.rs` (swap detection).

---

### `keyed-list-all-or-nothing-debug-only` — a mixed keyed/unkeyed list is caught by a `debug_assert` only, and the erasure codemod skips some sites

**Observed**: a `FlexView` / `column()` / `row()` child list must be all-keyed
or all-unkeyed. Mixing `.keyed(..)` with `.child(..)` / `.flex(..)` is detected
by a `debug_assert!` on rebuild only
(crates/frust-widgets/src/authoring.rs:~557 documents the tripwire; like the
`debug_assert`s noted at list_view.rs:~102 it is inert in release builds), so in a release build a mixed list silently falls back to
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
`crates/frust-widgets/src/authoring.rs` (`rebuild_children`'s `debug_assert`), and the Phase 3
review round of the any-erasure plan (its keyed-equivalence and codemod
findings).

---

### `focus-navbar-item-truncation-unmarked` — a truncated navbar/tabbar item drops its focus link silently

**Observed**: `frust_material::navbar`'s `NavigationBarView::rebuild` and
`frust_cupertino::tabbar`'s equivalent (both now `plugins/{material,cupertino}`) hand-roll their
item pod lists rather than
going through `authoring::rebuild_children`. Their shrink arm honors half of
the contract — an in-flight capture is cancelled (`cancel_item` +
`set_active(false)`) and each item's icon/label children are torn down through
`teardown_child`, which marks its own orphans — but never consults the **item
pod's own** `is_focused()` flag: it is neither cleared nor reported via
`mark_focus_orphaned` before `items.truncate(common)` drops the pod. An item
holding the live focus path, truncated out of a shrinking bar, would leave
`RenderRoot`'s `focus_active`/`ime_state` standing over a widget that no longer
exists.

**Applies to**: **latent — unreachable today.** Neither `NavItemWidget` nor
`TabItemWidget` ever calls `EventCtx::request_focus`, so an item pod's
`focused` flag is never set in the first place and the gap cannot be triggered
from app code. It becomes real the moment either item type takes focus, or a
future hand-rolled item list copies this shape for a focusable child.

**Why accepted**: nothing is observably wrong today, and the two candidate
fixes are both larger than the gap. Routing these lists through
`authoring::rebuild_children` (which already implements capture-cancel,
focus-clear, and the gated orphan mark in one place) is the right end state but
re-shapes two catalog widgets' reconcilers; replicating the
`ctx.has_focus() && pod.is_focused()` gate inline is smaller but adds a third
hand-rolled copy of a contract that already has one home. Deliberately
deferred rather than fixed speculatively during the focus/IME fixes.

**Evidence**: source inspection of
`crates/frust-widgets/src/material/navbar.rs` and
`crates/frust-widgets/src/cupertino/tabbar.rs` — the `self.items.len() <
prev.items.len()` arm in each, plus the absence of any
`request_focus`/`is_focused`/`set_focused` occurrence in either file (which is
what makes it latent); found during review-fix-3 (FC)'s audit of the
focus-severing sites.

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
from `CosmeticLoopRate::FLOOR_HZ` (`crates/frust-theme/src/motion.rs`);
restated here during review-fix-3 (FC) so the app-visible consequence is
discoverable from the register rather than only from a field doc.

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
returns, so the gap is signal-path-only. Exposure increased 2026-08-04: bare
`frust` now defaults into the TUI (single-binary convergence), making Android
**release** builds more likely to be launched from a raw-mode session — the
TUI's build launcher drives `frust-drive::android_build::build` in-process
(`spawn_blocking`).

`kill <pid>` (SIGTERM) and terminal-close (SIGHUP) still scrub correctly in a
TUI process — the same `ctrlc` `termination` feature + `frust-drive::interrupt`
three-signal handler that covers the CLI; only the raw-mode-delivered keyboard
Ctrl-C byte is inert. That signal path exits via `std::process::exit`, which
bypasses the TUI's `ratatui::restore()`/mouse-capture teardown, so a
SIGTERM'd TUI scrubs the plaintext file correctly but leaves the terminal in
raw mode/alternate screen afterward (pre-existing, not a regression of this
change; a shell `reset` recovers it).

See also `tui-both-streams-tty-gate` below for a related, deliberate TTY-gate
narrowing introduced in the same change.

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

**Why accepted**: the gap is pre-existing (the raw-mode/ISIG interaction has
always existed when a TUI terminal is in raw mode), and deferred per Ed's
review r0 (2026-08-04) on the single-binary branch. The exposure increase is
real: many new users will now encounter the TUI first. Mitigation exists
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

**Evidence**: frust-single-binary review r1 (2026-08-04); plan-verify sweep
(wf_d589d3ec-080, confirmed crossterm's raw-mode ISIG clearing and raw-mode
SIGINT immunity on Darwin and Linux manual testing; confirmed CLI path scrubs
via signal, TUI path does not; confirmed build-launcher no-op on Ctrl-C
cancel).

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
the related signal-handling gap introduced in the same change). There is
deliberately no environment escape valve: `FRUST_NO_TUI` was considered and
rejected by Ed (2026-08-04) — "frust --args for CLI and frust for TUI should
cover everything."

**Evidence**: frust-single-binary reviews r0-r2 (2026-08-04); plan-verify sweep.

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

### `iap-store-account-gated-verification` — no real-store-account purchase round trip has been exercised end to end

**Observed**: `frust-iap`'s connection/product/purchase flows against a real
store need either a Play Console internal-testing-track listing (Android) or
an App Store Connect product plus a sandbox tester (iOS) — neither exists
for this repo yet, so no purchase against a real store account has ever been
exercised end to end. That is a coverage gap, not a capability gap: the
mocked host-side path and, on iOS, the StoreKit-Testing path are both
exercised today.

**Applies to**: both mobile platforms' real-store paths specifically. iOS is
covered without an account via a `.storekit` local StoreKit-Testing
configuration (`plugins/iap/README.md` §4), which exercises
`init_connection`/`fetch_products`/`request_purchase`/`finish_transaction`
against a simulated catalog. Android has no offline equivalent — Play
Billing always talks to the real Play service — so its coverage today is
limited to the mobile compile gates and the host-side conformance suite's
error paths (`docs/PLUGINS_ARCHITECTURE.md`'s Data Flow).

**Why accepted**: store-account setup (a paid developer account, a
published or internal-track listing) is an operational prerequisite outside
this crate's own scope. The parts of the contract that don't need one are
covered host-side: the connection state machine, guard order, and two-phase
purchase contract against the `cfg(test)` fake store
(`plugins/iap/src/conformance.rs`), and the plugin-owned event-delivery
thread (`plugins/iap/src/event.rs`) — the single FIFO consumer every
purchase-update callback is handed to, delivering off the UI thread and off
the caller's thread in enqueue order — is unit-tested independent of any
store backend. What remains unverified is narrowly the real store round trip
itself, not the platform-independent contract around it.

**Evidence**: `plugins/iap/README.md` §3 (Store setup) and §4 (Testing
without a store account); `plugins/iap/src/conformance.rs`'s module doc;
`plugins/iap/src/event.rs`'s module doc.

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

**Why accepted**: the primary failure paths (round-0 review F2/F9) are closed — the
guard makes "no exit without a rollback attempt" structural, and the double-fault
window is narrow and requires storage-level failure. A `tainted`-handle flag with a
typed error on subsequent use is the known remedy if this residual is later promoted;
it was deferred rather than designed under the review-loop cap. Apps needing robustness
against this class drop the handle on any `transaction` error and reopen (README §4a).

**Evidence**: `plugins/database/src/lib.rs` `RollbackGuard::drop` (best-effort
`ROLLBACK`), the qualified poison-policy doc on `lock_conn`, README §4a's closing
caveat.

### `tui-shimmer-ansi16-degrade` — the §B10 phase-line shimmer degrades to flat+BOLD at Ansi16

**Observed**: `themed_shimmer_spans` (`crates/frust-tui/src/ui/anim/shimmer.rs`) sweeps a
real color ramp at TrueColor (a live RGB lerp) and at Xterm256 (`Theme::SHIMMER_RAMP_X256`,
a fixed 5-entry palette-index table bucketed per character), but at `ColorDepth::Ansi16`
there are only the theme's two named ANSI colors to choose from — too coarse a palette for
a convincing color sweep between them. The head-tracking motion still plays: characters
near the sweep head get `Modifier::BOLD` against a flat `theme.muted()` foreground
everywhere else, rather than any color change.

**Applies to**: any Ansi16-depth terminal (`ColorDepth::detect()`'s conservative floor for
an unrecognized `$TERM`, `TERM=dumb`, or no `TERM` at all) rendering the build/install
phase-line shimmer (workbook §B10).

**Why accepted**: Ansi16 has no intermediate hues between the theme's muted and accent
tokens to sweep through — a real color ramp needs a palette this depth doesn't have.
BOLD-only motion is still a visible, cheap sweep at any depth, and keeps the degrade
decision inside `themed_shimmer_spans` itself (dispatched on `theme.depth()`) rather than
pushed out to call sites.

**Evidence**: `crates/frust-tui/src/ui/anim/shimmer.rs`'s `themed_shimmer_spans`/
`shimmer_spans_flat_bold`; `crates/frust-tui/src/ui/theme.rs`'s `ColorDepth`/
`ThemeColor::resolve`; flagged as review Major M4 (shimmer color-depth degrade), fixed in
the same round with this documented residual.

---

### `devtools-ios-physical-forward-deferred` — the devtools client can't reach a physical iOS device

**Observed**: `frust-drive`'s devtools client connects directly over localhost (desktop, and
iOS Simulator, which shares the host's loopback) and forwards through `adb` for a physical
Android device. There is no equivalent for a physical iOS device — no forwarding helper
exists, so a client cannot reach the in-app debug service on one.

**Applies to**: `frust drive`/`frust-tui` devtools sessions targeting a physical iOS device.
Desktop and both simulators/emulators are unaffected.

**Why accepted**: physical-iOS forwarding needs `usbmuxd` (or an equivalent) and a Mac to
build/verify against, neither available on this build host. Deferred by plan decision
rather than discovered as a gap; the natural v2 step once a Mac session is available.

**Evidence**: iOS physical-device forwarding via usbmuxd is an explicit non-goal of the
frust-tui devex feature.

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

### `devtools-e2e-component-only` — no test binds the real drive client to the real service in one process

**Observed**: the drive-client↔real-service path is component-tested from both directions — the
real `DevtoolsClient` against a hand-rolled canned wire server (`frust-drive`), and the real
`Service` against a hand-rolled NDJSON client (`frust-devtools`'s `loopback.rs`) — but no test
exercises the real client and the real service together in one process. Adding one would need a
direct dev-dependency from `frust-drive` (tooling) onto `frust-devtools` (framework) or vice versa,
which breaches the tooling/framework isolation charter (see DEVTOOLS_ARCHITECTURE.md's Layer
Dependencies): the two sides may meet only at the protocol leaf.

**Applies to**: the devtools wire protocol end to end; each side's contract is covered, but never
the pair.

**Why accepted**: the charter this would breach is load-bearing (it is what keeps a tooling crate
from dragging `tokio` into the framework graph, and vice versa), so a same-process integration test
is structurally out of reach from either crate alone. Phase 3's TUI integration exercises the real
client against a real app's real service as an ordinary consumer (a separate binary, not a
dev-dependency), and is the closure path for this gap.

**Evidence**: `crates/frust-drive/src/devtools_client.rs`'s hand-rolled fake-server tests;
`crates/frust-devtools/tests/loopback.rs`'s hand-rolled fake-client test; DEVTOOLS_ARCHITECTURE.md's
tooling/framework isolation charter.

---

### `devtools-token-entropy-windows-fallback` — Windows devtools tokens come from the non-CSPRNG fallback

**Observed**: the devtools handshake token's strong-entropy path reads `/dev/urandom`, which does
not exist on Windows; `os_random_bytes` is `#[cfg(unix)]`, so every Windows debug/profile session
mints its token from the documented fallback (a composition of OS-seeded `RandomState` SipHash
outputs, wall clock, monotonic instant, pid, and a stack address) — 128 bits an unprivileged
co-resident peer cannot practically enumerate, but not a CSPRNG.

**Applies to**: the Windows desktop shell only; Linux, Android, macOS, and iOS all take the
kernel-CSPRNG path.

**Why accepted**: `frust-devtools` carries no CSPRNG dependency budget (protocol + tokio + log;
pins are law) and no Windows host exists in the current verify environment to validate a
`BCryptGenRandom` FFI path. The fallback still gates the listener behind an unguessable-in-practice
secret; the exposure window is debug/profile developer builds on the developer's own machine. A
`BCryptGenRandom`-based source (via `std::os::windows` FFI, no new crate) is the named follow-up
when a Windows verification host is available.

**Evidence**: `crates/frust-devtools/src/token.rs` (`os_random_bytes` cfg gate + module doc).

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
implementation for Linux; on macOS and Windows every fetch returns `MetricsError::Unsupported` at
runtime rather than failing to build.

**Applies to**: macOS and Windows desktop sessions. Linux desktop (the dev/CI platform) and Android
are both fully supported.

**Why accepted**: a deliberate no-new-dependency decision for this feature — `sysinfo` was the
plan's original option and was dropped to keep `frust-drive` free of the added dependency;
a native macOS source (e.g. `host_statistics`/IOKit) is deferred rather than pursued in this phase.

**Evidence**: `crates/frust-drive/src/metrics/desktop.rs`'s module doc ("Linux-only... the macOS
gap is tracked as a `docs/LIMITATIONS.md` entry"); the no-`sysinfo`-dependency decision defers
macOS metrics to this entry.

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

**Why accepted**: a deliberate v1 stance ported from fdemon-pro, matched to the threat model of a
loopback developer tool: the bind is not reachable off-host, and the devtools handshake token
still protects the app-side service itself. Server-side auth (e.g. a bearer token) is a named
follow-up, not a v1 requirement.

**Evidence**: `crates/frust-mcp/src/config.rs`'s `McpConfig` doc comment (bind address is never
configurable); `crates/frust-mcp/src/server.rs`'s module doc (Host-header guard, no other auth);
`crates/frust-mcp/src/engine.rs`'s `run_app` doc comment (no per-call project override).

---

### `mcp-ios-simulator-unverified` — `frust-mcp` iOS Simulator sessions are compile-clean but unexercised

**Observed**: `frust-mcp`'s session engine and tool layer share the same iOS Simulator run pipeline
`frust-drive` already ships, but no device/Simulator run of an MCP-launched iOS session has been
performed this phase — only desktop and Android sessions are exercised by the crate's own tests
and manual runs.

**Applies to**: `frust-mcp` sessions with `target` resolving to an iOS Simulator udid.

**Why accepted**: no Mac was available this phase; the code path is shared with `frust-drive`'s
already-verified iOS pipeline, so the residual risk is scoped to the MCP session engine's own
wiring (devtools connect, log parsing) rather than the launch pipeline itself.

**Evidence**: `crates/frust-mcp` test suite (`engine_lifecycle`, `tool_families`, `http_smoke`)
exercises desktop and Android targets only, via `FakeProcessRunner`.

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

**Observed**: `frust-mcp`'s `ClientRegistry` (the TUI's MCP panel, §B13, reads it live) tracks only
a count, a mint-order opaque `id`, and a connect time per session — no client name, version, or
capabilities. Disconnection has exactly one signal: the per-session `ClientGuard`'s `Drop`, fired
when rmcp tears the session down. The Streamable-HTTP transport exposes nothing lower-level to
build a better signal from, so a client that vanishes without a clean teardown (a killed process, a
network drop rmcp doesn't notice) leaves its entry — and the panel showing it as connected — until
the server itself stops.

**Applies to**: `frust-mcp`'s `ClientRegistry` in both `serve` and `serve_embedded` modes; the TUI's
MCP panel client list.

**Why accepted**: matches fdemon-pro's own client-tracking precedent for the same transport.
Per-client name/version display is a named follow-up, gated on confirming rmcp actually exposes an
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

### `tui-mcp-sessions-tab-uncapped` — `AppState::sessions` (session tabs) has no eviction and grows for the process lifetime

**Observed**: `frust-tui`'s `AppState::sessions` map — one entry per session tab — has no close or
eviction mechanism of any kind; every session the workbench has ever launched, human-driven or
MCP-driven, keeps a tab entry for the rest of the process's life. MCP-driven growth is bounded one
layer down: `supervise::mcp_backend::McpSessionRecords` caps its own retained launch records at
`MCP_RECORD_CAP` (oldest-terminal evicted first, live never evicted) and `run_app`/`restart_app`
refuse once that cap of live MCP sessions is reached — but the *tab* an evicted record backed is
never itself removed, so it survives in the UI as an un-restartable ghost: `restart_app` on its id
reports `EmbeddedError::NoSuchSession`, diverging from `frust-mcp`'s own reference `SessionEngine`,
which evicts its terminal-session record and its tab-equivalent state together. The human-driven
insert path — every session a user launches from the workbench's own UI — has no cap at all; only
the MCP-driven path is bounded, because only an unattended agent can plausibly launch sessions for
hours unattended. The keyboard restart (`R` / palette 'Restart session') shares this same ghost
risk and has the same toast-not-silent-drop fix as its MCP twin: `runner::apply_effect` reconciles
`McpSessionRecords` with the engine after every `update()` (`observe`/`mark_closed`), so a record
whose tab was closed or replaced by a restart now counts as finished and is evictable — the map no
longer grows by one record per keyboard restart — and a keyboard restart that lands on an already-
evicted record surfaces a Warn toast ('no launch record for this session — relaunch it with r')
instead of the old stderr-only log line.

**Applies to**: `frust-tui`'s `AppState::sessions` for the whole session lifetime; the MCP-launched
subset's *record* (not tab) is capped as described above.

**Why accepted**: no tab-close mechanism exists in the workbench to build an eviction policy on top
of (closing a tab a user might still want to scroll back through is a UX decision, not a memory-
safety one); the unbounded growth is real but slow enough in the human-driven case (bounded by how
many sessions a person opens in one sitting) that the records-layer cap on the actually-unattended
MCP path was judged the fix that matters for v1.

**Evidence**: `crates/frust-tui/src/supervise/mcp_backend.rs` module doc ("What this backend
deliberately cannot do" — "An evicted MCP record's tab still exists"); `McpSessionRecords::
retain_bounded`; `run_app_refuses_bookkeeping_free_once_the_cap_of_live_sessions_is_reached` test.

---

### `no-hot-reload-restart-is-a-rebuild` — every restart is a full rebuild + relaunch, never a hot reload

**Observed**: every restart path — the TUI's `R` keypress / palette 'Restart session', the TUI's own
'Watch: restart on save' (`W`/palette/run-config checkbox), `frust run --watch`'s file-change
relaunch, MCP's `restart_app`, and DAP's `frustRestart` — is a full rebuild and relaunch with app
state reset each time: Flutter's "hot restart" semantics, never "hot reload". A device restart
reruns the whole build → install → launch pipeline rather than patching a running process (see
`tui-device-stop-app-termination-residual` and `mcp-stop-app-termination-in-flight` for what "stop"
already does and does not guarantee before that relaunch begins). The TUI's own `R` restart shares
that same best-effort stop window: the stop is issued, not awaited, before the relaunch fires.
'Watch: restart on save' is desktop-only — a device or ad-hoc session refuses it (`Message::ToggleWatch`)
with "Watch is desktop-only: the watch loop has no device-side kill/rebuild/relaunch story yet",
`frust run --watch`'s own reason — and shares `R`'s rebuild+relaunch path (`engine::update`'s
`restart_session_at`) rather than being a fourth mechanism. `engine::update`'s `on_session_event` turns a session's `watch` flag off the moment it lands
`SessionState::Killed`, whichever path killed it — the keyboard `x`, `close_tab` (X/palette/context
menu, which clears it immediately rather than waiting for `Killed`), MCP's `stop_app`, DAP
terminate/disconnect, or `restart_app`'s own kill of the session it replaces — so a stopped session's
watcher never outlives it; `Exited(_)` is left untouched, so a crash or compile-error exit keeps a
watched session watching and the next save still relaunches it. Neither MCP's `restart_app` nor DAP's
`frustRestart` carries that flag onto the *new* session, though: both bypass `restart_session_at`, the
one seam that re-sends `EnableWatch` after a relaunch, so the replacement session always starts
unwatched and watch must be re-toggled by hand afterward. On MCP/DAP stop paths the old tab is not
removed either — it parks in the session list as `Killed`, same as any other MCP-launched session (see
`tui-mcp-sessions-tab-uncapped`). The 300ms
trailing-edge debounce itself is duplicated rather than shared: `frust-cli`'s `watch_loop_with_slot`
and `frust-tui`'s `supervise::watch` each run their own copy (`frust-tui` has no dependency on
`frust-cli`) — a tracked follow-up is moving it into `frust-drive`. On Windows, both loops' kill
(the TUI's session stop/restart and the CLI's respawn) already route through the same
`frust_drive::process::StreamHandle::kill` → `windows_tree_kill` (`taskkill /T /F`) path, so a
watched session's relaunch reaches the whole `cargo run` tree there too, falling back to a
direct-child-only `Child::kill` only if `taskkill` itself is missing or fails.

**Applies to**: every restart entry point across `frust-tui` (including 'Watch: restart on save'),
`frust-cli`'s `--watch` flag, `frust-mcp`, and `frust-dap` — desktop and device alike.

**Why accepted**: in-process hot restart and hot reload both need capability the framework doesn't
have yet. Hot restart (state reset, code re-run without a process relaunch) would need a seam to
dispose and rebuild the running app, but the root `Component` is taken by value once, by
`frust::run` (`crates/frust/src/lib.rs:1837`); it runs under the shell's **root** `Owner`, which
lives for the whole process and is never disposed (`crates/frust-core/src/component.rs:56-68`); and
`ReactiveRuntime` is installed once into a process-lifetime `OnceLock` and never torn down
(`crates/frust-reactive/src/runtime.rs:122`) — none of the three has a dispose-and-rebuild path
short of exiting the process. Hot reload (patching running code in place) has no Rust-native path
short of subsecond-class hot-patching tooling that is tip-crate-only, unsupported across
struct-layout changes, and experimental/unproven on Android and iOS; the devtools wire protocol also
has no structure-mutating method to carry a reload over (`crates/frust-devtools-protocol/src/method.rs`'s
`Method` enum is read/input-simulation only: `handshake`, `widget_tree`, `widget_props`,
`frame_stats_subscribe`, `frame_stats`, `metrics_snapshot`, `input_tap`, `input_scroll`,
`input_text`, `screenshot`). The rebuild cost is judged acceptable meanwhile: on an i5-12600 Linux
host (2026-09-25), an incremental `cargo build` after touching one file took 1.0s (the app crate),
1.7s (`frust-widgets`), and 1.9s (`frust-core`), against a 43s cold build — consistent with
`docs/DEVELOPMENT.md`'s separately measured 0.89s incremental-build median.

**Reopen path**: a framework spike replacing `frust::run`'s by-value root with a factory closure, a
disposable (not process-lifetime) root `Owner`, a resettable `ReactiveRuntime`, and a devtools
`restart` method to drive the three remotely.

**Evidence**: `crates/frust/src/lib.rs:1837` (`pub fn run<C: Component>`);
`crates/frust-core/src/component.rs:56-68` (`Component::init` doc, root-component owner);
`crates/frust-reactive/src/runtime.rs:122` (`static RUNTIME: OnceLock<ReactiveRuntime>`);
`crates/frust-devtools-protocol/src/method.rs` (`Method` enum); on-host build measurement,
i5-12600 Linux host, 2026-09-25; `docs/DEVELOPMENT.md`'s incremental-build baseline.
`crates/frust-tui/src/supervise/watch.rs` (`WATCH_DEBOUNCE`, module doc's debounce-duplication note);
`crates/frust-tui/src/engine/update.rs` (`WATCH_DESKTOP_ONLY`, `restart_session_at`, `toggle_watch`);
`crates/frust-tui/src/supervise/mcp_backend.rs`'s `restart_app` (no `SourceWatchers` access);
`crates/frust-drive/src/process.rs`'s `windows_tree_kill` (shared by `RealProcessRunner::spawn_streaming`,
which both `frust-cli`'s `run` command and `frust-tui`'s `Supervisor`/`supervise::watch` build on).

---

### `dap-no-stepping-v1` — `frust-dap` has no breakpoints, stack, or variable inspection

**Observed**: `frust-dap`'s DAP surface is launch orchestration only —
`initialize`/`launch`/`configurationDone`/`threads`/`disconnect`/`terminate` plus the
`frustRestart`/`frustWidgetTree` custom requests. There is no `setBreakpoints`, `continue`,
`next`/`stepIn`/`stepOut`, `stackTrace`, `scopes`, or `variables` — `threads` answers a single
static thread only so a client's UI has something to show, not because stepping is supported on it.

**Applies to**: every `frust-dap` session, on every platform.

**Why accepted**: ruling D6 — stepping is deliberately delegated to native lldb tooling (Xcode's
debugger for iOS, Android Studio/`lldb.sh` for Android, a desktop `lldb`/`gdb` attach) rather than
reimplemented on top of the launch-orchestration engine, which has no breakpoint/stack/variable
model to build one on. `frust-dap` orchestrates build/deploy/launch/logs; a real debugger attaches
to the running process for anything past that.

**Evidence**: `crates/frust-dap/src/adapter/mod.rs`'s `threads`/dispatch surface (no
breakpoint/stack/variable request handling); frust-dap feature plan ruling D6.

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

**Why accepted**: v1 scope — one app per connection is the whole orchestration-v1 contract (ruling
D6's launch-only surface), and `attach` needs a session-picker UX this adapter does not have (the
backend exposes `sessions()`, but nothing in the DAP protocol surface here resolves a client's
choice into one). Not attempted this round.

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
`auto_configure_ide` both default **on** (fdemon parity, a deliberate product decision), and the
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
stdio mode this entry once covered was removed; the server is embedded-only now).

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

### `dap-resolution-helpers-mirrored-from-mcp` — device/mode resolution is duplicated, not shared, between `frust-mcp` and `frust-dap`

**Observed**: `frust_mcp::tools`'s device-matching, mode-parsing, and devtools-error-message
helpers are a private module — `frust-dap`'s `adapter/resolve.rs` mirrors their behavior (exact-id
match, then unique case-insensitive substring of id/name; the physical-iOS refusal; the
not-connected/not-supported/unauthorized wording) rather than importing them, since they are not
part of `frust-mcp`'s public API.

**Applies to**: `frust-dap`'s `launch`'s `device`/`mode` resolution and its `frustWidgetTree`
not-connected error wording; a future change to `frust-mcp`'s private resolution logic will not
propagate to `frust-dap` automatically.

**Why accepted**: exporting the helpers would widen `frust-mcp`'s public surface for a single
internal consumer; mirroring was judged the smaller footprint for v1, with the drift risk recorded
here so a future `frust-mcp` resolution change is checked against `frust-dap` too.

**Evidence**: `crates/frust-dap/src/adapter/resolve.rs` (mirrored device/mode matching, doc comments
naming the source) and `crates/frust-dap/src/adapter/mod.rs`'s `widget_tree_failure_message`
(mirrored not-connected/unauthorized phrasing); `crates/frust-mcp/src/tools/session.rs` and
`crates/frust-mcp/src/tools/mod.rs` (the private originals).

---

### `dap-vscode-extension-unverified` — `editors/vscode-frust` has not been runtime-tested in a VS Code host

**Observed**: `editors/vscode-frust` is verified only statically — `node --check` on
`extension.js` and `JSON.parse` on `package.json` — never launched inside an actual VS Code
extension host against a real embedded `frust-tui` DAP server. The extension itself never spawns a
process: its `DebugAdapterDescriptorFactory` returns a `vscode.DebugAdapterServer` pointed at a
launch config's `debugServer`, falling back to the `frust.dapPort` setting and then port 4849 — a
pure attach-by-port connection to whatever `frust-tui` instance is already listening. It is also
unpublished: install is `npx @vscode/vsce package` → `code --install-extension`, never the
Marketplace.

**Applies to**: anyone using the extension for the first time; the DAP server it connects to
(`crates/frust-dap`, embedded in `frust-tui`) is itself covered by `cargo test -p frust-dap`'s and
`cargo test -p frust-tui`'s own tests, so this entry is about the editor-integration layer
specifically — whether VS Code's debug UI actually drives a session end to end through the
descriptor factory and the generated `launch.json`.

**Why accepted**: no VS Code host is available in this environment; the extension's own contract
(connect via `DebugAdapterServer`, resolve `projectRoot` from the workspace folder if the launch
config omits it) is small enough that static verification plus the crate's own tests give
reasonable confidence, but the end-to-end path is unexercised.

**Evidence**: `editors/vscode-frust/extension.js` (`FrustDebugAdapterDescriptorFactory`, never
spawns); `editors/vscode-frust/README.md`; frust-dap feature plan task 03's completion summary
(devbox `node --check`/`JSON.parse` only, no VS Code host run).

---

### `dap-ios-simulator-untested-pending-mac` — `frust-dap` iOS Simulator launches are unverified

**Observed**: `frust-dap`'s `launch` calls `SessionBackend::run_app` on whatever backend its host
supplies — in production, `frust-tui`'s `TuiSessionBackend`, which shares `frust-drive`'s iOS
Simulator run pipeline the same way `frust-mcp`'s own `SessionEngine` does (see
`mcp-ios-simulator-unverified` above) — but no Mac has been available to run a `frust-dap`-embedded
iOS Simulator session end to end through either host; only desktop targets are exercised by the
crate's own tests, which drive the adapter over a test `SessionEngine` rather than the TUI.

**Applies to**: `frust-dap` sessions with `device` resolving to an iOS Simulator udid — the
standing gap every iOS path in this repo carries pending Mac device access.

**Why accepted**: same cause and same mitigation as `mcp-ios-simulator-unverified` — the underlying
launch pipeline is `frust-drive`-verified; the residual risk is scoped to `frust-dap`'s own adapter
wiring (log pump, exit watch, `frustWidgetTree`) on that platform, and to `frust-tui`'s own embed
wiring around it.

**Evidence**: `crates/frust-dap` test suite (desktop targets only, via `FakeProcessRunner`, driven
against a test `frust_mcp::SessionEngine` rather than `frust-tui`'s own backend); see
`mcp-ios-simulator-unverified` above for the shared pipeline's own status.

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

**Why accepted**: fdemon-pro (the ported source this module follows) worked around the identical gap
by spawning a *second*, separate adapter-binary process — a workaround `frust-dap` cannot reuse,
since it has no per-session spawnable binary of its own. Generating a config that could never
actually connect was rejected in favor of honestly reporting nothing was written.

**Evidence**: `crates/frust-dap/src/ide_config/helix.rs`'s module doc and `SKIP_REASON`;
`crates/frust-dap/src/ide_config/mod.rs`'s `generate_ide_config` Helix arm.

---

### `dap-zed-adapter-unverified` — the generated Zed DAP config names an unverified adapter

**Observed**: `crates/frust-dap/src/ide_config/zed.rs`'s `ZedGenerator` names the debug adapter as
`"Delve"` (Go's adapter) in the generated `.zed/debug.json` entry — mirroring fdemon's own Zed
generator, which uses the same name for the identical reason: Zed ships no native adapter for
either language, and `"Delve"` is a name Zed's debug panel already recognises, with the
`tcp_connection` forwarding the session to the already-running server that speaks the actual
protocol. Only the adapter name is mirrored — fdemon writes `"request": "attach"`, frust keeps
`"request": "launch"` (`frust-dap` has no attach story). Never verified against a real Zed release.

**Applies to**: any workbench where the detected or overridden IDE is Zed and a DAP config is
generated for it.

**Why accepted**: Zed ships no native Frust (or Go, fdemon's case) adapter to name honestly;
`"Delve"` is the same workaround fdemon already ships. Verifying needs a real Zed instance,
unavailable this round — do not surface this adapter name in user-facing docs until it is; remove
this entry once confirmed (or replace it if a future Zed release starts validating the
adapter/language pairing and rejects the workaround).

**Evidence**: `crates/frust-dap/src/ide_config/zed.rs`'s `ZED_ADAPTER` doc comment.

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
judged disproportionate for this round — hand-rolling JSONC span-splicing is real parser-writing risk
for a config-generation feature, and no byte-preserving JSON/JSONC crate is pinned in this workspace
(pins are LAW, `docs/DEVELOPMENT.md`'s Version-Pin Policy). `toml_edit` is the precedent for exactly
this shape on the TOML side (`docs/TUI_DEVELOPMENT.md`'s pin row), but it has no JSONC-editing
equivalent pinned here. The chosen remedy for this round is honest disclosure — this entry, plus the
doc-comment corrections on `merge_config`/`run_generator`/`post_write` — rather than a bigger,
unreviewed parser change. This round's other write hardening (temp-file-and-rename landing, and
refusing a config path that resolves via symlink outside the project) changes how a write lands, not
what content is merged, so it closes a different gap without touching the loss described here.

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
on `SessionView::dropped` for the UI to display — it is never appended to the session's own log
store as a line. `supervise::session_feeds::SessionSubscribers::deliver_lines` (what feeds a DAP
client's `SessionEventFeed`) replays only lines actually present in that log store, so a client
attached over `frust-dap` has no way to learn the supervisor itself silently dropped `n` lines
upstream — only the *feed's own* overflow (a separate, already-marked case; see
`dap-log-subscription-may-drop-lines` above) is reported in band.

**Applies to**: any `frust-dap` session whose host is `frust-tui` and whose supervisor-level output
channel overflows; unreachable in ordinary interactive use (the same rarity as the feed's own
overflow), and distinct from — upstream of — that overflow.

**Why accepted**: not an evaluated v1 stance; recorded as a gap found while verifying `frust-dap`'s
embedded-feed contract for this doc pass, not fixed here. Closing it needs either synthesizing a log
line for the drop count (extra plumbing `SessionView` does not otherwise need) or extending
`SessionEvent` with a marker variant `SessionSubscribers` replays alongside real lines — both larger
than a doc-verification pass.

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

**Evidence**: the `dell_mini_pc` Windows 11 Pro 26200 gate, 2026-09-24.

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

**Evidence**: the `dell_mini_pc` Windows 11 gate, 2026-09-24; rust-lang/cargo#6313.

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

**Evidence**: the `dell_mini_pc` Windows 11 gate, 2026-09-24.

**Trigger for removal**: `frust run` generating the icon itself ahead of a first build, if that is
ever judged worth doing.

---

### `android-from-windows-host-unit-tested-only` — building/running Android from a Windows host is unit-tested, not device-verified

**Observed**: building/running Android from a Windows host — the `gradlew.bat` invocation, the
`GRADLE_USER_HOME`/`USERPROFILE` fallback, and the JDK probe's Android Studio bundled-JBR paths — is
proven only by unit tests against injected environments; no Android SDK or device has exercised the
pipeline for real from a Windows host.

**Applies to**: `frust-drive`'s Android build/run pipeline when the host OS is Windows.

**Why accepted**: no Windows host in the current verify environment carries an Android SDK or
device; the unit tests cover the environment-resolution logic (paths, fallbacks, JDK probe order)
but not a real Gradle invocation or device install.

**Evidence**: `crates/frust-drive`'s Android pipeline unit tests (injected `EnvLookup`); no device
gate run.

**Trigger for removal**: a Windows host with an Android SDK and a connected or emulated device
running `frust build apk`/`frust run -d <device>` for real.

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

### `i18n-rtl-unverified` — Parley's intra-line bidi behavior with FSI/PDI marks is unverified

**Observed**: this framework's widget layout is LTR, and nothing in it has been
device-verified rendering right-to-left content. `LocaleSet`'s default bidi isolation wraps
every interpolated placeable in FSI/PDI marks (`U+2068`/`U+2069`) specifically so an RTL
argument cannot reorder the surrounding text — but how Parley's own intra-line bidi algorithm
actually handles those marks alongside mixed-direction text has not been exercised on a
device.

**Applies to**: any message interpolating an RTL argument into an LTR (or mixed) message,
pending the device gate the `i18n` manual test owes (`docs/PLUGINS_DEVELOPMENT.md`).

**Why accepted**: no RTL locale or right-to-left CLDR data has shipped in this repo's locale
sets yet; bidi layout was out of scope for this plugin's v1, which owns message resolution and
formatting, not text layout.

**Evidence**: `plugins/i18n/src/engine/mod.rs`'s `LocaleSet::with_isolating` doc; the pending
`i18n` device gate in `docs/PLUGINS_DEVELOPMENT.md`.

---

### `i18n-currency-minor-units` — CLDR currency minor-unit digits are a table maintained in-crate

**Observed**: `frust_i18n::fmt::currency`'s zero-decimal (`JPY`, ...) and three-decimal
(`KWD`, ...) currency handling comes from two hand-maintained ISO-code tables
(`plugins/i18n/src/fmt/currency.rs`'s `ZERO_DIGIT`/`THREE_DIGIT`) rather than from ICU4X
itself — ICU4X 2.2's `icu_experimental` currency formatter exposes no CLDR `currencyData`
minor-unit-digit lookup a caller can query.

**Applies to**: `fmt::currency` on every platform; a currency ISO code CLDR reclassifies (a
digit-count change, or a move into/out of either table) needs a manual update to keep
rendering correct.

**Why accepted**: `icu_experimental`'s own crate doc calls its whole surface unstable, and no
minor-unit API exists yet to delegate to instead. The maintenance cost is bounded — both
tables are short, CLDR's minor-unit exceptions change rarely — and is reassessed the day the
currency component graduates out of `icu_experimental`, which touches both `fmt/currency.rs`
and `fmt/number.rs` (its `percent` formatter shares the same `icu_experimental` exposure).

**Evidence**: `plugins/i18n/src/fmt/currency.rs`'s module doc and `ZERO_DIGIT`/`THREE_DIGIT`
tables; `plugins/i18n/src/fmt/number.rs`'s module doc ("percent rides `icu_experimental`, like
currency").

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

**Evidence**: `plugins/i18n/src/engine/resolve.rs`'s chain walk (lines 50-75, first owning
bundle's `entry.bundle` is formatted); `plugins/i18n/src/fmt/fluent_fns.rs` (lines 77-88,
`with_icu_functions` closure captures locale at bundle-build time); review
finding R2-M4.

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
figure. Accepted by Ed 2026-08-15.

**Opt-out**: `default-features = false` in `frust-i18n`'s dependency stanza (app's
`Cargo.toml` or workspace root). Apps that skip the feature retain the direct API's
`NUMBER()`/`DATETIME()` calls (subject to `i18n-inmessage-format-locale`'s constraints)
but lose in-message formatting entirely and gain no binary savings (Fluent's own parser
does not become conditional).

**Evidence**: `plugins/i18n/README.md` §10 (size measurement procedure and figures);
`plugins/i18n/Cargo.toml`'s default feature list and the `formatting` feature's own
dependencies.

---

### `desktop-shells-runtime-unverified` — the Linux desktop shell's Wayland session is unverified

**Observed**: Phase A's `frust-shell-macos`/`-windows`/`-linux` (menu bar, lifecycle, window
icon/identity) were built and integrated from a headless Linux host — proven only via cross-target
`cargo check` (Windows: `x86_64-pc-windows-gnu`; macOS: `aarch64-apple-darwin`, both host-side
where the toolchain exists and via a containerized `rust:1-bookworm` + `mingw-w64` recipe; Linux:
native `cargo check --workspace` here). **macOS** is no longer in this gap: the MacBook runtime
gate (2026-08-15, `macbook-gate-r1`, 15/15 checks passed) launched real windows and verified the
menu bar with the app-named items, ⌘Q via both the menu and the accelerator, Hide/Show All,
close-then-Dock-click reopen from the inactive state, and the zero-config preview — all with no
functional bugs found. **Windows** is no longer in this gap either: the Windows 11 gate
(2026-08-19, `examples/shadcn-demo` built natively with MSVC on hardware) verified the
titlebar+taskbar icon, the `AppUserModelID` taskbar identity, the dark titlebar following the app
theme, native menu-bar activation delivery, a declared accelerator firing its item (proven by
injected-keystroke probe against the live window), and quit — and caught two real defects, both
fixed and re-verified in the same round: the quit role dead-ended in muda's `PostQuitMessage` (now
an owned item the handler maps to `WM_CLOSE`), and submenu accelerators never entered the `HACCEL`
because items were appended before the submenu was attached (build order is load-bearing; both
contracts are documented at their `frust-shell-windows/src/menu.rs` sites). **Linux X11** closed on
2026-09-13 (KDE on Xorg, NVIDIA T400): `WM_CLASS` matched the app id, the `.desktop` entry paired
via `StartupWMClass` (KDE task manager), `_NET_WM_ICON` rendered in the titlebar and taskbar, and
titlebar-close exited 0. Only a **Wayland** session (the `app_id` half) remains open.

**Applies to**: a Phase A desktop shell app under Wayland, until the `app_id` pairing is verified.

**Why accepted**: the closed macOS, Windows, and Linux X11 gates cover the native shell call sites
this crate owns, each read against vendored source (muda 0.19.3, winit 0.30.13, windows-sys 0.61.2,
objc2/objc2-app-kit 0.3.x) rather than guessed; only a Wayland session — a distinct winit backend
for the `app_id` identity path — has not yet run.

**Evidence**: desktop-shells Phase A tasks 02/03/04/05/06 completion summaries (Risks/Limitations
sections); the cross-target build matrix; macOS runtime verification on a MacBook
(window, menu, dock and quit behavior); Windows runtime verification — the 2026-08-19 gate, landed with the menu-quit
and accelerator-registration fixes that narrowed this entry; Linux X11 verification — the
2026-09-13 gate (KDE on Xorg, NVIDIA T400).

---

### `desktop-windows-titlebar-theme-revert` — a Windows titlebar override can be silently reverted between app brightness changes

**Observed**: winit 0.30.13's `Window::set_theme` never writes back its own `preferred_theme`
field (a winit bug), so its `WM_SETTINGCHANGE` handler — which branches on that field being `None`
to mean "no app override, follow the system" — treats an app-forced titlebar the same as an
unthemed one and can silently re-apply the *system* theme over it, without necessarily firing
`WindowEvent::ThemeChanged`. `frust-shell-windows`'s round-1 fix (`theme.rs`'s `TitlebarTheme`)
removes the `applied == wanted` latch so every core-signaled brightness change re-issues
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

**Evidence**: `crates/frust-shell-windows/src/theme.rs` module docs ("The residual gap this round
does not close"); desktop-shells Phase A fix-round-1 task F2.

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
available substitute. Flagged by the implementor for reviewer confirmation, closed by the round-1
review, and the gap's exact shape was then confirmed live on real hardware.

**Evidence**: desktop-shells Phase A task 03 completion summary (Notable Decisions #3,
Risks/Limitations #3). Runtime confirmation on a MacBook: a
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
Flagged by the implementor for reviewer confirmation, closed by the round-1 review, and both
routes were then observed live on real hardware. A later fix would route termination through
`appkit_glue`'s `applicationShouldTerminate:` (`NSApplication.TerminateReply.terminateLater`, run
the graceful shutdown, then `terminateNow`) instead of the current predefined action.

**Evidence**: desktop-shells Phase A task 03 completion summary (Notable Decisions #2);
`plugins/camera/src/apple.rs` (`AppleSession`'s `Drop`, module doc's macOS-shares-the-Apple-arm
note). Runtime confirmation on a MacBook:
the `terminate:` route (menu Quit and ⌘Q, including while the window was hidden) exits without
`run` returning; the close-window route returns from `run` cleanly (exit code 0).

---

### `desktop-macos-app-menu-title-process-name` — the bold application-menu title shows the process name, not `DesktopConfig::app_name`, for unbundled binaries

**Observed**: AppKit derives the bold application-menu *title* in the menu bar from the
bundle/process name and ignores the installed `NSMenu` item's title for unbundled binaries. A
`cargo run`-style dev binary named "Frust Gate" via `DesktopConfig` shows `frust-gate` (the
process name) as the bold menu title, while every item label inside it (About/Hide/Quit Frust
Gate) correctly carries the configured name. Verified live on the MacBook runtime gate
(`macbook-gate-r1`, finding F-2).

**Applies to**: unbundled macOS dev-preview launches (`cargo run` / `frust run` fallback) using a
Phase A desktop shell.

**Why accepted**: no public API sets the application-menu title for an unbundled process — it is
sourced from the process/bundle name, not from any NSMenu item. Phase B's `.app` bundle assembly
ships `CFBundleName` (from `DesktopConfig::display_name`) in every `Info.plist`, and the MacBook
runtime gate r2 (2026-08-17) verified the fix live: a `frust build macos` bundle's bold menu
title reads the display name ("Gate App"), closing r1's F-2 for bundles. The shell's default
About/Hide/Quit item labels follow the same name — `resolve_app_name` prefers the bundle's
`CFBundleDisplayName`/`CFBundleName` over the executable stem when no `DesktopConfig::app_name`
is set (gate r2 finding F-4). Only the unbundled bold title remains process-named, which is
AppKit's own behavior.

**Evidence**: MacBook runtime verification of the window title and application-menu name;
`crates/frust-shell-macos/src/lib.rs` (`resolve_app_name`), `src/appkit_glue.rs`
(`bundle_display_name`).

---

### `desktop-installer-runtime-partial` — installer formats are runtime-verified only where hardware and host tooling allow

**Observed**: `frust build <os> --installer` shells out to a pinned `cargo-packager` over an
already-assembled bundle. Runtime status per format, this run: **Linux `.deb`** — built and
verified for real on this host (`file` confirmed a genuine Debian binary package). **Linux
`.AppImage`** — the typed refusal path (`InstallerError::PackagerFailed`) was proven for real, but
the format itself did not build on this host: `cargo-packager`'s vendored `linuxdeploy`'s `strip`
step rejects a relocation section (`.relr.dyn`) this host's newer `binutils` emits — an
upstream/environment tool mismatch, not a Frust defect. **macOS `.dmg`** — built and verified for real on the MacBook
(gate r2, 2026-08-17; Developer-ID/notarization checks still owed — detailed below).
**Windows NSIS `.exe`/WiX `.msi`** — owed to the Windows 11 PC pass (unit-tested
against `FakeProcessRunner` only). **`.rpm`** is not a supported output at all —
`cargo-packager` 0.11 builds only `.deb`/AppImage/pacman on Linux, so a requested `.rpm` is a typed
`InstallerError::RpmNotSupported` refusal naming `tauri-bundler`'s documented `.rpm` path (or
`alien` over the `.deb`) as the workaround, never a silent failure. Separately, **AppImage requires
a square configured icon**: an absent or non-square `[desktop] icon` surfaces as `cargo-packager`'s
own typed refusal ("Could not find a square icon to use as AppImage icon") rather than a bundle
with no icon (unlike a plain `frust build linux`, where a bad icon only downgrades to a
`BundleNote`). **macOS `.dmg` identity/signing** — `PackagerConfig`'s `macos` block (Dmg-only) now
passes the already-assembled bundle's `Contents/Info.plist` path and the entitlements path (when
`macos/app.entitlements` exists) into cargo-packager's own config; verified against the pinned
`cargo-packager` 0.11.8 source that its `Dmg` arm's synthesized `.app` overlays every key from the
named plist onto its own generated one. `signingIdentity` is forwarded only when `[macos]
signing-identity` is Apple-issued (the same prefix list `codesign` uses below); a non-Apple
identity is suppressed instead, with a typed `InstallerNote::PackagerSigningSkipped` (rendered by
the CLI as `Note (dmg): ...`) — cargo-packager's own codesign pass always adds `--timestamp`
unconditionally (verified against the pinned source), which needs network access, so forwarding a
local/self-signed identity would make `--installer dmg` depend on the network. Suppressing it keeps
signing offline end-to-end; because the packager reassembles the bundle (which would break the
suppressed identity's binary signature — gate r2 finding F-6), the packager is handed a
signature-stripped copy of the executable (`stage_unsigned_binary`), so the `.dmg`'s inner `.app`
ships cleanly unsigned; the assembled bundle's own `codesign` pass (below) is unaffected. The `icons` entry for `.dmg` is likewise the assembled
bundle's own generated `<binary_name>.icns`, not the source PNG every other format uses —
cargo-packager copies an `.icns` input verbatim, keeping its file name, so the merged plist's
`CFBundleIconFile` (which names the binary, not the display name) resolves; a bundle with no
generated `.icns` gets no `icons` entry for `.dmg` at all. The MacBook runtime gate r2
(2026-08-17) verified the `.dmg` lane for real with an `Apple Development` identity: dmg built via
the pinned packager, mounted, inner `.app` carrying the overlaid plist identity keys and a
plugin-contributed key + entitlement, volume icon present, and the credential scrub proven with
`APPLE_*` variables exported. What is still **owed**: the same run under a real `Developer ID
Application` identity (none in the build keychain) — the packager's own re-sign of the inner
`.app`, its secure timestamp, the pre-notarization `spctl` "rejected" verdict — plus a
`notarize = true` run with real Apple credentials, confirming the submission reaches `notarytool`
and either succeeds or surfaces as the hard build failure this contract describes.
**codesign flags** —
whenever a `[macos] signing-identity` is configured, `desktop_build::macos::codesign` always passes
`--options runtime` (Hardened Runtime; harmless for an ad-hoc signature, mandatory for notarization
eligibility) and passes `--timestamp` only when the identity carries one of the Apple-issued
prefixes (`Developer ID Application:`, `Apple Distribution:`, `Apple Development:`,
`Mac Developer:`, `3rd Party Mac Developer Application:` — the development prefixes joined the
list after gate r2's F-6, where classifying them as local shipped a `.dmg` whose inner app had an
invalid signature) — an ad-hoc/self-signed identity's codesign call therefore stays fully offline.
Real `Developer ID` `--timestamp` behavior remains unverified on hardware. **Notarization** (Apple's separate `notarytool` submission + stapling,
needed for a `.dmg` to run without a Gatekeeper warning on a machine that didn't build it) is
opt-in and, by default, unreachable: `cargo-packager` falls through to notarization on its own
whenever it can read Apple credentials from its environment, so `frust build --installer` removes
all nine `APPLE_*` credential variables (`APPLE_KEYCHAIN_PROFILE`; `APPLE_ID`/`APPLE_PASSWORD`/
`APPLE_TEAM_ID`; `APPLE_API_KEY`/`APPLE_API_ISSUER`/`APPLE_API_KEY_PATH`; `APPLE_CERTIFICATE`/
`APPLE_CERTIFICATE_PASSWORD` —
`desktop_build::installer::APPLE_CREDENTIAL_ENV_VARS`) from the packaging child's environment
unless `[macos] notarize = true` opts in, recorded either way by a typed `InstallerNote`
(`NotarizationSuppressed`/`NotarizationEnabled`). Opted in, the credentials pass through and
`cargo-packager` runs `xcrun notarytool submit --wait` against the signed `.app`: credentials that
fail to resolve only warn and skip (never fail the build), but a resolved submission Apple rejects,
or that fails to complete, is a hard installer error. Both notes fire only when the config really
reaches `cargo-packager`'s notarization branch (Dmg + an Apple-issued identity).

**Applies to**: `frust build <os> --installer`, and any macOS `.dmg` distributed outside the
building machine.

**Why accepted**: matches Phase A's device-availability constraint carried into Phase B — the
macOS half closed on the MacBook at gate r2 up to the identities that keychain holds; Windows
installer assembly stays proven by unit test against a faked process runner, the same asymmetry
`desktop-shells-runtime-unverified` already accepts for the shells themselves. The Linux `.AppImage` gap is a host tool-version mismatch with a
proven typed-error path, not an unverified code path — closing it needs either a host with an
older/compatible `binutils` or an upstream `linuxdeploy` fix, neither of which this pipeline
controls. Notarization is wired and opt-in, but exercising it needs a real Apple Developer
session this environment has no MacBook or account to run — verification, not automation, is
what's owed.

**Evidence**: desktop-shells Phase B task 05 (`05-installers-doctor`) and task 07
(`07-cli-build-targets`) completion summaries (Testing Performed — real Linux smoke: genuine `.deb`
verified via `file`, `.AppImage` failure tail); `crates/frust-drive/src/desktop_build/installer.rs`
(`InstallerFormat::for_target`, `RpmNotSupported`, `PackagerMacosConfig`, `InstallerNote`,
`generated_icns`); for the credential-scrub-by-default + `notarize` opt-in contract:
`crates/frust-drive/src/desktop_build/installer.rs` (`APPLE_CREDENTIAL_ENV_VARS`,
`InstallerNote::NotarizationSuppressed`/`NotarizationEnabled`), `crates/frust-drive/src/manifest.rs`
(`MacosSection::notarize`/`notarize_enabled`), `crates/frust-drive/src/process.rs` (`ProcessRunner`'s
`remove_env` support); `crates/frust-drive/src/desktop_build/macos.rs` (`codesign`,
`APPLE_ISSUED_IDENTITY_PREFIXES`/`is_apple_issued_identity`).

---

### `desktop-single-window` — desktop shells support exactly one window

**Observed**: `frust-shell-macos`/`-windows`/`-linux` and the shared `frust-shell-desktop` core
create and manage a single `winit::window::Window`; there is no multi-window API at the facade or
any per-OS shell.

**Applies to**: every desktop shell, on every OS.

**Why accepted**: matches the pre-existing desktop preview's scope ("like today"); a deliberate
Phase A non-goal, not a defect — revisit only if a future winit brings first-class multi-window
support worth exposing through `DesktopConfig`.

**Evidence**: the desktop-shells scope list for menus, deep links and lifecycle states that
multi-window stays out of scope (single window, like today).

---

### `paths-macos-legacy-fallback-runtime-unverified` — two of the three macOS legacy read-through sites have no runtime exercise

**Observed**: of the three macOS legacy-XDG read-through sites, only `plugins/database`'s
file-level one (`resolve_db_path_from`, the sole live caller with durable data at stake) has run
against a real `~/Library` tree — the MacBook runtime gate r2 (2026-08-17) verified fresh-create
under `~/Library/Application Support`, the read-through opening a planted legacy
`~/.local/share/databases/<name>.db` in place (nothing migrated, copied, or deleted), and
new-wins with both present. The other two remain unit-tested only: `frust-paths`' **built-in**
per-app-stem probe (`macos_dir_from`'s `<base>/<app_stem>` check) has no in-repo caller — it
serves only a direct-API app doing its own `app_stem()`-shaped layout — and
`frust-shell-desktop`'s pipeline-cache read-through (`load_path`, via
`frust_paths::legacy_cache_dir`) is a no-op on macOS today regardless (Metal has no wgpu
`PIPELINE_CACHE` feature, so a save never fires to populate either base; gate r2 confirmed no
cache file appears). `shared-preferences` is inert here — its macOS backend is `NSUserDefaults`,
so its `file` backend never runs.

**Why accepted**: the two unexercised sites have, respectively, no caller and no reachable save
path — runtime proof would require building a caller that does not exist. Unit tests against an
injectable existence probe plus the darwin cross-target gates cover the logic itself.

**Evidence**: `crates/frust-paths/src/lib.rs` (`macos_dir_from`, `legacy_data_dir`/
`legacy_cache_dir`); `plugins/database/src/lib.rs` (`resolve_db_path_from`);
`crates/frust-shell-desktop/src/cache.rs` (`load_path`);
MacBook runtime verification of the data and cache directory resolution.

---

### `desktop-contributions-v1-scope` — three named scope limits on the plugin desktop-contribution seam

**Observed**: three deliberate v1 narrowings on the desktop-contribution feature (Phase C), each
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
spec'd speculatively ahead of one. The registry carries zero desktop-lane contributions today, so
none of the three has blocked a real plugin yet.

**Evidence**: `crates/frust-drive/src/desktop_build/contributions.rs`'s `applies_to` (Windows
match arm); `crates/frust-drive/src/plugin/mod.rs`'s `Contribution::MacosEntitlement` doc comment;
`crates/frust-drive/src/plugin/apply.rs`'s `desktop_contributions` doc comment; desktop-shells
Phase C task 03 completion summary, Doc Updates Needed.

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

Scope, post review-round-0 M2 fix: the ladder's step-3 (`Typeface::System`) arm used to carry the
bundled Glyph bytes along even when `System` was selected ("so the other slot's publish doesn't
blank them"), which meant a plain Material3/Cupertino theme with **no** `NativeTypefaces`
extension published the non-empty pair `(Space Mono, IBM Plex Mono)` on its very first resolve —
so *any* app that resolved a native control under an extension-less theme before installing a
design system would latch Glyph's faces regardless of whether a real swap ever happened. That arm
now publishes no bytes (`&[]`); an all-empty pair never crosses the platform seam
(`publish_font_bytes`'s own skip), so an extension-less resolve latches nothing and a
subsequently installed design system's pair becomes the process's first real publish and
registers correctly. The residual gap this entry now covers is therefore narrower and genuinely
platform-side only: a **mid-process swap from one real face pair to another** (e.g. one design
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

### `refusal-banner-text-device-unverified` — the refusal banner's warning text has never rendered on real Android/iOS hardware

**Observed**: `plugins/native-widgets/src/api/builders.rs`'s `placeholder()` — the frust-drawn
fallback all six builders (`native_button`/`native_label`/`native_switch`/`native_slider`/
`native_progress`/`native_image`) degrade to under `ResolvedSurfaceMode::RefusedTranslucent` —
shapes its visible label/description prose through a crate-local `BannerText` helper built on
`frust::authoring::text::{TextContext, TextLayout, TextStyle}` (system-UI font, not a
design-system typeface — no design-system catalog is reachable from this plugin, the same charter
the rest of the banner follows). Host-side tests prove the path non-vacuously:
`placeholder_paints_visible_text_within_its_slot_rect_at_every_slot_size` asserts at least two
glyph runs paint (label and description) at each of `native_switch`'s 70x40, `native_progress`'s
260x24, `native_button`'s 160x48, and a deliberately tiny 40x16 slot, using a paint scene
(`BoundsRecorder`) that actually honours the clip stack — remove `ClipToSlot`'s `push_clip` and a
run's raw bounds would escape the slot rect and fail the assertion, rather than trivially passing
by construction. None of this has run through an on-device or simulator Android/iOS text-shaping
stack: font metrics, DPI, and native text shaping can all differ from the desktop `TextContext`
these tests exercise.

**Why accepted**: the fallback only paints under `ResolvedSurfaceMode::RefusedTranslucent`, itself
reached only when the platform's own compositor offers no translucent alpha mode for the surface
frust requested — an on-demand path, not mainline rendering — so a device gate
confirming the warning text actually renders and clips on real hardware is owed, not blocking.
Rides alongside `native-typeface-first-publish-latch`'s own owed device gate (above) for the same
plugin's typeface ladder — both are NATIVE_WIDGETS text-rendering paths awaiting real Android/iOS
hardware.

**Evidence**: `plugins/native-widgets/src/api/builders.rs`'s
`placeholder_paints_visible_text_within_its_slot_rect_at_every_slot_size` and
`a_refused_slot_still_publishes_no_platform_view_frame_through_the_clip_wrapper` tests (`cargo test
-p native-widgets`, host-only).

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

### `native-widgets-segmented-stepper-apple-only` — `native_segmented`/`native_stepper` exist on iOS and macOS only, and decision D2 refuses `native_tab_bar`/`show_native_sheet` on Android too

**Observed**: `native_segmented` and `native_stepper` are registered in `APPLE_KINDS`
(`plugins/native-widgets/src/controls/mod.rs`), not `SHARED_KINDS` — neither control has an
Android `NativeWidget` impl, or a Linux/Windows/web host one either. Each app-facing builder
resolves this at compile time: `NativeSegmentedView`/`NativeStepperView`'s `SEGMENTED_ARM`/
`STEPPER_ARM` constants (`plugins/native-widgets/src/api/builders.rs`) are `true` only under
`cfg(any(target_os = "ios", target_os = "macos"))`; everywhere else the builder renders the
frust-drawn `RefusalBanner` placeholder instead of an empty slot, naming the missing arm. Android's
framework has no segmented control (the stock option, `MaterialButtonToggleGroup`, would impose an
AndroidX/Material dependency this chartered leaf plugin never assumes an app added) and no
increment/decrement stepper control either.

`native_tab_bar` is the same decision, one arm narrower: it is registered in `IOS_ONLY_KINDS`, not
`APPLE_KINDS` (macOS has no bottom-tab-bar idiom either —
`plugins/native-widgets/src/controls/tab_bar.rs`'s module doc, *No macOS arm, no Android arm
(decision D2)*), and `NativeTabBarView`'s own compile-time gate (`TAB_BAR_ARM`,
`plugins/native-widgets/src/api/builders.rs`) renders the same `RefusalBanner` everywhere but iOS,
naming both the missing idiom and decision D2. `show_native_sheet`/`show_native_sheet_into` refuse
for the same reason at runtime instead of compile time: the sheet arm exists only on iOS
(`present::apple_sheet`), and every other target — Android included — resolves through
`present::mod.rs`'s own `UnsupportedSheet`, answering `PresentError::Unsupported`.

**Why accepted**: a Material-backed segmented Android arm and a composite stepper Android arm (two
`ImageButton`s plus a `TextView`) are both named follow-up plans, not v1 scope — building either
means taking on the Material/AndroidX dependency this plugin's leaf-plugin charter
(`docs/PLUGINS_CODE_STANDARDS.md`) currently avoids entirely. No Linux/Windows/web arm exists for
either control either; the compile-time refusal covers every non-Apple target uniformly. The same
reasoning covers `native_tab_bar` (no `BottomNavigationView` dependency either) and the sheet arm
(no `BottomSheetDialog`/`NSPopover` arm yet — `present/mod.rs`'s module doc's *Platform arms*
names both as follow-up work).

**Evidence**: `plugins/native-widgets/src/controls/mod.rs`'s `SHARED_KINDS`/`APPLE_KINDS`/
`IOS_ONLY_KINDS` tables and their source-scanning parity test;
`plugins/native-widgets/src/controls/segmented.rs`'s and
`stepper.rs`'s module docs ("No Android arm (decision D2)" / "No Android arm (the same shape as
decision D2)"); `plugins/native-widgets/src/controls/tab_bar.rs`'s module doc, *No macOS arm, no
Android arm (decision D2)*; `plugins/native-widgets/src/api/builders.rs`'s `SEGMENTED_ARM`/
`STEPPER_ARM`/`TAB_BAR_ARM` compile-time constants and `banner_placeholder`;
`plugins/native-widgets/src/present/mod.rs`'s `UnsupportedSheet` and its module doc's *Platform
arms* section; `plugins/native-widgets/README.md`'s "Eleven controls" table.

---

### `native-tab-bar-bare-no-liquid-glass` — `native_tab_bar` is a bare `UITabBar`, so it never gets iPadOS 18+'s top placement or the Liquid Glass scroll-edge effect

**Observed**: `native_tab_bar` builds and drives a bare `UITabBar` directly from Rust —
deliberately never a `UITabBarController` (decision D5,
`plugins/native-widgets/src/controls/tab_bar.rs`'s module doc, *A bare bar, never a
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
and navigation ownership frust's own `Router` already owns (module doc's decision D5) — the whole
reason this control is a bare bar in the first place. Nothing about the bare-bar shape can be
patched onto the controller-only chrome without reversing that decision.

**Evidence**: `plugins/native-widgets/src/controls/tab_bar.rs`'s module doc, *A bare bar, never a
`UITabBarController` (decision D5)*; `plugins/native-widgets/README.md`'s tab-bar paragraph ("On
iPadOS the bare bar stays at the bottom and gets no Liquid Glass — the iPadOS 18 top tab bar and
the floating glass bar are `UITabBarController` features");
`plugins/native-widgets/src/apple/events.rs`'s `UITabBarDelegate`/`tabBar:didSelectItem:` wiring.

---

### `native-keyboard-focus-via-full-keyboard-access` — a hosted iOS control is keyboard-focusable only when Full Keyboard Access is on

**Observed**: every standard UIKit control this plugin hosts
(`plugins/native-widgets/src/apple/factory.rs`'s `createView` builds each one straight from
`objc2-ui-kit` — `UIButton`/`UILabel`/`UISwitch`/`UISlider`/`UIProgressView`/`UIImageView`/
`UIActivityIndicatorView`/`UIDatePicker`/`UISegmentedControl`/`UIStepper`/`UITabBar`) is left
exactly as UIKit vends it: the plugin does not override `canBecomeFocused` (or any other
`UIFocusEnvironment`/`UIFocusItem` method) anywhere in `apple/factory.rs` or any
`src/controls/*.rs` platform arm. UIKit's own default is that a standard control answers
`canBecomeFocused` `true` — and so becomes reachable by a hardware keyboard's Tab traversal, a
Made-for-iPhone game controller's D-pad, or Switch Control — only when the user has Full Keyboard
Access on (Settings → Accessibility → Keyboards → Full Keyboard Access, or a connected keyboard's
own toggle); it is off by default. Nothing in this crate's create/update/dispose path asks about or
changes that setting.

**Applies to**: every control this plugin hosts on iOS/iPadOS when Full Keyboard Access is off —
the platform default. macOS's AppKit arm is a separate focus model this entry does not cover.

**Why accepted**: this is UIKit's own accessibility contract, not a gap this plugin introduces —
`canBecomeFocused`'s Full-Keyboard-Access gate is Apple's, and this plugin's controls behave
exactly like the same controls in a plain UIKit app that never overrides it either.

**Evidence**: absence, honestly cited — `plugins/native-widgets/src/apple/factory.rs` and every
`plugins/native-widgets/src/controls/*.rs` platform arm carry no `canBecomeFocused` override, no
`becomeFirstResponder` override, and no `UIFocus*` conformance anywhere in the crate (`git grep
canBecomeFocused` under `plugins/native-widgets/src` finds nothing on this base).

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

A mid-interactive-dismissal race is accepted rather than closed: a successor request arriving while the user's swipe is still animating a displaced sheet away resolves it at once (`DisplacedAction::ResolveAndContinue`, `apple_host.rs`) without waiting for that animation, so the successor's own presentation attempt can find UIKit still mid-transition and resolve `PresentError::NoHost` instead of showing; and if the user's swipe is then cancelled, the old sheet is back on screen with no live entry to answer it — removable only by another swipe, not by a further programmatic `dismiss` — accepted, a Phase 7 device-gate observable.

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

### `semantics-untestable-out-of-tree` — an out-of-tree design system can implement `Widget::semantics` but cannot test it

**Observed**: `examples/design-system-sample`'s widgets each carry a `semantics` impl, and the
vocabulary to write one is fully public (`SemanticsCtx::push_node`/`push_container`, `Role`,
`Node`, `Action`, `ChildPod::semantics_child`) — but nothing can *drive* a semantics pass from
outside the framework: `SemanticsCtx::new` is `pub(crate)` in `frust-core`, and the only public
producer, `frust_core::RenderRoot::semantics()`, is not re-exported by the `frust` facade. The
three built-in design-system plugins (`frust-glyph`/`frust-material`/`frust-cupertino`) are
unaffected despite living outside `frust-widgets` now — each reaches `RenderRoot` through a
sanctioned `frust-core` test-only dev-dependency, the same plugin-tier exemption
`docs/PLUGINS_CODE_STANDARDS.md` records for the app-tier "no direct `frust-core` dependency" rule
(`docs/CODE_STANDARDS.md`), not available to a genuinely external crate like `design-system-sample`.

**Applies to**: any external design-system crate wanting to unit-test its `Widget::semantics`
output.

**Why accepted**: closing it means either re-exporting a semantics-pass entry point from the
facade or loosening `SemanticsCtx::new`'s visibility — a deliberate facade-API decision deferred
to the feature owner, not a gap this task's scope covers.

**Evidence**: `examples/design-system-sample/sample-design/src/testing.rs`'s module doc ("Known
gap: `Widget::semantics` cannot be driven from out of tree") and the workspace README's "What it
found" section; `crates/frust-widgets/tests/semantics_tree.rs` as the in-tree contrast that does
reach `RenderRoot::semantics()`.

---

### `hover-window-leave-standing` — hover and press chrome outlive the state that caused them, until the next in-window `Move`

**Observed**: four deliberate, test-pinned v1 gaps share one cause — nothing re-derives hover
without a fresh uncaptured pointer `Move`, and the desktop shell delivers no window-leave event.
A pointer leaving the window keeps the last-hovered widget's tint standing (no `CursorLeft` is
ever dispatched to clear it); a `Down` that ends a hover with the pointer then held still shows no
tint for the press itself until the next `Move`; a click's `Up` likewise ends the link, so a mouse
resting where it clicked shows no hover tint until it moves again; and a scroll or any other
mutation happening under a stationary pointer leaves stale hover/pressed chrome in place rather
than re-testing the pointer's position against whatever moved. All four resolve themselves on the
next in-window pointer `Move`, which is what makes them a standing-until-next-move gap rather than
a stuck one.

**Applies to**: every desktop shell (`frust-shell-desktop`); any widget using
`EventCtx::claim_hover`/`PaintCtx::is_hovered`. **Mobile is affected too, in one shape**: nothing
in the pipeline distinguishes a touch contact from a mouse (`PointerEvent` carries no pointer
kind, and both mobile shells map a touch drag to `PointerPhase::Move`), so the hover refusal is
structural only for a **captured** pointer. A touch drag that captured nothing — a finger sliding
over a non-capturing hover consumer — is an ordinary hover pass and does tint it. The tint is
transient: the lift's `Up` ends the link, which is why `Up` is a hover-ending pass at all.

**Why accepted**: hover is deliberately derived only from a real pointer `Move`, with no
synthetic re-derivation pass, because the alternative (a per-frame position re-test independent of
input) would give hover its own polling loop the rest of the pipeline doesn't have. A window-leave
event is a real, addressable gap (winit exposes `CursorLeft`) but scoped out of this phase; the
rest are accepted properties of the epoch model, not shell gaps. The touch residual is accepted
over the alternative of a pointer-kind field on `PointerEvent`, which would widen the input
vocabulary (and every out-of-tree exhaustive match on it) to suppress a transient tint that the
`Up` rule already bounds to the length of the gesture.

**Owed device check** (Android + iOS, not yet run): drag a finger across a non-capturing hover
consumer (a `material::list_item` row with `on_press` is the reference; drag from empty chrome onto
it so nothing captures) and confirm the 8% overlay appears under the finger *and* is gone by the
frame after lift — i.e. the residual is transient, never a tint stranded on a touch-only screen.

**Evidence**: `crates/frust-core/src/app.rs`'s hover pipeline tests (`a_captured_move_cannot_claim_hover`,
`a_down_up_or_cancel_ends_the_hover`, `a_non_pointer_pass_leaves_a_live_hover_standing`);
the hover pipeline (2026-08-17, commit `68ac7e93`) listed this among its known v1 gaps; the touch residual and
the `Up` rule were traced in review the same day.

---

### `cursor-desktop-runtime-unverified` — the desktop cursor-apply path has no live-window run

**Observed**: `frust_core::CursorIcon` resolution and the winit mapping/change-gate
(`winit_cursor_for`, `cursor_change_to_apply`, `ShellHandler::sync_cursor`) are unit- and
compile-verified only — no session has opened a real desktop window and watched the platform
pointer actually change shape over a widget requesting one.

**Applies to**: every desktop shell (macOS, Windows, Linux) via `frust-shell-desktop`'s shared
core.

**Why accepted**: the mapping is exhaustive over `CursorIcon` and the change gate is a pure
function, both covered by host-run unit tests; a live-window pass is an ordinary runtime-
verification gap of the same shape already tracked for the rest of the desktop tier (see
`desktop-shells-runtime-unverified`), not a defect specific to cursor.

**Evidence**: `crates/frust-shell-desktop/src/app_handler.rs`'s
`every_framework_cursor_maps_to_its_winit_counterpart`/`the_cursor_is_pushed_to_winit_only_on_a_change`
tests (cursor API merged 2026-08-17, `49bc7657`).

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

**Evidence**: `crates/frust-core/src/app.rs`'s `RenderRoot::cursor()` doc comment ("Residual:
a widget that is torn down … leaves the last shape in place until the next `Move`");
the cursor API (merged 2026-08-17, `49bc7657`) listed this among its limitations.

---

### `overlay-portal-v1-scope` — six named narrowings in the framework overlay portal

**Observed**: `frust::overlay_portal` and `frust::authoring::OverlaySlot` float a pod above the
whole app — owner-hosted, root-painted after the main tree, root-routed ahead of it — and the v1
seam declines six things, each named in its own source:

1. **No focus trap.** Focus inside a pod behaves exactly like focus anywhere else; nothing confines
   traversal to the pod or restores it on dismiss.
2. **No declined-key forwarding.** Key, IME and edit-command events stay focus-routed and are never
   re-offered to an overlay owner that did not take focus, so a pod cannot hand a key it declined
   to a sibling panel that would have taken it.
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
   floated surface costs a full tree walk.
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
the baseline `TextInput`'s selection toolbar, `frust_shadcn::tooltip`/`hover_card`,
`frust_beui::tooltip`, and `frust_material::tooltip`/`rich_tooltip`. Item (3) applies to the three
catalogs' own hosts, which is where most floated surfaces still live.

**Why accepted**: each is a seam the first callers do not need, and each is cheaper to add once a
second caller states its shape than to guess at now. (3) in particular is migration work with no
behaviour riding on it — the catalog hosts work, and moving them is a port rather than a fix.
(5) is a real cost rather than a correctness gap: the walk happens per overlay pointer event, not
per frame, and the broadcast is the only route that reaches an owner without knowing where it sits.

**Evidence**: `crates/frust-core/src/overlay.rs`'s "Not in v1" section (focus trap, declined keys,
inspection, nesting) and its per-pass registry contract; `crates/frust-widgets/src/overlay.rs`'s
own "Not in v1" section (semantics, hover, IME); `crates/frust-core/src/event.rs`'s
`InputEvent::Overlay` routing contract (the broadcast); `plugins/shadcn/src/components/tooltip.rs`,
`plugins/beui/src/components/tooltip.rs`, and `plugins/material/src/tooltip.rs` module docs (all
titled "Riding the framework portal") against the six unported
`plugins/{shadcn,material,beui}/src/overlay/{anchored,modal}.rs` hosts;
`plugins/material/src/tooltip.rs`'s `TooltipWidget`/`RichTooltipWidget` `semantics` methods and
their pre-port use of `crate::overlay::anchored` (for item (4)'s regression above).

**Trigger for removal**: per item — a focus-trap seam, a declined-key route, the catalog hosts
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
unaffected: it selects `SelectionToolbarPolicy::Native` and gets UIKit's own handles and loupe
(see `ios-native-edit-menu-device-status` for that route's own status).

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
the pointer) that this round does not add.

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

**Evidence**: `crates/frust-core/src/event.rs`'s `FileDropEvent` "Source" and "Broadcast follow-up"
doc sections; `crates/frust-widgets/src/drag/target.rs`'s "OS file drops" module-doc section ("The
drag always ends", "Known limit"); `crates/frust-shell-desktop/src/app_handler.rs`'s
`FileDropAccumulator`, `FileHoverLatch` and the
`WindowEvent::HoveredFile`/`DroppedFile`/`HoveredFileCancelled` arms.

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

### `selection-toolbar-mouse-hold-opens` — a held mouse press opens the selection toolbar, because nothing says it is a mouse

**Observed**: `PointerEvent` carries a phase, a position and a button, and no pointer *kind* — the
framework cannot tell a finger from a mouse or a pen anywhere in the tree. The text field's
long-press therefore arms on any primary `Down`, so holding a mouse button still inside a field for
the long-press threshold selects the word under it and raises the toolbar, which no desktop
platform does. The desktop gesture actually meant to open it (a secondary press) works as well:
this is an extra route, not a missing one.

**Applies to**: desktop and any mouse-driven host on the framework route. The same blindness makes
every other press-and-hold in the tree fire for a mouse; the text field is where it is most
visible, because it is the gesture that opens a menu.

**Why accepted**: adding a kind to `PointerEvent` changes the one type every widget's event handler
destructures, and each shell would have to source it (winit distinguishes touch from mouse, the
browser has `pointerType`, both mobile shells synthesize their own) — a change worth making
deliberately rather than as a side effect of one field's gesture. The wrong behaviour here costs a
selection the user can dismiss, never an edit.

**Evidence**: `crates/frust-core/src/event.rs`'s `PointerEvent` (three fields, no kind);
`crates/frust-widgets/src/textinput.rs`'s "Selection gestures and the toolbar" section (the
long-press arms on a primary `Down`, with no kind consulted).

**Trigger for removal**: a pointer-kind axis on `PointerEvent`, sourced by every shell, after which
the long-press gates on touch.

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
`crates/frust-core/src/app.rs`'s `RenderRoot::perform_accessibility_action` (a `Click` arm, a
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
originated it, `frust_material::overlay::anchored` (a Phase-3 sibling port, ported rather than
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

**Applies to**: `frust_shadcn::table` and `button` respectively. `input_otp` is not among them —
it accepts `EditCommand::Paste` and decodes the paste chord itself, so a multi-character paste
fills its slots.

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
  fragment program this task did not write. The five that *are* ported are semantic
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
  larger surface the porting card scoped out).
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

**Observed**: `PaintScene` publishes no blur filter a widget can reach (`frust-engine` renders
Gaussian blur/drop-shadow filter passes internally — `docs/RENDER_ARCHITECTURE.md`'s GPU Substrate,
`frust-engine::filters` — but there is no `frust_scene::Command` exposing one to a plugin-tier
widget yet). Every one of upstream's `backdrop-filter`/`filter: blur(...)` effects therefore
degrades the same way catalog-wide: a frosted backdrop (drawer, bottom sheet, popover, morphing
modal, center-morph modal, command palette, expandable action bar, project folder, wallet card,
morphing search) becomes a flat wash with no blur behind it; a text/badge/number roll's blur half
(animated badge, number ticker, action-swap's `Blur` variant, checkbox exit, tooltip, select item
entrance, text-reveal/chromatic-reveal, preview rail entrance, adaptive stepper) is dropped and the
opacity/scale/translate half is kept; and the one filter-driven merge effect — adaptive stepper's
upstream `Liquid` gooey metaball surface (an SVG blur + contrast filter fusing three pills into one
blob) — has no primitive to approximate at all, so each pill paints on its own instead.
`morphing_tabs` is not a blur casualty: its own module doc corrects the porting card's premise —
upstream's `morphing-tabs.tsx` has no gooey SVG filter to begin with, so the port's analytic
rounded-rect morph path is not a degradation of anything upstream actually did.

**Applies to**: every `frust-beui` component whose upstream source names a `filter`/
`backdrop-filter` CSS property — dozens across `components`, `agents`, and `blocks`; see each
component's own "Degradations" section for its specific blur.

**Why accepted**: this is the same framework gap `frust-shadcn`'s and `frust-material`'s drawer/
sheet/dialog/popover scrims already accept (no LIMITATIONS entry exists for those catalogs' own
instances yet, since no gate has needed one); a filter layer's scene-level `Command` is a later
render-tier plan (`frust-engine::filters` already builds and schedules the passes internally), not
something a facade-only plugin can add.

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
unverified until a Mac runs it. Acquisition answers `None` on Android and iOS today because no
mobile shell installs a `DeviceHandle` yet (`facade-gpu-context-desktop-only` above), so every
gpu-effects variant renders its 2D path there regardless of the opt-in.

**Evidence**: `plugins/beui/src/components/tilt_card.rs`, `wheel_picker.rs`,
`cylinder_carousel.rs`, `plugins/beui/src/blocks/project_folder.rs`, `wallet_card.rs` module docs'
"Degradations"/"The true-3D ..." sections; `plugins/beui/src/gpu_fx/mod.rs` module docs ("What it
still cannot do"); commit `833e6d64` ("Verified on the T400 desktop"); `examples/beui-demo/README.md`'s
"GPU Effects" section (T400 substrate coverage, Metal owed).

**Trigger for removal**: the widget-subtree boundary is permanent (a new render-tier capability,
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
render-tier additions, none currently planned.

---

### `beui-gpu-fx-depth-and-pooling` — a translucent face writes depth like an opaque one, and the target pool over-allocates small faces

**Observed**: three mechanics bound every `gpu_fx` scene, substrate-wide, not per component.
(a) A `Quad3dScene::depth` attachment writes depth for a translucent face exactly as it would for
an opaque one, so a scene wanting faces to blend over each other — rather than the farther one
being rejected outright — has to submit them in the order that makes that true, and the rule
splits by geometry, not by which component reaches for it: `cylinder::drum_scene` (the wheel
drum, the carousel wall) and `card3d::fan_scene` (the wallet account switcher) both sort their
faces nearest-first — a far-then-near submission would blend a translucent plate twice and darken
every seam two rows or cards overlap, and the sort is stable with a non-finite depth sorted to the
back — while `fan::fan_scene` (the folder preview fan) instead keeps the caller's own
back-to-front order, because a fan's sheets want a nearer one blending *over* a further one and
its caller-supplied stacking order is the only signal that pile has. (b) `FxPass`'s `Binding` —
the bookkeeping deciding whether a frame owes `ExternalFrame::bind_texture`/`unbind_texture` —
lives once per pass inside the same `Mutex<PassState>` every `record` call locks, not once per
surface: with two engine-tier surfaces live at once, each surface's own drain calls `record` on
the identical `FxPass`, and the second call's `Binding::needs_rebind` sees the generation the
first call already recorded and answers `false`, so the second surface's `ExternalFrame` never
receives the bind and composites nothing under that `SceneTextureId`. (c) `pool::TargetPool`
quantises every offscreen target's extent up to a 256px square before keying it — so a small face
still occupies a full 256px-square texture — reaps a key that has gone unasked-for for 120
consecutive frames, and checks a key's quantized extent against
`min(4096, device.limits().max_texture_dimension_2d)` before allocating; `create_target` wraps the
`create_texture` calls in both a `Validation` and an `OutOfMemory` error scope, drains each
(bounded to `DRAIN_POLL_LIMIT` polls, with a drain that never resolves counted as a failure), and
refuses (`None`, nothing inserted) rather than caching a target either scope rejected. That
device-ceiling check runs only inside `TargetPool::acquire`, on the render thread — the component
itself already committed to the 3D path earlier, on the UI thread, because `card3d::target_for`
checks only the raw overscanned side against the pool's own `MAX_TARGET_SIDE` policy cap (4096) —
quantisation to a 256-texel multiple happens later, in `pool::quantize` / `TargetKey::new` — and
never the device's real `max_texture_dimension_2d`. So when the quantised extent exceeds
`min(4096, the device's max_texture_dimension_2d)`, `acquire` answers `None` on the render thread
after the component has already committed to the 3D path in paint — it composites under a
`SceneTextureId` that never gets bound, and the face silently never appears, with no 2D fallback
for that extent. Each component owns its own pool with no aggregate
budget across components, so a colour + `Depth24Plus` target at the 4096px ceiling — on the order
of 100 MB — is a per-component worst case, not a substrate-wide one. Separately,
`Quad3dRenderer` (the compiled pipelines and uniform buffer a pass records through) is built once
per component *instance* on the first frame that instance has content rather than shared per
device, so two live instances of the same component (two `tilt_card`s on screen at once) each pay
their own pipeline-compile cost on their own first frame.

**Applies to**: `cylinder_carousel::gpu_cylinder`/`wheel_picker::gpu_drum` and
`wallet_card::gpu_fan` for the nearest-first sort; `project_folder::gpu_fan` for the
back-to-front order; every `gpu_fx` consumer for (b) whenever a process drives two engine-tier
surfaces at once, and for the pool's quantization/reap/ceiling policy, the two-scope error-scoped
allocation, the render-thread-only device-ceiling refusal, and the per-instance renderer compile
in (c).

**Why accepted**: the sort direction in (a) is mode-specific by design — a drum's or fan-of-cards'
item order carries no meaning of its own (whichever face is nearest wins by distance), while the
folder fan's caller-supplied stacking order is the only signal that pile has — so one rule cannot
serve both, and each geometry module owns its own resolution rather than pushing the decision onto
a component. (b) is accepted because every shipped shell drives one engine-tier surface per
process — see `external-pass-registry-process-wide` above, which the same one-process-wide-state
shape traces back to; a per-surface `Binding` is real design work with no shipping consumer to
prove it against yet. In (c), the pool's 256px quantization and 120-frame reap mirror
`frust_gpu::effects`' own texture-pool policy value for value, so a small face's over-allocation
is the same trade the engine already makes; the device-ceiling check and the two-scope
error-scoped allocation turn an out-of-range or refused request into "draw nothing this frame"
instead of a validation panic. The render-thread-only device-ceiling refusal itself is accepted
because every desktop adapter this port currently targets reports a `max_texture_dimension_2d` of
at least 4096, and a paint-side check that could catch it before the component commits to the 3D
path would need the shell's live device limits published through the facade, which nothing does
yet. The missing aggregate budget is accepted because no shipped component drives more than
a handful of resident targets at once. `Quad3dRenderer`'s per-instance compile is accepted in its
own module docs as a documented cost: sharing one renderer per device would serialize the
concurrent `record` calls two live engine-tier surfaces are allowed to make, and would collapse
every pass's diagnostic label into one.

**Evidence**: `plugins/beui/src/gpu_fx/cylinder.rs` module docs ("Depth is the whole point, and it
needs the nearest face first") and `drum_scene`; `plugins/beui/src/gpu_fx/card3d.rs`'s `fan_scene`
doc; `plugins/beui/src/gpu_fx/fan.rs` module docs ("Depth, and why the sheets are still submitted
back-to-front"); `plugins/beui/src/gpu_fx/quad3d.rs` module docs ("Depth") and its
`Quad3dRenderer` doc ("Accepted as a documented per-instance cost for now rather than solved");
`plugins/beui/src/gpu_fx/schedule.rs` (`PassState`'s single `binding: Binding` field, `FxPass`'s
`Mutex<PassState>`, `ExternalPass::record`'s `needs_rebind`/`record_bind` call); `docs/LIMITATIONS.md`'s
own `external-pass-registry-process-wide` entry; `plugins/beui/src/gpu_fx/pool.rs`
(`TARGET_QUANTUM = 256`, `MAX_UNSEEN_FRAMES = 120`, `MAX_TARGET_SIDE = 4096`, `TargetPool::acquire`'s
device-ceiling check, `create_target`'s `push_error_scope`/`drain_error_scope` over both
`wgpu::ErrorFilter::Validation` and `wgpu::ErrorFilter::OutOfMemory`, module docs "Why a pool at
all"); `plugins/beui/src/gpu_fx/mod.rs`'s `DRAIN_POLL_LIMIT` and `drain_error_scope` (the bounded
poll, exhaustion-as-failure behaviour `create_target` relies on); `plugins/beui/src/gpu_fx/card3d.rs`'s
`target_for` (its `MAX_TARGET_SIDE`-only check, not the device's real `max_texture_dimension_2d`).

**Trigger for removal**: none planned for (a) or the pool's quantization/reap/per-instance-compile
shape in (c) — both are structural consequences of a shared depth-tested-translucency primitive
and a bounded per-component texture pool, not a defect either geometry module could fix
independently. The device-ceiling refusal within (c) has a real trigger: the facade exposing the
live device's limits to widgets, so a paint-side check can refuse before the component ever
commits to the 3D path. (b) shares its trigger with
`external-pass-registry-process-wide`: a per-surface `Binding` keyed by surface identity, or the
first shell that keeps two engine-tier surfaces live at once, whichever lands first.

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
what the 3D path fans is that panel's rows, not the card deck the porting card's premise described
(see `wallet_card`'s own "A premise correction").

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

### `beui-substituted-springs` — an upstream ad hoc spring resolves to the nearest catalog spring

**Observed**: several upstream components author a one-off, per-component spring
(`{ stiffness, damping, mass }`) that is not one of `tokens::motion`'s six named springs
(`SPRING_PRESS`, `SPRING_LAYOUT`, ...). The port substitutes the nearest catalog spring rather than
adding a seventh token per caller — e.g. `message_bubble`'s `BUBBLE_POP` (upstream
`520/27/0.52`) resolves to `SPRING_PRESS` (`500/30/0.6`); `text_animation`'s `Reveal` spring
(upstream `140/26/1.2`) resolves to `SPRING_LAYOUT`.

**Applies to**: any component whose module docs name a substituted spring — `message_bubble`,
`text_animation`, and others sharing the same token-budget rationale.

**Why accepted**: keeping the token set closed to six named springs is a deliberate charter choice
(`tokens::motion`, cited from `lib.rs`'s Charter) — motion tokens are catalog-level, not
per-component numbers — and the nearest catalog spring reads indistinguishably close at these
timings.

**Evidence**: `plugins/beui/src/agents/message_bubble.rs` module docs ("`BUBBLE_POP` is
substituted"); `plugins/beui/src/components/text_animation.rs` module docs ("Reveal's spring is
substituted").

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

### `beui-focus-and-keys` — four named focus/keyboard gaps across the catalog

**Observed**: four related gaps, each named in its component's own source:

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
4. **No widget-side auto-dismiss timer.** `animated_toast_stack` ports no timer at all — upstream's
   lives in a React hook, and a widget-side timer could only fire on the user's next input event,
   which is not what auto-dismiss means; the app owns the dismiss schedule instead.

**Applies to**: `frust_beui::components::select`/`combobox`/`multi_select`,
`frust_beui::blocks::feedback_widget`, `frust_beui::components::expanding_arrow_button` (both
`hold_action_button` and `slide_action_button`), `frust_beui::components::animated_toast_stack`.

**Why accepted**: (1) mirrors an already-accepted shadcn boundary; (2) and (3) are framework-level
gaps with no plugin-tier fix available, already accepted for their originating catalogs/entries;
(4) is a correct reading of what a widget can own versus what only the app's own clock can drive.

**Evidence**: `plugins/beui/src/components/select.rs`, `combobox.rs`, `multi_select.rs` module docs
("No type-ahead"/"No type-ahead, and no keyboard list navigation");
`plugins/beui/src/blocks/feedback_widget.rs` module docs ("Closing does not blur the message
field", "It reserves its open box and paints inside it" — the morph is pure paint, no rebuild);
`plugins/beui/src/components/expanding_arrow_button.rs` module docs ("fires on the next
event pass", "Keyboard activation is not ported"); `plugins/beui/src/components/animated_toast_stack.rs`
module docs ("No timer").

---

### `beui-no-file-drop` — `file_upload` has no file-drop/drag-session input, only programmatic add

**Observed**: no shell in this repository publishes a file-drop signal for a widget to read, and
frust delivers no drag-session or path type at all, so `blocks::file_upload`'s dropzone is a prop
(`on_browse`/`add_files`), never an observed drag-and-drop state.

**Applies to**: `frust_beui::blocks::file_upload`.

**Why accepted**: no shell-level file-drop input exists anywhere in the framework yet; this is a
platform-input gap, not something a facade-only plugin can add.

**Evidence**: `plugins/beui/src/blocks/file_upload.rs` module docs and
`FileUploadView::add_files`/`on_browse` (no drag-session or path type reaches a widget).

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
- **`ai_sidebar` has no drag-and-drop and no inline rename** — the same missing drag protocol
  `beui-no-file-drop` names, and no reachable seam to swap a row's label for a live text field.
- **Agent lists auto-collapse silently** — `todo_list`'s and `agent_activity`'s automatic
  `collapseOnComplete` flips an internal flag rather than reporting through a callback, the
  rebuild-driven variant `no-plugin-reachable-deferred-state-callback` now also documents.
- **`chat_app`'s user bubble is `Soft`, not upstream's default variant** — a deliberate visual
  choice for the assembled example, named in its own module doc.

**Applies to**: `frust_beui::agents::code_block`/`tool_approval`/`file_diff`/`tool_result`/
`agent_activity`/`citations`/`streaming_response`/`message`/`image_generation`/`ai_sidebar`/
`todo_list`/`chat_app`.

**Why accepted**: each is either an inherited platform gap already accepted elsewhere (clipboard,
drag-and-drop, deferred-callback), a named scope line (shiki, pre-parsed diff model, favicons), or
a deliberate design choice recorded in its own module doc (the reveal ramp, the bubble variant).

**Evidence**: `plugins/beui/src/agents/code_block.rs`, `tool_approval.rs`, `file_diff.rs`,
`tool_result.rs`, `agent_activity.rs`, `citations.rs`, `streaming_response.rs`, `message.rs`,
`image_generation.rs`, `ai_sidebar.rs`, `todo_list.rs`, `chat_app.rs` — each module's own
"Degradations against upstream" section.

---

### `beui-blocks-degradations` — named gaps across the `blocks` catalog

**Observed**: recurring, named gaps across `frust_beui::blocks`, each a platform or scope boundary:

- **`notification_stack` animates its own height** (no out-of-flow layer to pop a card out of, so
  the box resizes with the stack instead), and ships no upward overflow handling, no per-card
  dismiss, and no grouping — none of which upstream's own source has either, per the porting card's
  premise correction.
- **`swipeable_list`'s thresholds are distance-only.** Upstream's release rules pair a distance arm
  with a velocity arm; `frust::input` publishes no `PointerEvent` velocity a swipe gesture can read,
  so only the distance arm is ported (the velocity arm's constants are recorded, unused, so it can
  be restored later) and there is no full-swipe dismiss.
- **No outside-press dismissal for `expandable_tabs`/`notification_stack`.**
- **No pointer-type distinction** anywhere in the catalog — a mouse and a touch press are handled
  identically, since frust's input events do not carry pointer type.
- **`morphing_tabs`' active-tab order commits on release, not after the neighbours settle** — a
  named simplification, not a bug.
- **Text narrowing (`ReactNode` → `String`) in stack/list rows** — `notification_stack` and similar
  row-based blocks take plain text where upstream takes arbitrary content, the same narrowing
  `shadcn-otp-table-button-api-gaps` names for `table`'s header/footer.
- **`availability_scheduler` is per-day time ranges only** — upstream has no drag-to-select grid
  either, so this is parity, not a gap the port introduced.
- **`dynamic_island`'s shell radius is a constant** (`RADIUS = 32`, browser-clamped upstream to half
  the height); the port passes the same constant through rather than deriving it from measured
  height.
- **`marquee` is not interactive** — a press does not pause it; only hover does, since the
  framework's hover link ends when a scene it's watching is torn down, which a press does not do.
- **`theme_toggle`'s reveal is scoped to the toggle itself**, not the full viewport upstream's View
  Transition API sweeps.
- **`loader` keeps its reduced-motion pulse deliberately** — a loader frozen mid-spin reads as hung,
  so reduced motion here paints the rest pose modulated by upstream's own `[1, 0.4, 1]` opacity
  pulse rather than dropping all motion.

**Applies to**: `frust_beui::blocks::notification_stack`/`swipeable_list`/`expandable_tabs`/
`morphing_tabs`/`availability_scheduler`/`dynamic_island`; `frust_beui::components::marquee`/
`theme_toggle`/`loader`; no-pointer-type-distinction applies catalog-wide.

**Why accepted**: each is a named platform gap (no velocity, no pointer type, no out-of-flow
layout) or a documented parity/premise correction against upstream's own source, not a regression
found later.

**Evidence**: `plugins/beui/src/blocks/notification_stack.rs`, `swipeable_list.rs`,
`expandable_tabs.rs`, `morphing_tabs.rs`, `availability_scheduler.rs`, `dynamic_island.rs`,
`plugins/beui/src/components/marquee.rs`, `theme_toggle.rs`, `loader.rs` — each module's own
"Premise correction"/"Degradations" sections.

---

### `beui-shader-background-verification` — the five ported shader variants are string-checked, not GPU-verified, in-repo

**Observed**: `frust-beui` carries no GPU dev-dependency (`plugins/beui/Cargo.toml` — `frust-widgets`
`test-support` and `frust-core` `test-support` only, both host-side fixtures), so
`shader_background`'s WGSL sources are checked as strings by the host test suite, never compiled or
rendered by an adapter/device in this crate's own tests.

**Applies to**: `frust_beui::components::shader_background`'s five ported WGSL programs.

**Why accepted**: verified instead by an out-of-tree device render run (T400/Vulkan) rather than an
in-repo GPU test, the same verification split the engine's own downlevel/WebGL2 lints accept for
shader correctness. This crate does carry an in-repo GPU-adapter harness now — the non-default
`gpu-effects` feature's `#[ignore]`d `gpu_fx` tests (see `beui-3d-degradations` above and
`examples/beui-demo`'s GPU Effects page) — but `shader_background` does not build on that feature
or that harness, so its own WGSL sources stay string-checked here regardless.

**Evidence**: on 2026-09-03 the conductor re-ran an out-of-tree wgpu harness that compiled all five
WGSL programs and rendered each to an offscreen target on an NVIDIA T400 over Vulkan (headless
Linux rig, the same adapter pins the engine's ignored tests use), outcome PASS; the worker's
earlier run on the same rig reported the same. `plugins/beui/Cargo.toml` (no GPU dev-dependency);
`plugins/beui/src/components/shader_background.rs` (WGSL sources as `&str` constants, compiled only
by the engine at runtime).

---

### `beui-modal-untested-on-device` — `show_modal`/navigator-hosted modal exercised only in-process so far

**Observed**: `overlay::modal`'s navigator-hosted path (`show_modal`,
`NavigatorController::push_with_options`) is exercised by this crate's in-process host tests only;
the `beui-demo` desktop gallery pass covered the `Stack`-mounted overlay pages (the hover/anchored
family) but not a device or desktop-window run of the navigator-pushed modal path specifically.

**Applies to**: `frust_beui::overlay::modal`'s `show_modal` route and every modal-hosted component
(morphing_modal, center_morph_modal, command_palette, drawer, animated_sidebar, bottom_sheet).

**Why accepted**: the entrance/exit-staging half of the mechanism is shared with `frust_material`'s
already device-verified `show_overlay_modal` route — same `push_with_options` +
`BackPolicy::DismissAnimated` seam, same `StagedPop`/`StagedExit` guard — so the residual risk on
that leg is component-specific paint, not the navigator mechanism. That argument does not cover
the back-press leg: a platform back press arriving mid-exit-ramp has not itself been exercised on
`frust_beui`'s route on a device. (`frust_shadcn`'s modal is a different seam,
`push_transparent_for_result` → `push_impl(BackPolicy::Pop)`, and is not part of this
shared-mechanism argument at all.)

**Evidence**: `plugins/beui/src/overlay/modal.rs:1281` (`show_modal`, `push_with_options` at
`:1308`) and its test module (in-process only); `plugins/material/src/overlay/modal.rs:1146`
(`show_overlay_modal`);
`plugins/shadcn/src/overlay/modal.rs:655` (`push_transparent_for_result`);
`crates/frust-widgets/src/nav/controller.rs:388-395` (`push_transparent_for_result`) and `:438`
(`push_impl` setting `BackPolicy::Pop`);
`examples/beui-demo/README.md`'s "Verifying it" section (headless dev rig, no device run recorded
for the modal-hosted pages specifically).

**Trigger for removal**: a device or desktop-window pass of `show_modal`, including a platform
back press delivered mid-exit-ramp.

---

### `scroll-view-no-external-offset-seam` — `message_scroller` (both ports) and beUI's `scroll_to` still re-implement scroll physics instead of composing over `ScrollController`

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
---

### `render-blurred-shadow-corner-collapse` — a per-corner blurred shadow rounds to its largest corner

**Observed**: `Command::BlurredRoundedRect` carries a per-corner `CornerRadii`, but both render
backends' blurred-rect primitive (the engine's `vello_common` 0.2.0 `BlurredRoundedRectangle`,
`vello_cpu`'s `fill_blurred_rounded_rect`) accepts only one radius. Both arms collapse the four
corners via `CornerRadii::largest()` before encoding, so a shadow behind geometry
with mixed corner radii (e.g. two sharp corners, two rounded) renders with all four shadow corners at
the largest configured radius instead of each corner independently.

**Applies to**: any glow/shadow paint (`ContainerView`'s `glow`, or a direct `PaintScene` caller)
whose `CornerRadii` are non-uniform. The fill, border, and clip paths stay exact per-corner —
`Command::RoundedRect`/`PushClipRounded` both lower every corner individually; only the blur is
affected.

**Why accepted**: neither backend exposes a per-corner blurred-rect primitive; matching one would
mean hand-rolling the blur pass rather than composing the existing one. The visual delta is confined
to the soft, low-opacity shadow rather than the crisp geometry, so it shipped as a v1 approximation
alongside the `CornerRadii`/`DashPattern` primitives rather than blocking on it.

**Evidence**: `crates/frust-engine/src/compile/blur_rrect.rs`'s decode of
`Command::BlurredRoundedRect` through `CornerRadii::largest` and its regression test.

---

### `render-hybrid-spike-outcome` — Phase 0 `vello_hybrid` spike: GO on the numbers, snapshot cache not provably deletable yet

**Observed**: the retired "vello_hybrid spike — Pixel 5, iOS Simulator, macOS Metal
(engine plan Phase 0)" section of `benchmarks/RESULTS.md` (now in git history (`git show f64be636:benchmarks/RESULTS.md`) — RESULTS.md carries only
the Frust-vs-Flutter matrix since 2026-09-05) measured the experimental `vello_hybrid` render tier
(`RenderTier::Hybrid`, override-only, `hybrid-tier` feature) against that file's "Pixel 5 — classic
baseline for the engine plan" section, same Adreno 620 hardware:

1. **Frame scenarios, p50/p95 ms, hybrid vs classic**: S1 12.20/13.90 vs 48.23/49.13, S2
   13.10/14.06 vs 42.23/43.98, S4 9.36/10.27 vs 16.71/17.45, S5 11.47/12.30 vs 94.71/98.52, S6
   11.59/12.38 vs 22.44/23.36 — 1.8-8.3x faster; S2/S4/S5/S6 are partly paced by the device's 90Hz
   mode, so those four ratios are lower bounds. The classic S1 model's ~9.6ms fixed
   full-resolution-pass term (`t ≈ 9.6 + 38.7·s²` ms) is gone outright: hybrid's entire S1 frame
   (12.20ms) sits below that fixed term alone.
2. **CPU/GPU split** (`frust-perf hybrid strip_us=<n> record_us=<n>`): S1 CPU strip 5.42ms, GPU
   command-record 0.62ms, GPU-side remainder (total − strip) 6.78ms.
3. **Atlas-cache arm** (`FRUST_HYBRID_ATLAS_CACHE=1`, glifo's experimental glyph-atlas cache): S6
   9.98/10.50 (strip 5.10→1.08ms, −79%), S2 11.31/12.39.
4. **material3-demo nav push/pop**: hybrid inline `total_p50` 15.22ms / `submit_p95` 17.66ms vs
   classic cache-ON `total_p50` 7.39ms — hybrid bypasses the snapshot cache/compositor entirely
   (stated outright in `renderer.rs`), so this is not a like-for-like number; no classic
   scale-1.0, cache-off inline pass was measured to compare against.
5. **iOS Simulator** (iPhone 16, iOS 18.6): renders through `tier=hybrid` (full home page, text,
   icons) where vello classic is black by construction (missing `INDIRECT_EXECUTION`); frame
   timings were not measured (no `frust-perf` lines on that console).
6. **macOS Metal** (desktop preview): hybrid runs without error via the `hybrid-direct` path; both
   arms were display-paced at the shell's hard-coded 1600×1200 physical window, non-discriminating
   between tiers; the 5120×2880 target and the root `PushLayer(alpha<1)` >4096-texture question
   were not measured on this pass — the desktop shell now has a window-size knob
   (`FRUST_WINDOW_SIZE`/`FRUST_WINDOW_MAXIMIZED`, `app_handler.rs`) that makes a 5120×2880 arm
   reachable, but that arm has not been re-run with it; still **not measured**.
7. **Browser WebGL2**: written NO-GO (the retired RESULTS.md Arm 7 — git history — and `benchmarks/harness/webgl2_arm.md`) — no
   wasm shell or target exists in this workspace; OPEN #1 decided **(b)**: the target-gated
   `wasm32` `gles`/`webgpu` section belongs to the Web Shell plan, not this one.

**Decision**: Phase 0 is **GO on the numbers** — the fixed per-pass cost and the iOS-Simulator
render blocker are both removed by a strips-on-wgpu core. Measured against the plan's ratified
thresholds: nav `total_p50` ≤ 16.7ms is met inline (15.22ms) but is a ~2x regression against the
cached classic path (7.39ms); full-screen-quad ≤ 3ms GPU is **not** met by S5 (11.47ms whole
frame, ~9.7ms GPU-side remainder, partly display-paced); cold-page ≤ 8ms and Graphics ≤
classic+10% were not measured on this pass.

**Not provably deletable on this evidence** (historical, at the time of the Phase 0 spike): the
snapshot-layer cache and compositor could not be retired on the hybrid nav number alone — an
inline (uncached) figure with no full-resolution classic inline number beside it. Resolved since:
p8-03 deleted the whole mechanism (cache, compositor, `FramePlan`/segment plumbing) once the engine
became the sole renderer; the nav budget question this paragraph left open no longer applies.

**Two hybrid-arm scope limits**: `vello_hybrid` itself outputs premultiplied alpha, so it is
correct — and unwarned — on a premultiplied-expecting translucent surface (Android's
`Inherit`/`PreMultiplied`); only a straight-alpha translucent surface (iOS's `PostMultiplied`) is
refused (`hybrid_translucency_refused`), degrading to Mode A with one warning. Every number above
is still an opaque-surface number regardless, independent of that refusal — the Pixel 5 surface
itself resolves `alpha_modes=[Inherit] chosen=Auto`, opaque; and the desktop shell's default
800×600-logical window (`crates/frust-shell-desktop/src/app_handler.rs`'s `INITIAL_SIZE`) is why
the macOS large-texture question above has no answer from either tier as measured — that default is
now overridable via the `FRUST_WINDOW_SIZE`/`FRUST_WINDOW_MAXIMIZED` knobs in the same file
([DEVELOPMENT.md](DEVELOPMENT.md) § Instrumentation), so the arm is measurable once re-run at
5120×2880.

**Image-atlas capacity guard**: `ImageResidency` (`hybrid_tier.rs`) tracks `live_pixels` against a
`pixel_budget` — half (`ATLAS_BUDGET_DIVISOR`) the device-clamped 8×4096² atlas capacity — and
skips a draw (`IMAGE_ATLAS_FULL`, warn-once) rather than panicking when an upload would cross it;
an evicted image's pixels reopen the budget for a later upload. The budget is conservative, not a
proof: the allocator's one guarantee is that a *fresh* atlas always fits a within-clamp image,
atlas count only grows and is unobservable past the first few uploads, and the backing
`guillotiere` allocator refuses on shape rather than area (module doc, `hybrid_tier.rs`). The glyph
atlas carries no equivalent guard — glifo itself degrades on a full atlas, and `vello_hybrid`'s one
glyph-atlas `expect` is reachable only with `FRUST_HYBRID_ATLAS_CACHE=1` — the accepted residual
(see [RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md)).

**Applies to**: the frust-engine + frust-gpu plan's Phase 0 GO/NO-GO gate only.
Historical: the spike tier this entry measured (`RenderTier::Hybrid`, `hybrid_tier.rs`,
the `vello_hybrid` pin) has since been deleted — the engine tier is its successor; the
entry stays as the record of the GO decision and its measured numbers.

**Evidence**: the retired "Pixel 5 — classic baseline for the engine plan" and
"vello_hybrid spike — Pixel 5, iOS Simulator, macOS Metal (engine plan Phase 0)" sections of
`benchmarks/RESULTS.md` (Arms 1-7, "Fit comparison against the classic S1 model", "What could not
be measured") — in git history (`git show f64be636:benchmarks/RESULTS.md`);
`benchmarks/harness/webgl2_arm.md`; `crates/frust-render/src/hybrid_tier.rs` and `tier.rs`;
`crates/frust-render/src/context.rs`'s `HybridDirect` arm and its `hybrid_translucency_refused`
straight-alpha refusal.

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

### `frust-text-no-variable-font-axes` — no seam for variable-font axes beyond weight

**Observed**: `frust-text::TextStyle` carries no variations/width field
(`crates/frust-text/src/style.rs:246-264`), and the parley translation pushes
only `FontWeight`/`FontStyle`/`FontSize`/`LetterSpacing`/`LineHeight` as
default run properties — no `FontVariations` or `FontWidth`
(`crates/frust-text/src/context.rs:129-137`) — even though the pinned parley
0.11.0 supports both (`style/mod.rs:77,85`). This is a seam gap in
frust-text, not a limitation of the underlying shaping engine.

**Applies to**: any catalog or app wanting to drive a variable font's `wdth`
(width) or custom axes (e.g. Roboto Flex's `GRAD`/`ROND`) from `TextStyle`.
Concretely, `frust_material`'s M3E emphasized type scale approximates the
reference's uniform `wght600+GRAD50` axis preset with a per-role weight
step-up instead, and wdth/ROND-based type styles (Condensed/Wide/Round) are
unexpressible — a future material3-demo theme page degrades to a plain
Regular/Emphasized toggle.

**Why accepted**: out of scope for the Material 3 Expressive Phase 1
foundations work, which spiked the gap rather than closing it (adding the
seam is a `frust-text` API change, not a `frust_material` one). Deferred to
whichever future work needs the axis, tracked here so the degrade is
discoverable rather than silently baked into the type scale.

**Evidence**: `crates/frust-text/src/style.rs:246-264` (`TextStyle`, no
variations/width field); `crates/frust-text/src/context.rs:129-137`
(`push_style_defaults`, no `FontVariations`/`FontWidth` push); pinned
parley 0.11.0 `style/mod.rs:77,85`.

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
cross-catalog type change, not something Phase 1's foundations work took on
speculatively. The dp table itself is exact; only the shadow decomposition is
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
the Phase 2 core-controls port took on. The same class of gap already exists
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
"the intended content") rather than attempted inside Phase 2's core-controls
port.

**Evidence**: `plugins/material/src/text_field.rs` module doc's "Known
limitations" section (the "A `Down` on an interactive slot blurs the field"
bullet); confirmed in review as a major finding.

---

### `material-picker-strings-no-i18n-seam` — the date/time pickers ship English-default strings with no path to a real locale

**Observed**: `frust_material::date_picker` and `time_picker` take every label, weekday name, and
the first-day-of-week index from a caller-fillable `DatePickerStrings`/`TimePickerStrings` struct,
defaulted to `DatePickerStrings::ENGLISH`/`TimePickerStrings::default()` (Flutter
`MaterialLocalizations`' own `en_US` values). This is an *adaptation* of the reference's
localization, not a port of it: a design-system plugin may not depend on a sibling plugin
(`docs/PLUGINS_ARCHITECTURE.md`'s Layer Dependencies), so `frust-i18n` is out of reach from
`plugins/material`, and there is no seam connecting one to the other even at the app layer. The date
picker's input-mode parse/format (`MaterialDate::format_compact`, `mm/dd/yyyy`) is narrower still:
it is the fixed `en_US` compact form, and **not overridable in v1** even via the strings struct —
unlike every other label, a non-`mm/dd/yyyy` input format cannot be substituted at all.

**Applies to**: any app shipping `frust_material::date_picker`/`time_picker` in a non-English
locale. An app translates the label surface itself (`..DatePickerStrings::ENGLISH`/
`TimePickerStrings::default()` for the fields it doesn't override) and reformats displayed dates on
its own, but the date picker's typed-entry mode stays `mm/dd/yyyy` regardless.

**Why accepted**: closing this needs either an i18n-aware seam threaded from an app's own
`frust-i18n` locale into the picker (a cross-plugin bridge the charter above doesn't provide today)
or a second, locale-parametrized compact-date format — both out of scope for the Phase 3 port,
which prioritized reference-accurate calendar/dial math over a localization bridge no other
design-system plugin has either.

**Evidence**: `plugins/material/src/date_picker/mod.rs` module doc's "Localization: English
defaults plus an override struct" section; `plugins/material/src/time_picker/mod.rs` module doc's
"Strings: English defaults, overridable as a bundle" section; `docs/PLUGINS_ARCHITECTURE.md`'s
Layer Dependencies (no design-system plugin may depend on a sibling plugin).

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

**Why accepted**: the general seam and its five adopters landed in one round (gate-r5); extending
the same forwarding builder method to `search`/`navigation_drawer` is a small, mechanical follow-up
that round didn't reach, not a new design problem.

**Evidence**: `plugins/material/src/overlay/modal.rs`'s `ModalDismiss` type and its "App-initiated
dismiss" module-doc section; `dismiss_handle` forwarding methods in `side_sheet.rs`, `dialog.rs`,
`date_picker/dialog.rs`, `time_picker/mod.rs`, `sheet.rs`; `search/view.rs`'s header doc (back
affordance still pops unstaged) and `navigation_drawer.rs`'s "Selecting does not dismiss" section
(both cite this id); gate-r5-04 `f3093a13` (staged-dismiss-public-seam) and gate-r5-02 `73cbcde6`
(bottom-sheet adoption).

---

### `material-staged-pop-one-frame-race` — a staged modal/sheet pop's identity guard still has a one-frame race window

**Observed**: `StagedPop` (`plugins/material/src/overlay/modal.rs`, shared by `sheet.rs`'s bottom
sheet) arms itself against `NavigatorController::depth()` when an exit stages and refuses the
deferred pop if depth moved by the time the ramp settles — the p3r2 closure fix that narrowed a
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

### `material-sheet-dismissal-pop-is-staged` — a modal/sheet dismissal delivers the navigator pop ~250-300ms after the gesture, not immediately

**Observed**: every dismiss trigger on `frust_material::overlay::modal`/`sheet` (scrim tap, close
button, handle tap, drag commit, Escape, Android back) stages an exit ramp and fires the navigator
pop only once that ramp settles, at the M3 exit-motion durations (`MaterialMotion::SHORT_4`/
`LONG_2`, 200-500ms scaled by remaining travel). An app timing work off the dismiss gesture — e.g.
firing a side effect from `on_result` — sees that delay rather than an immediate pop.

**Applies to**: any `frust_material` app wiring `on_result`/pop-result handling on a modal or sheet
push.

**Why accepted**: deliberate design, not a bug — the exit animation owns the pop timing so the
navigator page (and its input barrier) stays mounted for the whole reverse ramp; popping on the
gesture instead would tear the page down mid-animation.

**Evidence**: `plugins/material/src/overlay/modal.rs` module doc's "Exit motion, and why the close
hook is state-free" section; `plugins/material/src/sheet.rs`'s equivalent staged-exit machine.

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
settle-delivery fix — filed as a FINDING candidate rather than attempted in-crate.

**Evidence**: `plugins/material/src/refresh_indicator.rs` module doc's "Documented seam gap: the
scroll offset is not held during a refresh" section (incl. the `on_scroll` one-event-late note).

---

### `material-fab-fixed-tier-icon-centering` — a tight-constrained FAB paints its icon off-center

**Observed**: `FabWidget` lays its icon out against its `FabSize` tier's own container box
(`self.size.container()`), not the box the widget was actually laid out at — `Widget for
FabWidget::layout` never re-derives the icon-centering math from the incoming `bc`. A FAB rendered
at a tight-constrained size that doesn't match its nominal tier paints its icon off-center inside
the visually smaller (or larger) box. Measured on `plugins/material/src/toolbar.rs`'s scroll-hide
FAB morph (80dp→56dp, tight-constrained rather than paint-rescaled so layout/visual/hit rects stay
one): ~12dp of icon offset at the 80dp end — independent of the separate baseline icon-centering fix
(`crates/frust-widgets/src/icon.rs`, gate round 4, merge `568ff5ab`): that fix makes `IconWidget`
scale/center its glyph inside its own laid-out box. At *this* call site's `Medium` tier
(`self.size.icon_size()` = 24dp) the fix is genuinely a no-op, since the icon's own default
configured size is also 24dp — but that is a fact about the `Medium` tier only, not "every current
caller." The fix **does** change rendering at every non-natural tier (box ≠ the caller's configured
24dp default) in the sized-component families verified below — the product of a full sweep of every
production `BoxConstraints::tight` icon site in `crates/frust-widgets` and the four design-system
plugins (conductor + phase-6 review, 2026-08-21), recorded here as the verified set rather than a
claim of universality: `fab` (`FabSize`, tight-constrained at `fab.rs:860`) — Small/Medium 24dp
(natural), Large 36dp (`LARGE_ICON`, `fab.rs:182`) non-natural; `button/core.rs`
(`ButtonSize::metrics`, `:101-129`, tight-constrained at `:838`) — Xs/Sm 20dp, Md 24dp (natural),
Lg 32dp, Xl 40dp, all four non-`Md` tiers non-natural; `toggle_button.rs`
(`ToggleButtonSize::metrics`, `:259-283`, tight-constrained at `:1466`) — Sm 20dp, Md 24dp
(natural), Lg 32dp non-natural; `icon_button.rs` (`icon_glyph_size`, `:226-233`, tight-constrained
at `:1060`) — Xs 20dp, Sm/Md 24dp (natural), Lg 32dp, Xl 40dp non-natural; `split_button.rs`
(`SplitButtonSize::icon_size`, `:355-362`, tight-constrained at `:1438`, around the caller-supplied
`leading_icon`) — Xs 20dp, Sm/Md 24dp (natural), Lg 32dp, Xl 40dp non-natural. Sites the same sweep
verified NATURAL and therefore unaffected, recorded so the sweep is auditable: `navigation_rail.rs`
(`ICON_SIZE` 24, tight at `:1299`/`:1335`), `fab.rs`'s extended path (`EXTENDED_ICON` 24, `:214`),
`fab_menu.rs`'s item icons (`ITEM_ICON` 24, `:261`), and baseline
`crates/frust-widgets/src/icon_button.rs` (its `mark_view` sizes its own icon to the constraint at
`:111`, so box and configured size match by construction). At every non-natural tier the fix now
paints a glyph scaled and centered to that tier's box instead of the pre-fix 24dp corner-anchored
placement — the demo's fabs/buttons/button_group/icon_buttons/split_button playground pages each
expose the relevant Size knob live (e.g.
`examples/material3-demo/src/pages/playground/do_/fabs.rs:84`'s `any(icon(icons::ADD))` under a
selectable `FabSize::Large`; `do_/split_button.rs:303`'s `.leading_icon(|| any(icon(icons::SAVE)))`
with all five tiers offered). This is believed net-positive — the same glyph-vs-box contract
violation the fix closes, corrected everywhere it recurs, not only at the FAB — but **unverified on
device**: the widened re-verify scope is recorded against the FAB device-gate row. None of this touches the
`FabWidget` mechanism above: that mismatch is entirely `FabWidget`'s own `container` variable, which
stays the nominal tier size regardless of `bc`, independent of whatever the icon inside it is doing.
The Phase-2 `corner_radius` shape-morph seam on `fab.rs` — built so a FAB-family morph could ride
it — was deliberately bypassed for the same reason: `fab_menu`'s Square↔Circle trigger morph hand-rolls
its own morph via `shapes::Morph` instead of consuming that seam, specifically to avoid reproducing
this degrade on a second surface. That leaves `corner_radius` with zero consumers in this crate
today — resolved as **keep** (see *Why accepted*), not a signal to remove it.

**Applies to**: any FAB rendered at a size other than its nominal `FabSize` tier — today only
`toolbar.rs`'s 80dp→56dp scroll morph. `fab_menu`'s trigger avoids the class entirely by not using the
seam.

**Why accepted / fix direction**: closing the centering bug needs a `FabView` seam that exposes the
actual container size continuously (a `container_size` prop) so icon layout can track the
constrained box instead of the nominal tier — deferred as a FINDING candidate rather than attempted
inside the toolbars rework. `corner_radius`'s zero-consumer status is **resolved, not pending**:
the seam stays. [`FabView::corner_radius`](`fab.rs:556`) is the public mirror of the reference's
`M3EFab.cornerRadius` prop, and this arc's own success criterion is parity with the reference's
public prop surface, not in-crate usage — "no consumer" only ever meant `fab_menu` doesn't call
into it, not that the prop lacks a reason to exist. Recorded here so the two facts — the centering
bug and the seam's resolved fate — travel together instead of being lost between phases.

**Evidence**: `plugins/material/src/toolbar.rs` module doc (FAB morph section);
`plugins/material/src/fab.rs` (`Widget for FabWidget::layout`'s `self.size.container()`;
`LARGE_ICON` at `fab.rs:182`; the icon tight-constrain at `fab.rs:860`; `FabView::corner_radius` at
`fab.rs:556` and its module doc's "Corner-radius override seam" section);
`plugins/material/src/fab_menu.rs` module doc (trigger-morph rationale);
`crates/frust-widgets/src/icon.rs` (gate round 4 fix, merge `568ff5ab` — `Widget for
IconWidget::layout`/`paint`, the natural-vs-tight-box divergence); `plugins/material/src/button/
core.rs:101-129` (`ButtonSize::metrics`, tight-constrain at `:838`);
`plugins/material/src/toggle_button.rs:259-283` (`ToggleButtonSize::metrics`, tight-constrain at
`:1466`); `plugins/material/src/icon_button.rs:226-233` (`icon_glyph_size`, tight-constrain at
`:1060`); `examples/material3-demo/src/pages/playground/do_/fabs.rs:84` (default-24 icon under a
selectable `FabSize::Large`); the FAB device-gate re-verify scope.

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
The API floated at plan time (`MaterialTheme::with_scheme_override(ColorScheme)`) did not ship —
there is no `MaterialTheme` type anywhere in this crate. The only injection path that exists is
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
seed — out of scope for this arc.

**Evidence**: `plugins/material/src/tokens/hct.rs:966` (`theme_from_seed`; no
`with_scheme_override`/`MaterialTheme` anywhere in the crate); `examples/material3-demo/src/pages/
theme_config_page.rs`'s `toggles` (the disabled "Dynamic color" row and its supporting text);
`examples/material3-demo/src/theme/settings.rs` module doc ("Dynamic (device-sourced) coloring is
not modelled at all"); the Material 3 Expressive scope decision to leave dynamic color out.

---

### `material-no-focus-traversal` — no keyboard tab-order/traversal between controls; overlays claim focus only on first interaction

**Observed**: the catalog ports per-widget focus **visuals** throughout (state-layer focus rings
and outlines, gated on `PaintCtx::has_focus`), but the framework underneath has no keyboard
focus-**traversal** system — no Tab ring, no `autofocus`, no `FocusNode` equivalent — because the
baseline focus model is per-widget claim-based (a widget claims focus itself, from its own event
pass, in response to a `Down`) with no orchestration moving focus between controls or on mount.
`button/mod.rs` and `split_button.rs` each record it verbatim as an "Honest limitation" ("frust has
no keyboard focus traversal … no autofocus, no `FocusNode`, no Tab-ring"); `slider/mod.rs`'s thumb
ring is ported and painted, but "[n]othing focuses a slider today" for the same reason.
`dropdown/mod.rs`'s Up/Down/Enter/Escape panel handling is gated on the identical claim-based
model — the panel only sees key events once it holds focus, which it claims on a press inside
itself, so a caller must complete one pointer interaction with the panel before the arrows do
anything; the module doc names this "no auto-focus-on-appear hook and no focus-traversal seam in
the framework."

**Applies to**: every interactive `frust_material` component — the underlying claim-based focus
model is framework-wide, not a per-widget choice. Module docs call the gap out explicitly for
buttons, split button, sliders, and the dropdown panel; every other focus-visual seam in the
catalog (menus, dialogs, text fields) rides the identical model without its own restatement. See
`overlay-no-auto-focus-on-appear` for the sibling gap this entry does not re-cover: no widget
(material's `dialog`/`search` included) can claim focus programmatically **on mount** either —
that entry is the register's existing home for the on-appear half; this entry covers the
between-controls traversal half.

**Why accepted / fix direction**: framework-level work — a real fix is a `frust-widgets`
Tab-ring/traversal primitive (focus order, Tab/Shift-Tab movement) plus a claim-on-mount hook,
neither of which exists yet. Every current design-system component ports its focus **geometry**
ahead of that primitive so it activates for free the day traversal lands, rather than blocking the
port on it.

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

### `scroll-physics-snap-not-shipped` — no snap physics (page/fixed-extent/carousel) ships yet

**Observed**: `crate::physics` ships four platform-parity/utility physics (`Bouncing`, `Clamping`,
`AlwaysScrollable`, `NeverScrollable`) plus `RubberBand`; nothing implementing Flutter's
`PageScrollPhysics`/`FixedExtentScrollPhysics` family (a release that ballistically snaps to the
nearest page/item boundary) exists in this crate. The `ScrollPhysics` trait's chaining contract
(`.chain(parent)`) is designed to accommodate one — a snap physics overriding only
`create_ballistic_simulation` and delegating everything else to a chained parent — but no such type
is written.

**Applies to**: any app wanting a paged/carousel-style scroll surface built on the baseline
`ScrollView`/`ListView` rather than a hand-rolled widget — concretely, both catalog carousels
(`frust-shadcn`'s `carousel()` and `frust_material`'s `carousel` family) hand-roll their own snap
engines instead, precisely because this physics doesn't exist to compose over yet (see
`scroll-physics-shadcn-carousel-unmigrated` for both).

**Why accepted**: the seam was built parity-first (matching the platforms' own default feel); a snap
physics is straightforward follow-up work on the same trait, not a design gap.

**Evidence**: `crates/frust-widgets/src/physics/` (module inventory — `parity.rs`/`rubber_band.rs`
are the only concrete `ScrollPhysics` impls); `mod.rs`'s `ScrollPhysics` trait docs, *Chaining*.

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
`ScrollPhysics` change (e.g. a future snap physics, `scroll-physics-snap-not-shipped`)
automatically.

**Why accepted**: there is no page/fixed-extent snap physics to compose over yet (see
`scroll-physics-snap-not-shipped`), and neither upstream's scroll engine (embla for shadcn, M3E's
own weighted-carousel math for material) has a frust-side package to wrap — both ports' own module
docs record the re-implementation as a deliberate v1 choice. Migrating either onto the physics seam
is future work once a snap physics exists; the two hand-rolled engines are not required to converge
with each other first.

**Evidence**: `plugins/shadcn/src/components/carousel.rs` module docs (*Drag, and what ends it*,
*Motion*); `plugins/material/src/carousel/mod.rs` module docs ("Snapping, and the velocity that
picks the target") and `plugins/material/src/carousel/physics.rs` module docs ("Ramp shape: a
duration, not a spring") — no `crate::physics`/`ScrollView`/`ListView` dependency in either carousel.

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
lives with (**R-B3-inner**, `nav::ambient`'s `SWIPE_CLAIM`) — arbitrating once at `Down` is what
keeps the takeover decision a single read rather than a per-`Move` re-evaluation; the fix is a
strictly larger seam (a live-updating claim), not a bug in the one shipped.

**Evidence**: `crates/frust-widgets/src/scroll.rs`'s `SCROLL_CLAIM`/`InnerScrollState`/`deferring`
module docs (*Nested scrolling: innermost wins*).

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
momentum it should cancel); the stall guard is a refinement on top, not ported in this pass.

**Evidence**: `crates/frust-widgets/src/scroll.rs` and `list_view.rs`'s `fling_start_velocity` doc
comments (*Accepted gap*).

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

**Evidence**: `crates/frust-widgets/src/scroll.rs` and `list_view.rs`'s `ballistic_is_pinned_outward`
doc comments; `list_view.rs`'s local *One caveat* note on the same method.

### `testing-vello-cpu-layer-recursion` — `vello_cpu` 0.2.0's nested-layer compositing recurses ~O(depth); deep-layer test renders need an oversized stack

**Observed**: the adversarial golden case `adv-5k-layers` (5,000 nested `PushLayer`/`PopLayer`)
overflows the default ~8 MiB `cargo test` thread stack inside `vello_cpu` 0.2.0's layer
compositing, which recurses roughly once per open layer (bisected: 8 MiB fails, 16 MiB
succeeds). This is stack depth, not heap: the case's own 128 MiB heap-delta budget holds
comfortably (counting allocator + `VmHWM` cross-check).

**Why accepted**: `vello_cpu` is a version-pinned external dependency (dev-only oracle pin
`=0.2.0`); its compositing internals are not ours to restructure, and the recursion only bites
at layer depths far beyond any real widget tree.

**Workaround**: `crates/frust-testing/tests/adversarial.rs` runs every corpus-wide render on an
explicit 64 MiB thread (`run_on_oversized_stack`), documented inline.

**Evidence**: `crates/frust-testing/src/corpus/adversarial.rs`'s `adv-5k-layers` case and
`tests/adversarial.rs`'s `five_thousand_nested_layers_stay_within_a_bounded_memory_budget`.

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
faithful. One case stays unbounded: a dash pattern is expanded into sub-paths
before any culling, so a tiny dash period over a huge path never returns.

**Trigger for removal**: a widget-visible need for a drawn-anyway fallback
semantics for non-finite input appears (none identified today).

### `engine-image-minify-to-fit` — an image wider than one atlas layer renders minified

**Observed** (evidence: `crates/frust-engine/src/cache/images.rs`'s
`fit_extent`/`minify` and `crates/frust-engine/tests/images.rs`): the engine's
image path is atlas-resident, and a source larger than one atlas layer
(capability-chosen per-layer extent, mobile 1024² / desktop 2048², overridable
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

### `engine-scheduler-skip-on-escalation` — a layer shape the engine's scheduler cannot serve skips the frame rather than falling back

**Observed** (evidence: `crates/frust-engine/src/schedule/mod.rs`'s module
contract and the `RenderPath::EngineDirect` submit arm in
`crates/frust-render/src/renderer.rs`): the engine tier carries no second
renderer — a frame whose layer shape the scheduler cannot serve returns
`EngineError::SchedulerEscalation` and the surface presents nothing for that
frame: the previously presented content persists, a refusal counter
increments, and a rate-limited log names the reason — and since the shape is
a property of the layer tree, a shape that refuses once refuses every frame,
freezing the surface on its last presented image while the app keeps running.

The served set is wider still since the bounded third (spill) page landed
(p7-f2): isolated sibling fans of any width, a nested isolated chain of any
depth up to the chain bound, same-parity sequential page reuse, and — the one
shape the spill page exists for — a chain hanging off an isolated ancestor's
later child (a full-screen layer at fractional opacity holding a chip that
itself carries a nested chip, the navigation-scrub shape) are all served
within `MAX_LIVE_PAGES` (3) live pages; a single-primitive Gaussian blur or a
shadow-only drop shadow is planned and executed as a filter-round sequence,
sized by `filter_page_size` (coverage plus the filter padding, quantized,
refusing with `IntermediateTextureTooLarge` past the adapter ceiling and
`IntermediateTextureLimitReached` past the pool's own budget). What still
refuses: a chain deeper than the chain bound; a layer whose own round would
need to sample two live pages while a third is still owed to a round above it
— a fourth live page, past what the one bounded spill page extends to; a
filter that is not one of the two served kinds (flood and the
composite-original drop shadow are reserved — the latter needs a third live
page); a filter layer recorded inside another layer; and non-default blend or
mask layers.

**Accepted because**: the engine carries no second renderer to fall back to on
one surface; skipping loudly beats rendering the shape wrong.

**Trigger for removal**: the served set must eventually cover the widget
tree's real shapes — the opaque-pass hoist and the bounded spill page have
each narrowed the refused class in turn rather than closed it; the general
algorithm this narrows from is a stub behind the `full-scheduler` feature.

### `engine-glifo-atlas-experimental` — the engine's glyph atlas is built on a cache upstream labels experimental, wrapped in frust-owned policy

**Observed** (evidence: glifo 0.3.0 `glyph.rs`'s atlas-cache doc — "highly
experimental and not recommended for external use" — and
`crates/frust-engine/src/text/atlas_policy.rs`): the engine caches settled
glyph runs in a GPU atlas driven by glifo's cacher, which keys font size by
exact `f32` bits and leaves eviction/layout policy to the integrator. frust
wraps it in its own routing policy rather than trusting the defaults: the
policy decides *which runs are offered* — keyed on the quantized (1/4-px)
**device-space** size, the same `font_size × absorbed-uniform-scale` quantity
glifo keys — and routes to outline strips any run whose device size changed
inside an 8-frame window, any run above the size ceiling, any COLR-carrying
face (the tier cannot replay glifo's recorded COLR command stream and glifo
offers no way to withdraw entries afterwards), any transform glifo will not
absorb as a positive uniform scale, and any run whose demand would push the
resident population past the glyph texel budget. That budget is one running
total — every resident entry charged a per-entry texel cost at its own size,
rationed against the glyph share of the array (so no size class's refusal
depends on another's counts, and mixed sizes bound additively) — and it is a
deliberate **over-estimate maintained incrementally** from admit/evict
deltas, not a texel guarantee: glifo exposes no release-build way to price
the resident population, a glyph wider than its em is under-charged (the
packer still refuses independently), and a run is charged its distinct glyph
ids at admission, so subpixel-bucket variants can overshoot by that factor.
glifo's `PendingClearRect`s and recorded page replays are re-offered every
frame until the renderer acknowledges they were serviced, and glifo's
eviction pass is deferred only while an unreplayed recording has outlived
the entry-age window (a serial-based check — younger recordings name nothing
reapable), so a dropped frame loses nothing and a recorded command cannot
outlive its slot.
Glyphs share one `ImageCache` allocator with images so slot ids and
rectangles cannot collide. `FRUST_ENGINE_NO_ATLAS` routes every glyph to the
outline path — a correct, slower, tested fallback. A 200-frame scale-animated
wired-path test bounds resident entries and keeps images resident, and the
atlas-drawn and outline paths are byte-identical for a whole-pixel-placed run
on the T400 reference adapter.

**Accepted because**: the outline fallback is always live (animating sizes,
oversized glyphs, refused allocations, the kill switch all take it), so an
atlas defect degrades to the slower correct path rather than wrong pixels;
and the policy layer pins every behaviour upstream leaves open.

**Trigger for removal**: glifo stabilising its atlas surface, or the swap
phase absorbing the vendored core.

### `engine-bitmap-glyphs-gap` — bitmap-strike (PNG/BGRA/Mask) emoji do not render on the engine tier; COLR emoji do

**Observed** (evidence: glifo 0.3.0 `glyph.rs` — `BitmapData::Png` decodes
only under glifo's `png` feature, `Bgra`/`Mask` return `None` unconditionally;
`crates/frust-engine/src/text/backend.rs` counts such glyphs in
`skipped_glyphs`): the engine leaves glifo's `png` feature off because frust
already ships a PNG decoder (`image` 0.25.10) and a second one is pure
dependency weight, so a PNG-strike emoji glyph goes missing rather than
landing wrong; BGRA and Mask strikes are undecoded upstream regardless.
COLRv1 colour glyphs are the supported emoji path
(`crates/frust-engine/src/text/color.rs`). Within COLR, a glyph whose paint
graph needs a shape the engine cannot express exactly — a layer inside an
isolated blend bracket, or two nested outline clips at once — is dropped
whole and counted, never half-painted.

**Accepted because**: the bundled and platform emoji fonts frust targets ship
COLR tables; a wrong-pixels approximation of an inexpressible COLR shape
would violate the backend's never-wrong-pixels invariant; and decoding PNG
strikes properly belongs to a routing seam that hands glifo frust's own
decoder rather than enabling a duplicate one.

**Trigger for removal**: a glifo release taking an external PNG decode hook
(or decoding BGRA/Mask), or a measured need for a bitmap-strike font on a
target platform.

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
gates compare unhinted-vs-unhinted (tracked as an open action item), or a
DPR-aware policy replacing the binary device-class switch.

### `engine-metal-postmultiplied-truth-bug` — wgpu-hal's Metal `PostMultiplied` composites premultiplied regardless of its name

**Observed** (evidence: `crates/frust-gpu/src/surface.rs`'s
`compositor_expects_premultiplied` doc comment, citing
`wgpu-hal-30.0.1/src/metal/adapter.rs:468-471` and
`.../metal/surface.rs:269-273`, re-checked against the 30.0.1 pin at p8-07):
wgpu-hal's Metal adapter still advertises `CompositeAlphaMode::PostMultiplied`
but implements it as nothing beyond `render_layer.setOpaque(false)` — it never
asks Core Animation for straight alpha, and a `CAMetalLayer` has no such mode;
Core Animation only ever composites premultiplied. A Metal `PostMultiplied`
swapchain therefore reads back exactly like a premultiplied one even though
the mode's name and wgpu's advertised contract say straight — the same family
of upstream truth-bug as the iOS Simulator's missing `INDIRECT_EXECUTION`
(pre-engine; `docs/DEVELOPMENT.md`'s iOS Simulator Known Issue): a real
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
(merged 2026-08-18, CHANGELOG Unreleased) already fixes this — Metal will
advertise `PreMultiplied` and reject `PostMultiplied` with `UnsupportedAlphaMode`
— but it is not in the 30.0.1 pin. The next wgpu bump that carries it is the
trigger: `compositor_expects_premultiplied` collapses to nothing (and, being a
BREAKING wgpu-hal change, needs its own re-scrub of every backend/mode
combination this predicate routes on). No upstream issue is filed by this
workspace.

### `engine-metal-timestamp-drawless-pass-gap` — a Metal render pass with no draw can leave its GPU-timestamp pair unwritten

**Observed** (evidence: `crates/frust-gpu/src/diag.rs`'s `TimestampRing`
module doc, "A pass that draws nothing may measure nothing"): Metal samples a
render pass's counters at the vertex/fragment stage boundaries, so a pass
that runs neither stage — a pure clear, an empty pass — can leave its query
pair unwritten; `TimestampRing` drops the pair rather than reporting a
garbage span. A measurement gap only: the untimed pass still draws (or
doesn't draw) exactly what it always did, and every reported span
(`gpu_prepass_us`/`gpu_main_us`/`gpu_composite_us`/`gpu_blit_us`) is
ordinarily several passes, which absorbs it.

**Accepted because**: it degrades to a smaller sample, never a wrong number,
and every shipping span is normally a mix of drawing and non-drawing passes.

### `engine-punch-straddles-layer-composite` — an isolated layer whose content straddles a `ClearRect` composites whole after the punch

**Observed** (evidence: `crates/frust-engine/src/compile/clear.rs`'s module
doc, point 3's "One shape the cut cannot express"): the renderer orders a
`ClearRect`'s destination-out punch against the frame's own rounds by cutting
the surface round at the punch's painter-order depth (p7-f3). An isolated
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

**Trigger for removal**: the swap phase's full scheduler, if it composites a
layer in more than one pass.

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
tiers already pay for their own first-use pipeline compile (see
`engine-dx12-cold-start` above) — the fallback is correct by construction and
now measured cheap rather than merely assumed so.

**Trigger for removal**: a `SharedArrayBuffer`/Worker-backed pool that gives
`wasm32` a real background thread for this queue, if the measured cost above
ever needs to shrink further.

### `engine-dx12-cold-start` — the engine's pipeline warm-up pays its whole cost inside the first-ever DX12 launch; every later launch is cheap (MEASURED)

**Observed** (evidence: the retired "Windows DX12" section of `benchmarks/RESULTS.md`, in
git history (`git show f64be636:benchmarks/RESULTS.md`); p7-08, Dell mini PC/Intel UHD 730/i5, `material3-demo` release build, 5 cold
launches per arm via schtasks): wgpu's persisted `PipelineCache` is
Vulkan-only, but the DX12 driver-level shader cache persists across
processes, so run 1 after a fresh build is the true first-ever-launch cost
and runs 2-5 are the everyday warm-cache cost. Measured: **first-ever
launch** 588 ms TTFF on the engine (188.6 ms of it the in-frame
pipeline-warm-up compile) vs 1066 ms classic (~972 ms in adapter/device
init, 1.8x the engine's); **warm launches** are process-bound and
near-identical across arms (~200 ms TTFF), but warm first frames are
1.3-1.7 ms on the engine vs 18.8-27.1 ms classic (~14x), steady frames ~4x.
The GL/ANGLE fallback arm is not measurable on this rig (`WGPU_BACKEND=gl`
fails to create a surface — the backend is not compiled into the desktop
build); no toggle exists in shipped code to measure the eager-steal-off arm
separately.

**Accepted because**: this is a recorded number, not a defect — the engine's
warm-up-in-first-frame design costs more up front than nothing, but still
lands the first-ever launch faster than classic's, and every launch after
the first is markedly cheaper on the engine.

### `engine-webgl2-atlas-target` — wgpu-hal's GLES backend binds a one-layer atlas as `GL_TEXTURE_2D`, so every glyph and atlas image reads as blank on WebGL2

**Observed** (evidence: `examples/web-spike/RESULTS.md` § 17; upstream wgpu
issues #1614/#1574): `wgpu-hal` 30.0.1's GLES backend picks a texture's GL
target from the `wgpu::TextureDescriptor` alone (`get_info_from_desc`,
wgpu-hal `src/gles/mod.rs:513`), never consulting the view dimension a
sampler later asks for. `frust-engine`'s image/glyph atlas is a `D2` texture
with `depth_or_array_layers == 1` until a second layer is needed, sampled
through a `sampler2DArray` (`D2Array` view) — target and sampler disagree,
so GLES 3.0's incomplete-texture rule makes every sample read `(0,0,0,1)`:
every cached glyph paints as a solid box, every atlas image as an opaque
black rect. The desktop Vulkan/Metal/DX12 backends are unaffected; this was
the one thing standing between the browser tier and a full WebGL2 match with
WebGPU (2026-09-08).

**Accepted because**: the workaround has landed — `atlas_texture_descriptor`
(`crates/frust-engine/src/gpu/atlas.rs`) now floors `layers` at 2, and both
1x1 array placeholders (`gpu/atlas.rs::placeholder`, `renderer.rs::placeholder_view`)
allocate 2 layers, keyed off the exact condition wgpu-hal's own heuristic checks
(`the_layer_floor_is_two_because_wgpu_hal_gles_ignores_the_view_dimension` pins
the reason so a later tidy-up cannot silently restore the one-layer floor); verified
end to end (`examples/web-spike/RESULTS.md` § 17.4: the WebGL2 arm renders shaped
text, images, gradients, blur and layers identically to the WebGPU arm, with the
host Vulkan engine goldens unaffected) and held in place by an ongoing automated
gate — `crates/frust-testing`'s `webgl` feature (`tests/wasm_goldens.rs`,
`tests/wasm_binary_invariants.rs`) renders the same corpus in headless Chrome and
PASSED on the Linux rig (see [RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md)'s
`FRUST_ENGINE_DOWNLEVEL` row). The memory cost is one extra atlas layer,
paid only in the single-resident-layer case: 4 MiB at the MOBILE 1024² tier, 16 MiB
at the DESKTOP 2048² tier (both RGBA8), plus 4→8 bytes for the two 1x1 placeholders.
This entry remains as the record of the upstream constraint itself — `wgpu-hal`
30.0.1's GLES backend still derives a texture's GL target from the descriptor alone
(`get_info_from_desc`, wgpu-hal `src/gles/mod.rs:513`; upstream issues #1614/#1574) —
not as an open defect in frust.

**Trigger for removal**: a wgpu-hal release that honours the requested view
dimension instead of the descriptor's own layer count, removing the need for
the floor entirely.

### `engine-shader-quad-goldens-uncomparable` — a shader quad renders on the engine but no golden oracle can score it

**Observed** (evidence: `crates/frust-engine/src/effects/shader_quad.rs`
renders each frame's `ShaderProgram`s in a pre-pass and the compiler lowers
`Command::ShaderQuad` through the external-texture path; `examples/shadertoy`
renders its showcase on the engine tier): the command now draws, but neither
golden arm can compare it — `CpuOracle` has no shader compiler and keeps its
opaque placeholder stand-in, and `EngineOracle` encodes straight through
`EngineRenderer` without driving the pre-pass, so the quad's id resolves to
nothing there. `crates/frust-testing/tests/engine_goldens.rs` therefore keeps
`unit-shader-quad` in `DEFERRED_CASES` and still lists `ShaderQuad` in
`engine_skipped_commands`, now attributed to the oracles rather than the
compiler. `FRUST_ENGINE_NO_SHADER_EFFECTS` disables the pre-pass and the draw;
its runtime effect is covered by a manual negative run, not by a same-binary
test, because the switch is a process-global `OnceLock`.

**Accepted because**: a golden for a user-supplied fragment shader would pin
the shader's own output, not the engine's; the engine-side invariants (target
sizing, premultiplied compositing, reaping, run batching) are pinned by
`crates/frust-engine/tests/shader_quad.rs` on a real device instead.

**Trigger for removal**: an oracle that drives the pre-pass (or a
device-backed golden class for shader quads), at which point the deferral and
the skip-list row come out together.

### `engine-scene-texture-always-blended` — an external texture never joins the opaque depth-writing pass

**Observed** (evidence: `crates/frust-engine/src/compile/external.rs` sets
`may_have_transparency` unconditionally; `Command::SceneTexture` and the
`bind_texture` registration carry no opacity statement): the engine never
reads a caller's texels, so every `SceneTexture` and `ShaderQuad` draw takes
the blended painter-order path even when the texture is fully opaque (a video
frame, an opaque 3D render), paying the blend and forgoing depth occlusion of
what lies beneath it. Sampling is also unconditionally bilinear
(`ImageQuality::Medium`): the whole-pixel-translation nearest downgrade
`Command::Image` gets from `compile::paint` is not shared with the external
path; at a 1:1 pixel-aligned mapping the difference rounds away in `u8`.

**Accepted because**: claiming opacity the engine cannot verify would let a
translucent texture occlude what it should have blended over; drawing
correctly and slowly beats drawing wrong.

**Trigger for removal**: an opacity hint on the scene command or on
registration, and a shared sampling-quality resolver for image and external
paints.

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

### `facade-gpu-context-desktop-only` — `frust::gpu::with_context` answers `None` on Android and iOS

**Observed** (evidence: `crates/frust-shell-desktop/src/render.rs`
`publish_gpu_handle` installs the shared `DeviceHandle` at both
device-creation sites; `crates/frust-shell-android/src/lib.rs` and
`crates/frust-shell-ios/src/lib.rs` forward the `gpu` feature but never call
`frust_shell_common::gpu::install_gpu_handle`): a `gpu`-feature app reaches
the shared device on desktop only. The mobile shells' device-creation sites
(their JNI/FFI glue and executors) are not yet wired to the slot.

**Accepted because**: the seam's first consumers are desktop-first; the slot
and the facade accessor are platform-free, so wiring a mobile shell is a
local change at its device-creation site.

**Trigger for removal**: both mobile shells install the handle and a device
run (Pixel 5, iPhone) reads it back through `with_context`.

### `engine-ios-sim-seam-suites-unrun` — the engine's seam test suites have not been executed on the iOS Simulator

**Observed** (evidence: the retired `benchmarks/raw/mac/README.md`'s iOS Simulator table — in git
history, `git show f64be636:benchmarks/raw/mac/README.md` — gate `engine-p9-ios-sim-seam`): the `frust-engine` `scene_texture`/`shader_quad`/`shared_encoder`
suites have not yet been run on the iOS Simulator. Before `p9-f2`, their fixtures were refused at
`request_device` (`LimitsExceeded { max_inter_stage_shader_variables: requested 16, allowed 15 }`)
against the Simulator's Apple2 adapter, which reports only 15. The fixture fix is in —
`frust_gpu::test_device_limits` now derives the request from the adapter/`TierCaps` the same way
`create_device` does — but the Simulator re-run itself is still owed to the Mac rig, so gate
`engine-p9-ios-sim-seam` is recorded skipped for these three suites. `ShaderQuad`/`SceneTexture` on
the Simulator is proven only by the shadertoy app path (gate
`engine-p9-ios-sim-shadertoy-app`), not by the suites themselves.

**Accepted because**: the refusal was a fixture bug, not an engine or Metal deviation —
`ios_sim.rs`'s own device passes on the same adapter because it already derived its limits this
way — and the app-path run already exercises the same commands end to end; only the suites'
own Simulator execution is outstanding.

**Trigger for removal**: the three suites run to completion on the Simulator (Mac rig) and the
result is recorded as a gate on the phase (`benchmarks/RESULTS.md` holds only the Frust-vs-Flutter
matrix now).

### `facade-external-pass-desktop-only` — an `ExternalPass` has only ever run against a headless target, on desktop

**Observed** (evidence: `crates/frust-render/src/renderer.rs`'s `submit_impl`, both
`RenderPath::EngineDirect` and `RenderPath::EngineDirectUnpremultiply` arms call
`external_pass::run_external_passes` unconditionally (`renderer.rs` ~1165 and ~1294) immediately
before `shader_quads.prepare`, into the same frame encoder; those are the only two arms `submit_impl`
has, so the drain runs on every engine-tier `SurfaceRenderer` frame path with no platform gate —
`crates/frust-shell-android/src/app/render.rs:174` calls `renderer.submit`, and
`crates/frust-shell-ios/src/app/executor.rs:592`/`599` call `submit_deferred`/`submit`, both reaching
the same unconditional drain: a pass registered while an Android or iOS shell is running is invoked
every frame there, exactly as on desktop. What has actually been *exercised* is narrower:
`crates/frust-render/tests/external_pass.rs`'s two `#[ignore]` GPU cases drive that identical
drain→encode→submit sequence directly — against a `HeadlessTarget`, not a real swapchain — run on an
NVIDIA T400 4GB/Vulkan adapter on 2026-09-03: a pass-rendered 64×64 solid target composited at its
registered destination with the base colour intact outside it, and unregistering it stopped the draw
the next frame. No windowed run of any kind, and no mobile device run of any kind, has registered and
driven a real pass yet — that is an untested surface, not a gated one; the `frust::gpu::with_context`/
`install_gpu_handle` desktop-only slot (`facade-gpu-context-desktop-only` above) is the shared
`DeviceHandle` facade accessor, a separate mechanism that `run_external_passes` does not consult and
so does not gate this drain. `HeadlessRenderer` never drains the registry at all (see the module
docs). Output is always blended, never opaque (`engine-scene-texture-always-blended` above) — the
same rule an unproven mobile pass would still be under.

**Accepted because**: the seam's first and only exercised host is the engine-tier frame path itself,
proven headless; a real window and a mobile shell need no new code in the registry, only a run.

**Trigger for removal**: a windowed desktop run and a mobile-shell run (Pixel 5 / iPhone), each
registering and driving a real `ExternalPass` through its own frame loop.

### `external-pass-panic-isolation-dev-only` — a panicking `ExternalPass` is only isolated in a build that unwinds

**Observed** (evidence: root `Cargo.toml`'s `[profile.release]` sets `panic = "abort"` (also recorded
in `docs/PERFORMANCE_BASELINES.md`'s Release-profile hardening); `crates/frust-render/src/external_pass.rs`'s
`run_external_passes` wraps each pass's `record` call in `catch_unwind`, reporting the first panic of
an id at `warn!` and every later one at `debug!`, retiring the pass and queuing its binding for
unbind): `catch_unwind` keeps a panicking pass from taking the frame's encoder — and every sibling
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

### `external-pass-registry-process-wide` — one process-wide registry serves every live `EngineRenderer`, so a second surface can be left holding a stale binding

**Observed** (evidence: `crates/frust-render/src/external_pass.rs`'s module docs, "Who reaches this";
`static REGISTRY: Mutex<Registry>` is one process-wide map, while `run_external_passes` is called once
per surface, each time with that surface's own `&mut EngineRenderer`): a registered pass is recorded
into every live engine-tier surface's frame from one registration — the intended behaviour, so a
caller registers a pass once for every surface it wants it on. `unregister_external_pass`, though,
queues exactly one pending unbind per id in the shared map, and whichever surface's drain
(`run_external_passes`) runs first for the next frame takes and clears the whole queue: only that
surface's `EngineRenderer` unbinds the id. A second live surface's `EngineRenderer` keeps the stale
`wgpu::TextureView` bound — the pass is gone from the registry, so nothing calls `record` again to
refresh or clear that surface's own binding, and it stays stale until something re-registers under the
same id (which restores `record` calls, and so re-binding, on every live surface again).

The same one-registration-per-id design means the identical `Arc<dyn ExternalPass>` is what every live
surface's `run_external_passes` calls `record` on, and `SurfaceRenderer` drives each surface's
frame on its own dedicated render thread by default (`RENDER_ARCHITECTURE.md`'s Module Structure):
with two engine-tier surfaces live at once, nothing serializes their drains against each other, so the
same pass instance's `record` can be called from two threads at the same moment. `ExternalPass: Send +
Sync` has to mean tolerating that concurrent invocation — a pass whose interior mutability assumes only
sequential reuse (one surface's drain finishing before the next one starts) is unsound under two live
surfaces, not merely stale.

**Accepted because**: a single-window desktop shell is the only engine-tier host any shell ships today
(`desktop-single-window` above); a per-surface registry is real design work with no shipping consumer
to prove it against yet.

**Trigger for removal**: a per-surface registry keyed by surface identity, or the first shell that
keeps two engine-tier surfaces live at once — whichever lands first.

### `shell-ios-first-frame-above-goal` — the iOS split executor's first frame still lands above the 85 ms goal

**Observed**: overlapping the render thread's GPU bring-up with the UI thread's font-preinit join
(see [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)'s iOS render-thread split) narrowed but did
not close the gap: the iPhone SE's first frame lands 64-176 ms (median 100) on the published 2026-09-06
pass in `benchmarks/RESULTS.md`, against a 110 ms pre-overlap baseline and an 85 ms goal (an
earlier 12-launch probe on the same build read 89-115 ms against a 99 ms baseline). `TextContext::new`'s font preinit is bimodal — 51-65 ms on the fast path,
100-159 ms on the slow one, the first frame trailing the join by 13-23 ms — and once GPU bring-up runs concurrently underneath it, the slow mode is
what sets the median first-frame cost. A render-thread-style QoS boost applied to the font-preinit
thread was measured and did not close the gap either.

**Accepted because**: the render/font overlap already recovers most of the achievable win without
adding synchronization risk to the split; the remaining cost sits inside `TextContext::new`/Parley's
own font matching, which this shell does not own.

**Trigger for removal**: `TextContext::new`'s bimodal cost is made deterministic, or font preinit
moves off the first-frame critical path entirely.

### `shell-ios-pace-trace-artefact-only` — the "3.5% of S3 frames take two vsyncs" reading was a measurement artefact

**Observed**: the `ios-pace` diagnostic (`FRUST_PACE_TRACE`, see
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)) traced the earlier reading to `FramePasses::total`
— a sum of the UI thread's and the render thread's concurrently-running spans, not a wall-clock
interval — being read as if frames were missing a vsync. The real present-to-present cadence on the
iPhone SE is a locked 60 Hz with no evidence of dropped vsyncs, so no frame-path change follows from
that reading. Pre-acquiring a drawable ahead of need (the one candidate lever it might have
motivated) was tried and rejected regardless — a drawable parked across a `Pause` is a UIKit
watchdog hazard.

**Accepted because**: the diagnostic exists precisely to keep a future pacing claim honest, and this
reading turned out not to name a real regression.

**Trigger for removal**: a present-to-present (not cost-sum) measurement on a real device showing
actual dropped vsyncs.

### `engine-atlas-tier-by-driver-flag` — atlas tier is chosen by a single driver-reported flag, not a memory-class policy

**Observed**: `AtlasBudget::for_caps` (`crates/frust-engine/src/cache/images.rs`) picks the desktop
budget (2048x2048 x8 layers) or the mobile one (1024x1024 x4 layers) solely off
`TierCaps::transient_saves_memory`, itself read from `wgpu::AdapterInfo`. Two Android devices of
similar class resolve differently: the Pixel 5 (Adreno 620) resolves the desktop budget, the
OnePlus 9 (Adreno 660) resolves mobile. Functionally harmless today — both devices show zero atlas
skips — but the memory-class decision is made by a driver capability flag never intended as a
memory-tier signal.

**Accepted because**: no skip or measurable cost has been observed on either device; guessing at an
adapter-name or device-class heuristic ahead of an actual failure would trade one arbitrary signal
for another.

**Trigger for removal**: an adapter-class heuristic, or a shell-provided memory-class hint
`AtlasBudget::for_caps` can read instead of the driver flag alone.

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

### `shell-ios-tests-cannot-link` — `frust-shell-ios`'s lib tests cannot link on any Apple target

**Observed**: the crate's two test-only macro-expansion modules (`macro_expansion`,
`macro_expansion_state_factory` in `crates/frust-shell-ios/src/ffi_glue.rs`) each invoke `ios_app!`,
which stamps out an `extern "C" fn frust_destroy` (among other exports); compiled into the same
linked test binary, the two clash. iOS-gated unit tests (e.g. the `pace_trace` formatter tests) are
therefore verified only by `cargo check --target aarch64-apple-ios-sim --tests` (or
`--all-targets`), which compiles but never links — never by an actually-run `cargo test` on an Apple
target.

**Accepted because**: the macro-expansion smoke tests need to exercise real macro expansion on the
iOS target, and no real `cdylib`/app binary ever links this crate's own test harness in.

**Trigger for removal**: gating the macro-expansion test modules behind a dedicated cfg so at most
one of them compiles into any one linked test binary.

---

### `corner-insets-ios-26-only` — `WindowInsets::corner_insets` reports zero on all platforms except iPadOS 26+

**Observed**: `WindowInsets::corner_insets` is non-zero only on iPadOS 26+ under the system window control. Android, desktop, web, and iOS < 26 always report zero (by construction: no other shell reads a corner region), so the Glyph and Material app bar shifts never fire there. Bars do not consume the horizontal safe-area insets; corner widths are measured from the safe-area edge, so a hypothetical control on a notched horizontal edge would under-shift (no platform draws one there). Additionally, a bar hosted in a detail pane, sheet, dialog, or below other content cannot detect its window-space origin and still receives the window-wide corner values, causing over-shift; `corner_shift(false)` is exposed as the author's opt-out. Several bars do not honour corner insets at all: the `frust_cupertino::navbar`, `frust_material::selection_app_bar`'s contextual face, and shadcn/beUI header components.

**Applies to**: Android, desktop, web, and iOS < 26; the `frust_glyph::app_bar` and `frust_material::{app_bar, search_app_bar, sliver_app_bar}` widget implementations; also `frust_cupertino::navbar`, `frust_material::selection_app_bar`, and shadcn/beUI headers.

**Why not fixed**: nothing to report elsewhere. The notched-edge case has no producer. Android's edge-to-edge model has no corner control. Automatic placement detection to disable shifting would require a scoped context cleared by split views and sheets — a design change deferred to a follow-up plan.

**Watch item** (not reproduced on iOS 26.2): a developer-forum report that UIKit's corner layout guide does not reset to zero when a window returns to full screen — gate g4-01 observed the guide reset to zero on entering full screen and come back non-zero on return to a window; `.minimal` reports zero corners because the control takes a safe-area strip instead.

**Evidence**: device gate PASSED 2026-10-01 on the iPad Pro 13-inch (M5) iOS 26.2 Simulator (Xcode 27.0; 12 legs — windowed readout TL 66x43 / TR 10x43, bar slot clear, full screen zeros, rotation, Stage Manager resize, automatic/unified/minimal styles, RTL, no-regression).

---

### `bench-sub-markers-off-frame-thread-never-emitted` — a scenario sub-marker raised from a pool thread is dropped, not queued

**Observed**: `frust-shell-common`'s scenario-marker route (`crates/frust-shell-common/src/perf.rs`)
is per-thread: a marker raised on a thread only reaches a recorded frame if that same thread later
calls `RenderSender::send_scene` (which drains its own thread-local queue). A `spawn_blocking` pool
thread never calls `send_scene`, so a sub-marker raised there — `frust_bench`'s `s8-write`/
`s8-read`, `d1-*`, `d2-*` — is raised and then simply never emitted; the surrounding scenario's
inline `scenario=` op rows still land and remain the record of what happened.

**Accepted because**: the route's per-thread design is what keeps it lock-free on the frame path,
and every scenario the gap affects still self-reports through its inline op rows.

**Trigger for removal**: re-siting the affected sub-markers onto the UI thread, or a route that
forwards an off-thread marker onto the next frame some other way.

### `engine-text-glyph-lowering-dominates-encode` — text-heavy CPU encode cost is dominated by glyph lowering even on atlas hits

**Observed**: on S3/S6-shaped scenes (iPhone SE and OnePlus 9), 87-92% of the compile walk's cost is
spent lowering glyphs even when every glyph is an atlas hit: a cached glyph still pays a
general-purpose flattener and a full image-paint record in
`crates/frust-engine/src/text/backend.rs` rather than a cheaper cached-hit path. A candidate
fast-rect-style shortcut (mirroring the compiler's own axis-aligned rect fast path) is unproven for
glyphs.

**Accepted because**: correctness came first — every glyph draws through one general path rather
than a second, less-tested one — and the cost is measured but has no safe, proven lever behind it
yet.

**Trigger for removal**: a measured, proven fast path for a cached atlas-hit glyph in the text
backend.

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

### `bench-flutter-s2-single-line-parity-break` — Flutter's S2 rows now render single-line; earlier S2 numbers are not comparable

**Observed**: `benchmarks/flutter_bench/lib/scenarios/s2_list.dart`'s rows now set `maxLines: 1`/
`TextOverflow.ellipsis` to match Frust's S2 row geometry. Any S2 number measured before that change
used a different (wrap-capable) row shape on the Flutter side and cannot be compared against a
number measured after it.

**Accepted because**: matching row geometry is what makes an S2 frame-cost comparison meaningful in
the first place; the alternative was an already-mismatched comparison.

**Trigger for removal**: none needed to remove the entry outright — flagged so a reader does not mix
pre- and post-change S2 numbers in one comparison; superseded in practice by the next published S2
pass, which uses only post-change data.

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

### `web-no-startup-spans` — the web shell records no startup span line

**Observed** (evidence: `frust_shell_common::perf::SystemClock`/
`StartupSpans` use `std::time::Instant`, which panics on
`wasm32-unknown-unknown`): every other shell tier emits a one-time startup
span line (rebuild/layout/paint/first-present timing) through that shared
type; `frust-shell-web` cannot construct one without panicking on the very
target it targets, so it emits none.

**Accepted because**: the browser tier's own `web_time::Instant`-based
`perf::UiSpans` still covers per-frame rebuild/layout/paint cost every
frame (see [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)), so only the
one-time startup line is missing, not ongoing frame telemetry.

**Trigger for removal**: `frust_shell_common::perf::SystemClock`/
`StartupSpans` gain a `web_time`-backed `wasm32` arm.

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
(`crates/frust/Cargo.toml`'s `devtools` feature list omits it). (2026-09-08)

**Accepted because**: each gap is a documented no-op with a stated call
site for the eventual real implementation, not a silent absence — see
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md).

**Trigger for removal**: a DOM/ARIA semantics mirror, and a WebSocket-based
devtools transport, each its own future change.

### `web-ime-residual-gaps` — the web IME bridge ships, but gaps remain

**Observed** (evidence: `crates/frust-shell-web/src/ime.rs`'s module doc,
"Cases deliberately not handled"; [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)'s
Web IME bridge): the hidden-`<input>` overlay bridge (real browser
composition — CJK marked text, dead keys, mobile autocorrect — turned into
`ImeEvent::Compose`/`Commit`, and now the DOM `copy`/`cut`/`paste`
listeners too, since a browser hands clipboard access to the focused
editable element and to nobody else) is implemented and unit-tested,
but: (1) it is
**device-unverified** — the build host that landed it has no browser rig, so
no real CJK/IME input has ever been driven through a live browser session,
and two rules the IME bridge review added on 2026-09-08 rest entirely on that
unobserved ground. The first is the **unnamed-keystroke rule**: a `keydown`
the browser reports as `Unidentified` — what a mobile soft keyboard sends for
most of its keys — is not forwarded to the canvas, and the edit it produced is
taken from the `input` signal behind it instead (an `insert*` carrying data
becomes its characters, delivered once as a key event), which is the only
route by which a soft keyboard's letters reach the tree at all. The mark that
arms the carry lives no longer than the signal drain that queued it and is
ended by any keystroke the key path does deliver, so an unnamed keystroke
that changed nothing cannot attach to a later keystroke's echo; that bound is
reasoned from browsers queueing a keystroke's `keydown` and `input` in the
same task, not measured, and the rule itself is derived from winit 0.30.13's
web key mapping producing nothing for an unnamed key, not from a keyboard
watched doing it. The second is the **commit window**: an empty-data
`compositionend` no longer retracts the preedit on the spot, because several
browsers end a composition that way and deliver its text on the `input`
immediately behind it (mobile predictive text, Safari); the preedit is
instead held for the rest of that one signal drain, and an `insert*` `input`
arriving inside it commits. The window's width is reasoned from browsers
firing both signals in the same task, not measured. (2) a mobile browser's
visual-viewport jump when its soft keyboard opens is not followed — the
overlay is placed in layout-viewport coordinates and drifts from the focused
field until the next reposition; (3) the plain-key re-dispatch this bridge
relies on is coupled to winit's own choice to attach `keydown`/`keyup` to the
canvas element rather than `document` — a future winit release moving that
attachment point would silently break the re-dispatch; (4) **a soft
keyboard's Backspace is lost**: it arrives unnamed like the letters, but the
overlay element is emptied on every frame no composition owns, so the
deletion finds nothing to delete and the browser raises no `input` for it —
nothing reaches the tree. A carry for `deleteContentBackward` was added and
then withdrawn on 2026-09-08 as unreachable by construction; the candidate
fix (keep a sentinel character in the element for a deletion to consume,
refilled as it is consumed) changes what the input method sees and has to be
watched on a device first; (5) a field published as `Password` is served by a
`type="password"` element, and such an element may receive no composition at
all — many browsers and keyboards switch secure entry to a plain layout, as
the native platforms do — so a secret field takes text through the key path
and the unnamed-keystroke rule only. A hint that moves under a focused field
is answered from the frame loop as well as from a dispatched event: the
replacement inherits the DOM focus the old element held, and on a mobile
browser that `focus()` runs outside a gesture, so the soft keyboard may close
until the next tap. (6) **one cut can destroy text it wrote nowhere**: a
keyboard cut deletes in the canvas re-dispatch and writes in the DOM `cut`
callback behind it, and the handoff between the two is what makes that
reliable — but an engine that raises no `cut` for the overlay's permanently
collapsed selection (one defining no `beforecut` for the page to claim the
verb with) *and* withholds `navigator.clipboard` (a page that is not a secure
context) leaves no route at all, after the widget has already deleted.
Holding the delete until a write is confirmed is the only answer left, and
the confirmation is asynchronous, so it would have to be pushed back across
the shell/widget seam this bridge does not cross. (2026-09-12)

**Accepted because**: the composition/commit/cancel contract itself is real
and tested (unlike the no-op it replaces), and each residual is independently
scoped; none blocks ordinary text entry, and (4) blocks only deletion from a
soft keyboard — a hardware Backspace, and every deletion inside a
composition, still work. `ImeContentType` is no longer among them — the
overlay is built from the focused field's hint, so a secret field gets a real
`type="password"` element and a suggestion-refusing one gets
`inputmode="text"`. (6) needs three conditions at once (no `beforecut`, no
secure context, a cut rather than a copy), and a page serving an app over
plain `http` has already lost the async clipboard for every other purpose.

**Trigger for removal**: a browser gate exercising CJK composition
end-to-end (closes 1). Coverage so far, stated exactly: the browser gate has
run on Safari only and only partially — the IME legs were not among what it
exercised — the Chrome and Firefox legs are unconfirmed, and Firefox is not
installed on the development machine. A full gate must include, as manual
checks: Safari — compose CJK
text and let predictive text commit it (the commit window); Android Chrome —
type on the soft keyboard with no hardware keyboard attached (the
unnamed-keystroke rule: each letter exactly once), then press its Backspace
(expected: no deletion, which is gap 4 — a deletion that does land is
evidence the sentinel design is not needed); and a field published as
`Password` — confirm the overlay element is `type="password"`, that the
keyboard offers it no suggestions, and record whether composition is
available on it (gap 5). Then: the overlay reading `visualViewport` offsets
(closes 2); a conformance test pinning winit's canvas-attachment choice, or an
upstream `WindowEvent::Ime` implementation removing the need for the bridge
entirely (closes 3); the sentinel design landed with that Android evidence
(closes 4); and, for (6), either a delete deferred until its write is
confirmed or a Firefox leg showing the engine does raise a usable `cut` for
the overlay after all — which needs a Firefox install this machine does not
have.

### `web-generic-family-partial-fallback` — `Monospace`, `Serif` and `Emoji` still resolve no glyph on `wasm32`; `SystemUi`/`SansSerif` are covered by a bundled fallback face

**Observed** (evidence: `crates/frust-shell-web/src/fonts.rs`;
`crates/frust-text/src/context.rs`'s `register_generic_fallback`, re-exported
from `lib.rs`): fontique 0.11.0 ships a "Dummy system font backend for
targets like wasm32-unknown-unknown" whose generic-family map is empty, so a
generic `FontFamily` slot resolves no glyph on the web tier by default.
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
this task's scope covered.

**Trigger for removal**: a bundled monospace/serif/emoji policy, or a
page-side font-loading seam that lets an app supply those faces without
paying for them in every binary.

### `web-canvas-inline-style-resize` — an unstyled host page's canvas never tracks a live browser resize

**Observed** (evidence: `examples/web-gallery/README.md` § "Resize — live,
after a page-level fix"; `examples/web-gallery/index.html`'s
`MutationObserver`): winit sets the canvas's inline `style.width`/
`style.height` in pixels at creation time; an external stylesheet rule
targeting the canvas cannot override an inline declaration in the CSS
cascade, `!important` or not, so a page that styles the canvas only through
a stylesheet never sees it track a later window resize. A plain JS property
assignment (`canvas.style.width = "100vw"`) does override the inline value,
and once it is a viewport-relative unit the browser's own layout recomputes
it on every later resize, which winit's already-attached `ResizeObserver`
picks up correctly.

**Accepted because**: the shell's `WindowEvent::Resized` handling is
correct once the canvas element is sized by anything other than winit's own
inline declaration; `examples/web-gallery/index.html`'s `MutationObserver`
workaround is a two-line, host-page-only fix, not a shell defect requiring
a code change.

**Trigger for removal**: `frust-shell-web` grows a canvas-sizing option
(adopt a host-provided CSS class, or clear its own inline style after
creation) that removes the need for a host page to work around it.

### `web-webgpu-webgl2-runtime-fallback` — the browser tier renders on WebGPU when the page has one, and falls back to WebGL2 automatically otherwise

**Observed** (evidence: `crates/frust-gpu/src/context.rs`'s `ContextOptions::default`/
`RenderContext::with_options`, which every shell — including `frust-shell-web` —
takes unmodified: `backends` defaults to `wgpu::Backends::from_env().unwrap_or_default()`,
i.e. `Backends::all()` on `wasm32` since no process environment exists there;
`wgpu` 30.0.1's own `Instance::new` (`wgpu-30.0.1/src/api/instance.rs`) then
selects its real WebGPU backend only when the requested set includes
`BROWSER_WEBGPU` **and** the page's own `navigator.gpu` property is present,
falling through to the ordinary `wgpu-core`/GLES path — this crate's WebGL2
arm — otherwise): a plain `frust::web_app!` build makes no browser-detection
choice of its own; the fallback is `wgpu`'s, decided once at `Instance::new`,
not a frust-side branch. As of this writing, Firefox ships WebGPU by default
on Windows, with macOS support following behind the same rollout; Linux and
Android do not yet have it by default — whichever is true for a given
Firefox build, this mechanism is what a user actually gets: WebGPU when
`navigator.gpu` exists, WebGL2 (full parity — see `engine-webgl2-atlas-target`
above) when it does not. The forced-WebGL2 arm (Chrome's `?arm=webgl` query
param) is proven in `examples/web-spike/RESULTS.md`; the unforced,
browser-driven fallback branch itself is exercised only by the manual browser
gate ([DEVELOPMENT.md](DEVELOPMENT.md)), not by an automated suite — the
`frust-testing` `webgl` gate selects the GL backend directly via its own
feature/build configuration rather than through a WebGPU-less browser.
(2026-09-08)

**Accepted because**: the fallback is `wgpu`'s own upstream selection
contract, not frust code to maintain, and the WebGL2 destination it falls
through to is now a full-parity render rather than a degraded one.

**Trigger for removal**: an automated cross-browser CI matrix (e.g. a hosted
Firefox/WebDriver runner) exercising the unforced default path on a
WebGPU-less engine, rather than relying on the manual gate alone.

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
also `video-web-windows-linux-unavailable-v1` below. The playground's overlay
chrome is square-cornered to work around an engine-side defect: a rounded fill
composites incorrectly at a punched-hole edge.

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
backend.

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
slot there hosts nothing.

**Why accepted**: Mode A (composite an opaque native sibling above the
surface) is the whole desktop v1 contract — no translucent window, no
punched hole, no input forwarding. Mode B on the desktop would need a
translucent swapchain plus per-host z-order and hit-test plumbing, which is
a future plan rather than a missing line: the shield-rect channel the differ
already carries is passed empty here, and that is the single place input
forwarding would attach. Until then the rule for callers is the one the
plugin READMEs state — put controls **beside** the picture, never on top of
it (`docs/CODE_STANDARDS.md`'s Platform-View Conventions).

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
on a presented frame id through `FramePairing`.

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
0.16.1 and 0.17.1. `parley`/`fontique` (and `frust-engine`'s vendored `vello_common`/`glifo`
stack) already sit on 0.17.1; the 0.16.1 copy is held down by `accesskit_consumer 0.38.0`,
`gpu-allocator 0.28.0`, and `tree_arena 0.2.0`.

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

**Evidence**: `Cargo.lock` (two `hashbrown` entries, 0.16.1 and 0.17.1, and their reverse
dependencies); the lean Android graph in `benchmarks/frust_bench`.

---

### `android-legacy-gradle-cache-recreated` — `android/.gradle` can still be recreated by a Gradle run Frust did not launch

**Observed**: `frust build apk`/`frust run` always pass `--project-cache-dir <project>/build/android/.gradle` to `./gradlew`, so their own invocations never write Gradle's per-project cache under `android/`. A hand-run `./gradlew` from the `android/` directory — Android Studio's own build, or a developer invoking Gradle directly — gets Gradle's own default cache location instead and recreates `android/.gradle`.

**Applies to**: any project opened in Android Studio, or any manual `./gradlew` invocation from `android/`.

**Why accepted**: Frust cannot intercept a build it did not launch; `--project-cache-dir` only affects an invocation Frust itself constructs. `android/.gradle` is one of `build_dirs::LEGACY_CLEAN_DIRS`, so `frust clean` removes it wherever it reappears — the fix is cleanup, not prevention.

**Evidence**: `crates/frust-drive/src/build_dirs.rs` (`LEGACY_CLEAN_DIRS`); `crates/frust-drive/src/android_run/gradle.rs` and `android_build/mod.rs` (`--project-cache-dir`).

---

### `android-build-legacy-fallback-one-release` — a pre-migration Android layout keeps building via a read-only fallback, for one release

**Observed**: `android_build::artifacts::discover` resolves a build's Gradle output new-first (`build/android/app/outputs`), then falls back read-only to the pre-migration AGP default (`android/app/build/outputs`) when only that path exists, printing a one-time warning naming the migration recipe. The fallback is a compatibility bridge, not a second permanent layout.

**Applies to**: any project scaffolded before the `build/` root migration whose `settings.gradle.kts` still lacks the `buildDirectory` redirect.

**Why accepted**: it lets a pre-migration app keep building and running without forcing an immediate migration, at the cost of one extra directory probe per artifact lookup — accepted for one release cycle only, per the module's own doc comment, after which an unmigrated project is expected to have followed the recipe.

**Trigger for removal**: the release after the one this shipped in; delete `legacy_output_dir` and the fallback branch in `resolve_output_dir` together with this entry.

**Evidence**: `crates/frust-drive/src/android_build/artifacts.rs` (`legacy_output_dir`, `resolve_output_dir`, `MIGRATION_RECIPE_DOC`).

---

### `ios-build-phase-cargo-metadata-per-build` — the iOS build phase shells out to `cargo metadata` on every Xcode build

**Observed**: the generated Xcode build-phase script resolves the Rust target directory via `${CARGO_TARGET_DIR:-$(cargo metadata --format-version 1 --no-deps | ...)}` — when `CARGO_TARGET_DIR` is unset (the common case), every Simulator or device build shells out to `cargo metadata` before it can copy the compiled static library into `$BUILT_PRODUCTS_DIR`.

**Applies to**: every iOS build of a scaffolded app.

**Why accepted**: `cargo metadata` is what lets the phase resolve the real target directory correctly under a `.cargo/config.toml` `[build] target-dir` override or a `CARGO_TARGET_DIR` env var, instead of assuming the default; its cost (a workspace manifest walk, no compilation) is negligible next to the `cargo build` step the same script already runs first.

**Evidence**: `crates/frust-drive/templates/app/ios.tmpl/Runner.xcodeproj/project.pbxproj.tmpl`'s build-phase `shellScript`.

---

### `textinput-no-themed-family` — a baseline `TextInput` field never takes its font family from the theme

**Observed**: `TextInputWidget::effective_style` (`crates/frust-widgets/src/textinput.rs` ~1230) resolves only the field's ink COLOR from the theme (`Theme::scheme().on_surface`, further dimmed while disabled) — it never touches `style.family`. A field's family is therefore whatever it was built with: the caller's own explicit `.text_style(..)`, or, absent that, `TextStyle::default()`'s `FontFamily::SystemUi`. Neither a runtime theme swap nor `frust_testing::frame::pin_type_scale` can redirect it, unlike the design-system catalogs' own label/button/title text, which resolves FAMILY from a `Theme::type_scale` role (see [TESTING.md](TESTING.md)'s Deterministic Inputs section).

**Applies to**: every baseline `TextInput` field, and every catalog control built on it that does not itself override the family — shadcn's `input`/`textarea` and the `command` palette's search field; Glyph's `command_palette` query field; and beUI's `input`, `combobox`, `multi_select` and `prompt_input` fields, plus the `command_palette`, `feedback_widget`, `morphing_search` and `signup_form` blocks' fields, all of which are built on beUI's own `input`.

**Why accepted**: closing the gap means giving `TextInput` a themed-family seam mirroring `Text`'s `.themed_family(..)` opt-in (a new branch in `effective_style`, plus each affected catalog field wiring into it) — a `frust-widgets` API change not yet built, so an app that swaps themes or a font scale at runtime sees every field's chrome and ink follow the theme while its typed text keeps the system font.

**Trigger for removal**: `TextInput` grows a themed-family option (an opt-in analogous to `Text::themed_family`) and the fields listed above are wired through it.

**Evidence**: `crates/frust-widgets/src/textinput.rs` (`effective_style`); [TESTING.md](TESTING.md)'s pinnable-text paragraph; `crates/frust-testing/src/corpus/page.rs` module docs.

---

### `url-launcher-ios-open-failure-unobservable` — an iOS launch failure has nothing left to report back to

**Observed**: `UrlLauncher::open_external` on iOS dispatches the lookup-and-open sequence onto `dispatch_get_main_queue()` asynchronously (`DispatchQueue::exec_async`) and returns `Ok(())` immediately, before that closure has run. The closure's own call, `UIApplication::openURL_options_completionHandler`, passes `None` for the completion handler, so even a successful dispatch reports nothing back into Rust once it runs. A "no handler for this URL" or "app suspended" failure on this path is therefore unobservable to the caller — `open_external` always returns `Ok(())` on iOS regardless of what actually happens.

**Applies to**: iOS only; every call to `UrlLauncher::open_external` on that platform.

**Why accepted**: a synchronous main-thread bounce (blocking until the dispatched closure completes) would violate this crate's fire-and-forget, never-block-the-caller contract if `open_external` is ever called from a background thread — `exec_async` is the only shape that holds that contract regardless of caller thread, and there is no result channel back into an already-returned stack frame from a `dispatch_get_main_queue()` closure.

**Trigger for removal**: none anticipated without a callback-based `open_external` API (the crate currently returns before dispatch completes) — a redesign this crate's originating task did not call for.

**Evidence**: `plugins/url-launcher/src/apple.rs`'s module doc (*Main-thread dispatch*, *`unsafe`*) and its `open_on_main`/`open_external` functions.

---

### `url-launcher-desktop-no-deep-link-return` — desktop has no channel for a launched browser to hand a result back

**Observed**: `desktop::open_external` spawns `open`/`xdg-open` (macOS/Linux) or calls `ShellExecuteW` (Windows) and returns as soon as the OS accepts the launch; none of the three has any way for the resulting browser tab to hand a result — an OAuth authorization code, say — back to the launching process.

**Applies to**: macOS, Linux, and Windows alike; any app using this plugin to start a browser-mediated round trip (e.g. an RFC 8252 OAuth authorization request) on desktop.

**Why accepted**: this plugin's charter is opening a URL, nothing more — a full desktop OAuth round trip needs a return channel this crate does not provide (a local loopback listener, a manually pasted code, or similar), left to the app. Mobile's equivalent return leg goes through `frust::deep_links()` instead, which has no desktop analogue.

**Trigger for removal**: a desktop deep-link/callback channel ships elsewhere in the framework and this plugin (or its caller) wires a completion path into it.

**Evidence**: `plugins/url-launcher/src/desktop.rs`'s module doc; `plugins/url-launcher/README.md` §3 (*Desktop has no deep-link callback*).

---

### `url-launcher-desktop-no-association-unobservable` — a macOS/Linux opener that runs but finds no handler reports nothing back

**Observed**: `UrlLauncher::open_external` on macOS/Linux spawns `open`/`xdg-open` and returns as soon as the spawn itself succeeds. The only failure it can classify is spawn-time: `io::ErrorKind::NotFound` (the opener **binary** is absent from `PATH`) maps to `NoHandler`, other spawn errors to `Platform`. The opener's own verdict — `xdg-open` exits nonzero, e.g. `EXIT_FAILURE_OPERATION_IMPOSSIBLE`, when nothing is registered for the URL — arrives only as an exit status on the detached reaper thread, strictly after `open_external` has already returned `Ok(())`. A host with `xdg-open` present but no registered `http`/`https` handler therefore receives `Ok(())` while nothing opened.

**Applies to**: macOS and Linux only. Windows is unaffected (`ShellExecuteW` is synchronous and its return code encodes `SE_ERR_NOASSOC`/`SE_ERR_ASSOCINCOMPLETE`); Android is unaffected (`ActivityNotFoundException` is caught and mapped to `NoHandler`).

**Why accepted**: reading that exit status means waiting for the opener to exit, and `open_external` must not block its caller — the crate's fire-and-forget contract. There is no channel back into a call that has already returned; adding one (callback, channel, polled flag) would change the crate's synchronous shape for one platform alone. `UrlLauncherError::NoHandler`'s doc comment is narrowed to state exactly what each platform detects rather than promising a detection this backend cannot deliver. Directly parallel to `url-launcher-ios-open-failure-unobservable`.

**Related, same code path**: the reaper is a plain `std::thread::spawn`, which panics if the OS cannot create a thread. Under extreme thread exhaustion `open_external` can therefore panic where the pre-fix spawn-and-drop code could not. Judged acceptable (a desktop URL-open path is not a plausible thread-exhaustion site) and recorded rather than hidden; `std::thread::Builder::spawn` would degrade instead of panicking if this ever matters.

**Trigger for removal**: the crate grows an asynchronous completion shape (a callback or awaitable outcome) that can carry a post-return verdict to the caller — for example if the sibling `auth-session` work introduces one that this crate can share.

**Evidence**: `plugins/url-launcher/src/desktop.rs` (the `#[cfg(not(target_os = "windows"))]` arm and its module doc's *Why a reaped exit status can't sharpen `NoHandler` here* section); `plugins/url-launcher/src/lib.rs`'s `NoHandler` doc comment.

---

### `auth-session-android-callback-rides-intent-filter` — Android cannot prove the OAuth callback scheme is bound to this app

**Observed**: `FrustAuthSessionHost` observes the redirect through the same ordinary custom-scheme `<intent-filter>` (`action.VIEW` + `BROWSABLE` + `android:scheme`) every frust app registers for deep links (`examples/playground/android/app/src/main/AndroidManifest.xml`'s `frustplay` filter) — a plain custom scheme carries no Android App Links-style domain verification, so any other installed app that claims the same scheme string can intercept or forge the redirect before this host ever sees it. The slot is also exposed to injected intents from the same app (a compromised Activity can call `startActivity` with the callback scheme), and to tapped links in the browser or another app that claim the same scheme. The session generation does not narrow any of this: the host keeps only the latest launch's generation in its single `pending` slot and stamps whichever callback-scheme `Intent` it observes on the next Activity resume with that value — the generation never travels through the browser or the redirect URL, so a superseded tab's late redirect, or an injected `Intent`, is attributed to the live session by timing rather than evidence. The generation only lets the Rust side discard a result for a session that is no longer live (dropped future, already resolved).

**Applies to**: Android only; any app using this plugin whose callback scheme collides with another installed app's registered scheme, or that starts a new session while a previous tab is still open.

**Why accepted**: this is RFC 8252's own documented risk for a custom-scheme redirect (its §8.1 recommends reverse-domain-unique scheme naming, which narrows but does not eliminate the window), not a defect this plugin introduces — callers MUST use PKCE and verify the state parameter to defend against forged/injected callbacks (see `README.md` §3.4 *Security*). HTTPS App Links (domain-verified) or Android's newer Auth Tab API would close the gap, but neither is in this plugin's v1 scope.

**Trigger for removal**: a follow-on card adds an HTTPS App Links or Auth Tab callback path and `AuthSessionRequest` grows a way to select it; a narrower follow-on refuses redirects observed after a superseded launch (the host would need to remember that a tab it no longer tracks is still open).

**Evidence**: `plugins/auth-session/platform/android/src/main/kotlin/dev/frust/authsession/FrustAuthSessionHost.kt`'s class doc (*Why the outcome is read from `activity.intent`*, *The double-delivery note*) and its `pending` slot; `plugins/auth-session/src/android.rs` (*The generation round trip*); `examples/playground/android/app/src/main/AndroidManifest.xml`'s `frustplay` intent filter.

---

### `auth-session-android-ephemeral-browser-dependent` — `ephemeral` on Android is advisory to the Custom Tabs provider

**Observed**: `FrustAuthSessionHost` calls `CustomTabsIntent.Builder#setEphemeralBrowsingEnabled(true)` (present in the pinned `androidx.browser:browser 1.10.0`), but the flag is a request the provider may ignore: on the test device (Xiaomi 12, LineageOS 23.2) the provider selected was Fennec F-Droid 129.0.0 and two consecutive ephemeral sessions both saw the `frustauth=1` cookie set by an earlier session. In a later check the allow-listed provider was Chrome 153, which honoured the flag (an ephemeral session did not see the normal jar's cookie; a later non-ephemeral session did) — the behaviour is per provider, not per platform. Dropping the awaited future releases the session Busy slot immediately.

**Applies to**: Android only; apps relying on `ephemeral: true` for cookie isolation must not assume it holds on every browser.

**Why accepted**: the provider is chosen from a curated allow-list of well-known providers (default browser first if it supports Custom Tabs, then the first provider from the list that is installed and answers `CustomTabsService`); there is no portable way to demand ephemeral support short of binding to the service and checking `CustomTabsClient.isEphemeralBrowsingSupported`, which is a follow-on, not a v1 blocker.

**Trigger for removal**: the host probes ephemeral support before launching (or prefers a provider that reports it) and the gate records `no cookie (set now)` on a second ephemeral visit.

**Evidence**: `plugins/auth-session/platform/android/src/main/kotlin/dev/frust/authsession/FrustAuthSessionHost.kt` (`launch`); the two-session cookie check on the test device described under Observed.

---

### `auth-session-android-resume-means-cancelled-v1` — Activity resume while a session is pending is reported as Cancelled

**Observed**: An unrelated Activity resuming while `AuthSession::start`'s future is still live (e.g. a permission dialog, app switch, or system event) triggers an `onActivityResumed` callback whose resumed `Intent` carries no callback-scheme data; the host clears its pending session and reports kind `1`, so the future resolves `Ok(AuthSessionOutcome::Cancelled)` — indistinguishable from the user dismissing the Custom Tab. The tab itself is not closed by this: it stays on screen until the user dismisses it, and a redirect it delivers afterwards finds no pending session and is dropped (the app's own deep-link stream still observes it — `plugins/auth-session/README.md` § 5). Error kind `4` (`Platform`, "no resumed Activity") is a different, launch-time path: `start`'s posted runnable found no resumed Activity to launch from, so no tab was ever opened.

**Applies to**: Android only; any app using this plugin whose UI context allows or expects graceful recovery from interruption while an auth session is pending.

**Why accepted**: v1 scope — differentiating between user-dismissed tabs and system-level resume events would require tracking session state and callback origin more finely than the current host does. The one-session `Busy` guard ensures a stuck session never wedges a later one, so an app that retries the auth flow after an interruption will not deadlock.

**Trigger for removal**: a follow-on card tracks session lifecycle granularly, distinguishing user dismissal (Cancelled) from Activity-level interruption (a new `Interrupted` outcome or a more specific error variant).

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

### `auth-session-loopback-poll-interval-v1` — the loopback accept loop's 25 ms poll bounds cancel/timeout/drop latency, not instant

**Observed**: `LoopbackSession`'s accept loop polls its shutdown flag every 25 ms; a cancel (`LoopbackCancel::cancel`), a dropped future, or `LoopbackOptions::timeout` elapsing therefore closes the listener socket within about one poll interval — including while a connection is already being served, since each connection's reads also run in 25 ms passes (since commit `5e686576`) rather than blocking for the full request-head budget. The one exception is a response write already in progress, bounded by `CONNECTION_IO_TIMEOUT` (2 s) rather than the poll interval. While idle, the loop wakes 40 times a second whether or not a client ever connects.

**Applies to**: every `LoopbackSession`, on every target it runs on.

**Why accepted**: a self-connect wake (opening a throwaway loopback connection to interrupt `accept()` immediately) would close the gap but adds its own connect-to-self race and extra firewall/antivirus surface on Windows; polling is the portable choice, and 25 ms is well under human-perceptible latency for a cancel button or a timeout deadline.

**Trigger for removal**: none anticipated — a deliberate portability trade, not a deferral.

**Evidence**: `plugins/auth-session/src/loopback.rs`'s module doc (*Lifecycle* step 3, *Security*'s last paragraph) and its accept-loop poll interval.

---

### `auth-session-no-cancel-v1` — no way to cancel a live session from Rust

**Observed**: `AuthSession` exposes no `cancel` method — once `start` returns a pending future, it only resolves when the identity provider redirects, the user dismisses the platform tab themselves, or a platform-level failure occurs; there is no programmatic way for an app to dismiss a live session (e.g. on its own timeout or navigation-away). Dropping the awaited future (not awaiting it, or awaiting and then discarding the result) releases the session Busy slot immediately without waiting for platform resolution — but it does **not** end the platform UI: on Android the Custom Tab stays on screen until the user closes it (a redirect it delivers afterwards finds no pending session and is dropped); on iOS/macOS the presented `ASWebAuthenticationSession` sheet stays up until the user dismisses it or the next `AuthSession::start` `-cancel`s it in favour of the new session (its `CanceledLogin` completion is then discarded as a stale generation).

**Applies to**: `AuthSession::start`'s platform backends only — Android, iOS, macOS (the in-app browser-tab path). `LoopbackSession` is unaffected: it exposes `LoopbackCancel::cancel` plus a mandatory `LoopbackOptions::timeout`, so a desktop loopback session always has a programmatic way out.

**Why accepted**: v1 scope — the one-session `Busy` guard (this crate's *Exactly one live session* doc section) means a stuck session still cannot wedge a later one forever, even with no cancel path and no forced cleanup — dropping the future frees the slot for a new session. Neither this crate's originating task nor its gate script called for a programmatic cancel on the platform-backend side.

**Trigger for removal**: a follow-on task adds `AuthSession::cancel` (dismissing `ASWebAuthenticationSession` via `-cancel`, finishing the Android Custom Tab activity) and resolves the live session with `AuthSessionOutcome::Cancelled`.

**Evidence**: `plugins/auth-session/src/lib.rs`'s public API (`AuthSession::start`/`is_supported` only); `plugins/auth-session/README.md` §2.

---

### `auth-session-ios-https-callback-not-supported-v1` — iOS 17.4's HTTPS App-Link callback form is unavailable

**Observed**: the Apple backend calls the deprecated `-initWithURL:callbackURLScheme:completionHandler:` initializer exclusively (`apple.rs`'s `make_session`), never iOS 17.4's `-initWithURL:callback:completionHandler:`, which can express an HTTPS App-Link callback (`ASWebAuthenticationSessionCallback`) as well as a custom scheme — only a custom `callback_scheme` redirect is supported, never an HTTPS callback URL.

**Applies to**: iOS and macOS alike (both route through the same `apple.rs` module).

**Why accepted**: this crate's deployment floor is iOS 15, where the newer initializer does not exist at all — supporting the HTTPS-callback form would need a second, floor-gated code path for a callback shape this crate's originating task did not ask for. A downstream app asked for HTTPS App-Link callback support (2026-09-23) as a wanted, non-blocking enhancement — v1 ships served instead by the private-use-scheme `AuthSession::start` path, or, on desktop, by `LoopbackSession`'s plain-`http` loopback redirect.

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

**Observed**: a project scaffolded by the yanked 0.5.0 `frust-cli`/`frust-tui` depends on `frust` by a path into a framework checkout, and its tracked `android/gradle.properties` (`frust.embedding.dir`) and `ios/Runner.xcodeproj/project.pbxproj` (the `FrustEmbedding` package `relativePath`) hold machine paths into that checkout. No command rewrites them.

**Applies to**: projects created by 0.5.0 only; 0.5.1 and later scaffold registry projects with no machine path in a tracked file.

**Why accepted**: the set of affected projects is small (0.5.0 was yanked) and the edit is mechanical; an automatic migration would rewrite user-owned manifests.

**Workaround**: re-scaffold with 0.5.1+ and move the app's sources over, or edit the three files by hand: in `Cargo.toml` replace the `frust` path dependency (and any `frust-*` plugin path dependencies) with the registry form `frust = { package = "frust-ui", version = "<release>" }` and `frust-<plugin> = "<release>"`; in `android/gradle.properties` leave `frust.embedding.dir` to be refreshed by `frust run`/`frust build -d android` (it remains a tracked machine path); in `project.pbxproj` set the `XCLocalSwiftPackageReference "FrustEmbedding"` `relativePath` to `"FrustEmbedding"` so the `ios/FrustEmbedding` symlink that `frust run`/`frust build -d ios` creates resolves it.

**Trigger for removal**: a `frust` migration command, or 0.5.0 projects falling out of use.

**Evidence**: deferred action item act_000001a1021c875bkG9kgNET; the 0.5.0 templates (`crates/frust-drive/templates/app/` before the platform-wiring change).

