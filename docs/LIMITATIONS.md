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
(e.g. the camera preview inside the catalog's `ScrollView`); measured on
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
stays load-bearing regardless: the catalog's own camera page deliberately
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

### `ime-ios-content-type-unverified` — iOS secure-entry IME path is compile- and device-unverified

**Observed**: `FrustViewController.swift`'s IME reconcile path — the
content-type application (FINDINGS #31: suppressing the QuickType
suggestion bar and keyboard learning on a `"password"`-classified field),
the **mirror re-seed/reconcile** that keeps `FrustView.mirror` from carrying
a previous field's text across a focus move (`syncImeFocus`'s `seedMirror`
on the content-type branch and `FrustView.reconcileMirror(to:)` on the
steady-active branch), and the **per-frame `syncImeFocus()` call from
`renderFrame`** that makes non-touch focus moves (Return-to-next-field,
programmatic focus) reach any of it — has never been compiled (no macOS
host in this project's CI/agent loop) or run on an iOS device or simulator.
`cargo` cannot compile Swift; only `xcodebuild -scheme FrustEmbedding
-destination 'generic/platform=iOS' build` validates it, and that has not
been run since this path was introduced or since any of these fixes landed.

**Also covers (fixes F3/F3b, review-fix-3)**: `syncImeFocus`'s per-frame
`becomeFirstResponder()` retry is now **bounded** (`imeFocusSatisfied`)
instead of fighting an intentional UIKit-originated resign (user swipe-
dismiss, a sibling native control taking first responder) forever, because
nothing on the Swift side can observe *why* first responder was resigned.
A user touch on the Frust surface (`onTouch`, user-initiated) unconditionally
re-arms the bound — this is what lets a user bring back a keyboard they
dismissed themselves by re-tapping the field, since re-tapping an
already-active field changes neither Rust's `active` nor `contentType`
signal. A touch inside a Mode B hosted slot never reaches `onTouch`
(`FrustView.hitTest` returns `nil` there), so a sibling native control
retaining first responder is unaffected by the per-frame tick. This
mechanism is Swift-only and is exactly as compile- and device-unverified as
the rest of this entry.

**Residual limitation, accepted (not a bug to fix here)**: the same
unconditional re-arm-on-any-touch means a touch **elsewhere** on the Frust
surface (e.g. scrolling non-editable content, outside a Mode B sibling's
interactive slot) while that sibling native control holds first responder
also re-arms the bound. Rust has no way to learn the sibling took first
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
gate — while the iOS Swift side has now gone four batches running without
compiling at all. Compile-verified is not device-verified, so F5 narrows
FINDINGS #31's Android gap without closing the finding.

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
   reasoned, not compiled or measured** — this run is what would catch a
   regression.

**Evidence**: none yet — flagged during these fixes' implementation and
review; no device or simulator run has occurred.
