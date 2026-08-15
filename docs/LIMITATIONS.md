# Frust - Known Limitations

A register of accepted, known limitations an app author can actually hit —
not a bug list and not a TODO list. Open bugs and unshipped fixes are tracked
separately; this doc is for a degrade that is **measured, understood, and
deliberately shipped anyway** (accepted by Ed, or blocked on a documented
constraint), so it is discoverable from the docs instead of only from a gate
report or an old chat.

**Entry bar**: evidence, not suspicion. Every entry below traces to a device
gate, a review round, or a task's on-device measurement — cited at the end of
each entry. Each entry has a stable id (`` `id-like-this` ``) other docs and
code comments can cite. When a limitation is fixed, delete its entry rather
than marking it resolved-in-place — this file describes current-state gaps
only.

---

### `cam-blit-opaque` — forced-blit platform view is invisible (Android)

**Observed**: a Mode B platform view (the plugin's native sibling, e.g. a
camera preview) rendered above/behind an Android surface that takes the
**blit** render path is completely invisible — the surface resolves opaque
instead of translucent, so the native view sits behind an opaque frust
surface. Measured: the preview slot's pixels are pure page background (std
5.4, exactly `bg-surface`); every other chrome renders normally.

**The app is now told** (`translucencyRefused` shipped):
`frust::resolved_surface_mode()` answers
`ResolvedSurfaceMode::RefusedTranslucent` on exactly this surface — a poll,
read during rebuild like any other process-global shell state, never a
reactive wake. The *degrade itself is unchanged*: frust still paints the
opaque Mode A contract, and the host's native-sibling z-order is still fixed
at build time, so the sibling stays behind the opaque surface (invisible and
untappable) until the host is rebuilt in Mode A. What the signal buys is a
deliberate app-side fallback (render your own content in the slot) instead of
a dead rect.

**Applies to**: Android, any surface that takes the blit path — a GPU-tier
surface missing `Rgba8Unorm`+`STORAGE_BINDING` (no flag set by anyone; reach
across real hardware is unmeasured), or any build compiled/run with
`FRUST_NO_DIRECT_SURFACE` (a deliberate debug/safety valve). **Exempt**: the
experimental `cpu-tier` (`vello_cpu`) fallback — its output is already
premultiplied, so it stays translucent-capable and is never affected.

**Why accepted, and why the refusal itself exists**: a blit target lacks
`STORAGE_BINDING`, so the compute pass that premultiplies vello's straight-
alpha output (the fix for the Direct path's over-bright fringing defect)
cannot run there. Refusing translucency was chosen over shipping a silently
fringing surface — the gap was the missing app-facing signal, not the refusal
itself; do not remove the refusal to "fix" this. Accepted by Ed 2026-07-27 as
a documented limitation rather than a merge blocker, binding before Mode B GA
on Android. `translucencyRefused` has since shipped (above), so what remains
here is the *invisible sibling* itself, not the silence.

**Evidence**: on-device verification of the blit-path surface (Android); the
app-facing signal shipped as `translucencyRefused`.

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

**Why not fixed**: deferred by Ed's explicit ruling (review round 1, finding
3), not by oversight. Two fixes are on the table but the choice between them
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

### `focus-double-erasure-swap-blind` — a type swap through a doubly-erased pod is invisible to every reconciler

**Observed**: `any(any(view))` — an `AnyView` erased a second time — produces a
`ChildPod` whose stored element has the concrete type `Box<dyn Widget>`
*whatever the inner view is*. Every swap-detection site in `frust-widgets`
decides "did this rebuild replace the widget?" by comparing that erased
element's `TypeId` across the rebuild
(`authoring::rebuild_child_tracked`, the one funnel `rebuild_child` and
`rebuild_children` both use), so a genuine **inner** concrete-type change
reports `swapped == false`: the old widget really is torn down and replaced
inside `AnyView::rebuild`, but the pod's recorded `active`/`focused` flags are
neither cleared nor reported. When the live focus session belonged to the
replaced widget, the pod keeps routing `Key`/`Ime` events into a fresh widget
that never claimed focus, no `mark_focus_orphaned` is raised, and
`RenderRoot`'s `focus_active`/`ime_state` stay standing over a widget that no
longer exists — exactly the failure the single-erasure swap arms exist to
prevent (`docs/CODE_STANDARDS.md`'s orphan contract).

**Applies to**: any `ChildPod` built from a doubly-erased view, on every
platform. It is reachable by accident rather than by intent: a builder that
erases its own child (`pattern_switcher(key, pattern, child)` calls
`any(child)` internally) double-erases whenever the caller already handed it an
`AnyView`. **Single** erasure — the overwhelmingly common `any(concrete_view)`,
and every in-crate container's own child list — detects swaps correctly and is
unaffected.

**Why not fixed**: pre-existing (it predates the focus/IME fix rounds that
found it) and not fixable at a call site — the information the reconciler needs
has already been erased by the time it looks. Closing it needs **shared
swap-detection machinery**: `ErasedView` would have to report the *element's*
concrete `TypeId` through the erasure so nesting composes, instead of each
reconciler probing whatever boxed element it happens to hold. That is a
`frust-core` trait-surface change landing on every reconciler at once, and was
deliberately not attempted inside a focus/IME review fix.

**Evidence**: source inspection of `crates/frust-core/src/view.rs`
(`AnyView`'s `View`/`ErasedView` impls — the outer `dyn_build` boxes the inner
`Box<dyn Widget>`) against `crates/frust-widgets/src/authoring.rs`'s
`rebuild_child_tracked`; found during review-fix-3 (FC)'s audit of the
focus-severing sites.

---

### `focus-navbar-item-truncation-unmarked` — a truncated navbar/tabbar item drops its focus link silently

**Observed**: `material::navbar`'s `NavigationBarView::rebuild` and
`cupertino::tabbar`'s equivalent hand-roll their item pod lists rather than
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
deferred rather than fixed speculatively during a focus/IME review round.

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
caveat; flagged by phase-review round 1 (`workflow/reviews/db-plugin/REVIEW.md`).

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

**Evidence**: `workflow/plans/features/frust-tui-devex/PLAN.md`'s Non-Goals and Risks
sections (iOS physical-device forwarding via usbmuxd).

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

**Evidence**: `crates/frust-devtools/src/token.rs` (`os_random_bytes` cfg gate + module doc);
review finding recorded in `workflow/reviews/frust-tui-devex-phase2/REVIEW-round1.md`.

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
`workflow/plans/features/frust-tui-devex/phase3/TASKS.md`'s p3-06 completion note
("desktop/iOS sampling unavailable — pid not exposed").

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
gap is tracked as a `docs/LIMITATIONS.md` entry"); `workflow/plans/features/frust-tui-devex/phase3/TASKS.md`'s
conductor decision ("No sysinfo dep... macOS metrics deferred to LIMITATIONS").

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
hours unattended.

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
`~/.config/frust/tui.toml`). A successful bind additionally rewrites the detected IDE's DAP launch
config while `auto_configure_ide` is on.

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
dialog's `g` (Generate) action and the auto-configure-on-listen flow both report the skip rather
than a written file.

**Why accepted**: fdemon-pro (the ported source this module follows) worked around the identical gap
by spawning a *second*, separate adapter-binary process — a workaround `frust-dap` cannot reuse,
since it has no per-session spawnable binary of its own. Generating a config that could never
actually connect was rejected in favor of honestly reporting nothing was written.

**Evidence**: `crates/frust-dap/src/ide_config/helix.rs`'s module doc and `SKIP_REASON`;
`crates/frust-dap/src/ide_config/mod.rs`'s `generate_ide_config` Helix arm.

---

### `dap-zed-adapter-unverified` — the generated Zed DAP config names an unverified adapter

**Observed**: `crates/frust-dap/src/ide_config/zed.rs`'s `ZedGenerator` names the debug adapter as
`"CodeLLDB"` in the generated `.zed/debug.json` entry — a best-effort choice (mirroring
fdemon-pro's own analogous workaround of naming Go's `"Delve"` adapter for a non-Go TCP peer), never
verified against a real Zed release. Whether Zed's debug panel accepts a `CodeLLDB` entry pointed at
a non-lldb TCP peer, or validates the adapter/language pairing in a way that would reject it, is
unconfirmed.

**Applies to**: any workbench where the detected or overridden IDE is Zed and a DAP config is
generated for it.

**Why accepted**: Zed ships no native Frust (or Dart/Flutter-family) adapter to name honestly;
`CodeLLDB` is the closest generic match for a Rust project's debug panel. Verifying needs a real Zed
instance, unavailable this round — do not surface this adapter name in user-facing docs until it is.

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
comments or formatting, the moment its DAP server (re)binds with `[dap].auto_configure_ide` on (the
default — see `dap-tcp-unauthenticated-v1` above) and detects VS Code/VS Code Insiders/Cursor as the
parent IDE. This fires automatically, not on an explicit user action: the first bind after that file
exists silently reprints it.

**Why accepted**: a byte-preserving surgical splice (find the frust entry's byte span inside the
original text and edit only that span, leaving everything else untouched) is the real fix, but was
judged disproportionate for this round — hand-rolling JSONC span-splicing is real parser-writing risk
for a config-generation feature, and no byte-preserving JSON/JSONC crate is pinned in this workspace
(pins are LAW, `docs/DEVELOPMENT.md`'s Version-Pin Policy). `toml_edit` is the precedent for exactly
this shape on the TOML side (`docs/TUI_DEVELOPMENT.md`'s pin row), but it has no JSONC-editing
equivalent pinned here. The chosen remedy for this round is honest disclosure — this entry, plus the
doc-comment corrections on `merge_config`/`run_generator`/`post_write` — rather than a bigger,
unreviewed parser change.

**Evidence**: `crates/frust-dap/src/ide_config/vscode.rs`'s `VSCodeGenerator::merge_config` (clean →
parse → reprint) and its module doc; `crates/frust-dap/src/ide_config/merge.rs`'s `clean_jsonc`
(comment/trailing-comma stripping) and `to_pretty_json` (`serde_json::to_string_pretty` reprint);
`crates/frust-dap/src/ide_config/mod.rs`'s `run_generator` (byte-equality skip check against the
reprinted output only).

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
