# Frust - Known Limitations

A register of accepted, known limitations an app author can actually hit —
not a bug list and not a TODO list. Open bugs and unshipped fixes live in
`workflow/plans/`; this doc is for a degrade that is **measured, understood,
and deliberately shipped anyway** (accepted by Ed, or blocked on a documented
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
5.4, exactly `bg-surface`); every other chrome renders normally. The app is
never told — `translucencyRefused` is filed but not implemented, so this is
silent from the app's point of view.

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
fringing surface — the gap is the missing app-facing signal, not the refusal
itself; do not remove the refusal to "fix" this. Accepted by Ed 2026-07-27 as
a documented limitation rather than a merge blocker, binding before Mode B GA
on Android — `translucencyRefused` is tracked work, not abandoned.

**Evidence**: `workflow/plans/features/frust-camera/research/VERIFY-CAMERA.md`
§R2 and its "the reach, corrected" subsection; `followups/review-fix-2/03`.

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

**Evidence**: `research/SPIKE-SYNC.md` §2.8 (computed diagnosis) and §2.9
(measured, does not reproduce the computed gain);
`followups/gate-fix-1` is unrelated — this is a task-12b/main-loop result,
recorded in `VERIFY-CAMERA.md`'s Known issues #2.

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

**Evidence**: `research/VERIFY-CAMERA.md` §B, §G, ruling R1 (closed,
accepted).

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

**Evidence**: `workflow/reviews/frust-camera/REVIEW.md` Round 1 "Deferred to
Ed"; `research/VERIFY-CAMERA.md` Known issues #1.

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

**Evidence**: `plugins/camera` task 09 completion summary; crate rustdoc/
README caveats.

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

**Evidence**: `workflow/reviews/frust-camera/REVIEW.md` round 0 minors list;
`plugins/camera` task 03 completion summary, Risks/Limitations #2.
