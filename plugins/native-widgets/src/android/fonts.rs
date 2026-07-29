//! Theme ladder L3's Android-only half (native-widgets Phase 1, p1-08): the
//! actual JNI mechanics behind [`crate::controls::typeface`]'s
//! platform-neutral plan — get the app's private cache directory, write a
//! Glyph face's bytes there under a content-hash-keyed name (skipping the
//! write when it's already there, per
//! [`crate::controls::typeface::plan_registration`]), and hand the resulting
//! path to `Typeface.createFromFile`.
//!
//! Mirrors `crate::android::theme` (L1) vs `crate::api::theme` (L2)'s own
//! split for exactly the same reason: the pure cache-key/plan/degrade logic
//! lives in [`crate::controls::typeface`] so it's host-testable, and this
//! module is the JNI-only half that never compiles off Android at all
//! (`crate`'s module doc: `#[cfg(target_os = "android")] mod android;`).
//!
//! # The platform half never depends on `frust-theme`
//!
//! The Glyph font bytes originate in `frust-theme`'s `glyph-fonts` feature,
//! reachable only from [`crate::api::theme`] (behind this crate's
//! `frust-api` feature — the one place this crate's `Cargo.toml` allows a
//! `frust-theme` dependency at all, and only under `frust-api`). This module
//! never names that crate: `crate::api::theme` publishes the plain
//! `&'static [u8]` slices once via [`set_glyph_bytes`] — the api→runtime
//! seam, exactly like `crate::api::theme::ResolvedTheme`'s packed
//! colour/size primitives already cross it. `cargo check -p
//! frust-native-widgets --no-default-features` never sees this module reach
//! for `frust-theme`, feature on or off — see this crate's `Cargo.toml`.
//!
//! # One-time, content-hash-keyed registration
//!
//! [`typeface_for`] resolves (and caches, process-wide, in [`MONO`]/[`PLEX`])
//! a real `android.graphics.Typeface` the first time either face is asked
//! for, then returns the same cached object on every later call — including
//! from an `update` call, which carries no `Context` at all
//! (`crate::android::ctx::NativeCtx::context`'s own doc): the very first
//! `create` call for *any* control on this theme already has one, and since
//! Glyph is the framework's own default theme, a text-bearing control's
//! first build already asks for a Glyph face in the overwhelming common
//! case. A registration attempt that fails (no `Context` yet, an IO error, a
//! JNI/`Typeface` decode failure) is **not** cached — the next call retries
//! it fresh, mirroring `crate::android`'s own `HotMethods`/`CLASS_LOADER`
//! shape (only a *success* is ever cached) — so a transient failure can
//! still self-heal on a later call with a live `Context`.
//!
//! # Degrade path
//!
//! A registration failure degrades to `None` — the caller
//! (`crate::controls::platform`'s `Setter::Typeface` apply arm) treats that
//! exactly like [`Typeface::System`], i.e. `setTypeface(null)` — and logs
//! **one** warning for the whole process lifetime ([`WARNED`]), however many
//! times registration is retried, never a crash, never a panic across FFI
//! (`docs/CODE_STANDARDS.md`'s no-unwind rule).

use std::sync::{Once, OnceLock};

use jni::objects::{JObject, JString};
use jni::refs::Global;
use jni::{jni_sig, jni_str};

use crate::NativeWidgetError;
use crate::android::ctx::NativeCtx;
use crate::controls::typeface::{
    RegistrationAction, Typeface, cache_key, degrade_on_failure, plan_registration,
};

/// `android.graphics.Typeface` — the framework class
/// [`create_from_file`]'s static call resolves against.
const TYPEFACE_CLASS: &str = "android.graphics.Typeface";

/// This process's Glyph font bytes, published once by [`crate::api::theme`]
/// — see the module doc's *platform half never depends on `frust-theme`*.
struct GlyphBytes {
    /// Space Mono (Regular face) — [`Typeface::GlyphMono`].
    mono: &'static [u8],
    /// IBM Plex Mono (Regular face) — [`Typeface::GlyphPlex`].
    plex: &'static [u8],
}

static BYTES: OnceLock<GlyphBytes> = OnceLock::new();

/// Publish this process's Glyph font bytes — idempotent (the first call
/// wins; a later call, with the same or different bytes, is a silent
/// no-op — the same static-first shape as this crate's other process-wide
/// caches, e.g. `crate::android::theme`'s `DENSITY`). Never called at all
/// when the `frust-api` feature is off, or on a process that never resolves
/// a `Theme`, in which case [`typeface_for`] simply never finds published
/// bytes and every control quietly stays on [`Typeface::System`] — no
/// warning, since nothing was ever asked to register (see [`resolve`]).
pub(crate) fn set_glyph_bytes(mono: &'static [u8], plex: &'static [u8]) {
    let _ = BYTES.set(GlyphBytes { mono, plex });
}

/// The two faces' cached, real `Typeface` objects — populated only on a
/// *successful* [`register`] (module doc: a failure is never cached, so a
/// later call retries fresh).
static MONO: OnceLock<Global<JObject<'static>>> = OnceLock::new();
static PLEX: OnceLock<Global<JObject<'static>>> = OnceLock::new();

/// The one warning this whole module ever logs (module doc's degrade path) —
/// a registration failure is worth one loud signal, not one per control per
/// frame, however many times it's retried.
static WARNED: Once = Once::new();

/// Resolve `typeface` to a real, process-cached `android.graphics.Typeface`
/// object — `None` for [`Typeface::System`] (the caller's own
/// `setTypeface(null)`) or on any registration failure (module doc's degrade
/// path).
pub(crate) fn typeface_for(
    ctx: &mut NativeCtx<'_, '_>,
    typeface: Typeface,
) -> Option<&'static JObject<'static>> {
    match typeface {
        Typeface::System => None,
        Typeface::GlyphMono => resolve(ctx, &MONO, "glyphMono", |bytes| bytes.mono),
        Typeface::GlyphPlex => resolve(ctx, &PLEX, "glyphPlex", |bytes| bytes.plex),
    }
}

/// Resolve-and-cache one face — see [`typeface_for`].
fn resolve(
    ctx: &mut NativeCtx<'_, '_>,
    cache: &'static OnceLock<Global<JObject<'static>>>,
    face_id: &str,
    pick: impl FnOnce(&GlyphBytes) -> &'static [u8],
) -> Option<&'static JObject<'static>> {
    if let Some(cached) = cache.get() {
        return Some(cached.as_obj());
    }
    // No bytes ever published: this feature/theme combination was never
    // asked for a Glyph face (module doc's `set_glyph_bytes` note) — quietly
    // degrade, no warning.
    let bytes = pick(BYTES.get()?);
    let global = degrade_on_failure(register(ctx, face_id, bytes), |e| warn_once(face_id, e))?;
    Some(cache.get_or_init(|| global).as_obj())
}

/// See [`WARNED`].
fn warn_once(face_id: &str, error: &NativeWidgetError) {
    WARNED.call_once(|| {
        log::warn!(
            "frust-native-widgets: could not register the Glyph {face_id} typeface ({error}) — \
             degrading to the platform's system typeface (see docs/ARCHITECTURE.md's \
             `frust-native-widgets` row for the theme ladder)"
        );
    });
}

/// One face's registration: content-hash-keyed cache-file write (skipped
/// when the file already exists — [`plan_registration`]), then
/// `Typeface.createFromFile`.
///
/// # Errors
/// [`NativeWidgetError::Platform`] when this call has no live `Context`
/// (only `createView` carries one — [`cache_dir_path`]), the cache file
/// cannot be written, or the JNI call throws or returns `null`.
fn register(
    ctx: &mut NativeCtx<'_, '_>,
    face_id: &str,
    bytes: &'static [u8],
) -> Result<Global<JObject<'static>>, NativeWidgetError> {
    let path = cache_file_path(ctx, face_id, bytes)?;
    match plan_registration(std::path::Path::new(&path).exists()) {
        RegistrationAction::ReuseExisting => {}
        RegistrationAction::WriteThenCreate => write_atomic(&path, bytes)?,
    }
    create_from_file(ctx, &path)
}

/// `<cacheDir>/<contentHashKeyedName>` — see
/// [`crate::controls::typeface::cache_key`].
fn cache_file_path(
    ctx: &mut NativeCtx<'_, '_>,
    face_id: &str,
    bytes: &[u8],
) -> Result<String, NativeWidgetError> {
    let dir = cache_dir_path(ctx)?;
    Ok(format!("{dir}/{}", cache_key(face_id, bytes)))
}

/// `context.getCacheDir().getAbsolutePath()` — the one call in this module
/// that needs a live `Context`, hence [`register`]'s own doc on when this can
/// fail.
fn cache_dir_path(ctx: &mut NativeCtx<'_, '_>) -> Result<String, NativeWidgetError> {
    let context = ctx.context()?;
    let dir = ctx.run_jni("Context.getCacheDir", |env| {
        env.call_method(
            context,
            jni_str!("getCacheDir"),
            jni_sig!("()Ljava/io/File;"),
            &[],
        )?
        .l()
    })?;
    ctx.run_jni("File.getAbsolutePath", |env| {
        let path = env
            .call_method(
                &dir,
                jni_str!("getAbsolutePath"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?
            .l()?;
        let path = env.cast_local::<JString>(path)?;
        path.try_to_string(env)
    })
}

/// Write `bytes` to `path` via a same-directory temp file + rename, so a
/// crash mid-write can never leave a half-written file at the content-hash
/// key a later launch would otherwise trust as complete.
fn write_atomic(path: &str, bytes: &[u8]) -> Result<(), NativeWidgetError> {
    let tmp = format!("{path}.tmp-{}", std::process::id());
    std::fs::write(&tmp, bytes)
        .map_err(|e| NativeWidgetError::Platform(format!("writing {tmp}: {e}")))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| NativeWidgetError::Platform(format!("renaming {tmp} to {path}: {e}")))
}

/// `Typeface.createFromFile(String)`, retained as a process-lifetime global
/// reference (module doc: [`MONO`]/[`PLEX`] never release it).
fn create_from_file(
    ctx: &mut NativeCtx<'_, '_>,
    path: &str,
) -> Result<Global<JObject<'static>>, NativeWidgetError> {
    let class = ctx.class(TYPEFACE_CLASS)?;
    let jpath = ctx.run_jni("NewStringUTF(typeface path)", |env| env.new_string(path))?;
    let typeface = ctx.run_jni("Typeface.createFromFile", |env| {
        env.call_static_method(
            &class,
            jni_str!("createFromFile"),
            jni_sig!("(Ljava/lang/String;)Landroid/graphics/Typeface;"),
            &[jni::objects::JValue::Object(&jpath)],
        )?
        .l()
    })?;
    if typeface.is_null() {
        return Err(NativeWidgetError::Platform(format!(
            "Typeface.createFromFile({path}) returned null"
        )));
    }
    ctx.retain(&typeface)
}
