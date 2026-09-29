//! [`NativeCtx`] — the scoped JNI call context every Android
//! [`NativeWidget`](crate::runtime::NativeWidget) builds its views through.
//!
//! A context is **borrowed, never stored**: it wraps the live `Env` of the
//! JNI export currently on the stack (`crate::android`), plus the `Activity`/
//! `Context` the factory handed that export. It exists for exactly one
//! create/update/dispose call and cannot outlive it — which is also what
//! keeps every local reference a control creates tied to the JNI stack frame
//! the JVM pops on return.
//!
//! # What the helpers buy over a bare `Env`
//!
//! 1. **Typed errors.** [`NativeCtx::run_jni`] is the camera backend's
//!    `run_jni` shape: it checks/clears any pending Java exception and turns
//!    it into a [`NativeWidgetError::Platform`] naming the failing operation.
//!    Leaving an exception pending is undefined behaviour for the next JNI
//!    call, so no path here may skip it.
//! 2. **Process-lifetime caches.** [`NativeCtx::class`] resolves a class
//!    through the **application classloader** once and keeps one global
//!    reference for the process (camera's `HOST_CLASS` shape); callers get a
//!    fresh local ref each time, which dies with their frame. A control's own
//!    hot method ids belong in that control's `OnceLock` table (a proven
//!    pattern — `JMethodID` is `Send + Sync` precisely so it can be
//!    cached), seeded from [`NativeCtx::env`].
//! 3. **Local-frame discipline.** [`NativeCtx::with_frame`] is the wrapper a
//!    hierarchy loop must run inside: fifty children built without it would
//!    pin fifty-plus local references for the whole call.
//!
//! # Why the classloader, not `FindClass`
//!
//! `FindClass` resolves against the *bootstrap* loader on a JNI worker
//! thread, which cannot see app classes at all (`FrustNativeListener` is an
//! app class). The application `Context`'s classloader is the one that can —
//! the mechanism `frust-secure-storage`/`frust-camera` already use. The
//! **loader itself** is cached process-wide; the `Context` is not, because it
//! is the hosting `Activity` (the embedding's `FrustPlatformViewFactory`
//! passes the Activity as the `Context`) and a process-lifetime global
//! reference to an Activity is a textbook leak across recreation.
//!
//! # Which calls have a `Context`
//!
//! Only `createView` receives one, so [`NativeCtx::for_create`] carries the
//! `Activity`/`Context` pair and [`NativeCtx::detached`] (the
//! update/dispose/event paths) carries neither: [`NativeCtx::context`] then
//! reports [`NativeWidgetError::Platform`] rather than inventing a context.
//! Class lookups still work on those paths — the cached loader outlives the
//! call that seeded it.

// This module is the *whole* helper surface the built-in controls, their
// listeners, and the public `NativeComponent` surface build on.
// Most of it now has a caller on Android; one item (`set_enabled`) stays
// genuinely unused until a control needs the checked path rather than the
// cached one — the attribute stays for that rather than growing per-item
// `allow`s.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock};

use jni::objects::{JClass, JObject, JValue};
use jni::refs::Global;
use jni::signature::MethodSignature;
use jni::strings::JNIStr;
use jni::{Env, jni_sig, jni_str};

use crate::NativeWidgetError;
use crate::controls::date_picker::CivilDate;
use crate::registry::SlotId;

/// The generic listener class every interactive control attaches
/// (`plugins/native-widgets/platform/android/src/main/kotlin/dev/frust/
/// nativewidgets/FrustNativeListener.kt`), in the **binary/dotted** form
/// `ClassLoader.loadClass` expects — never the slash form `FindClass` wants.
///
/// This string and the Kotlin file's `package` declaration are one contract:
/// the class ships inside the plugin's own Gradle module under
/// `dev.frust.nativewidgets` (`crate::android`'s *Package* note), and a
/// mismatch here surfaces only at runtime, as a failed class lookup the first
/// time an interactive control is created.
pub(crate) const LISTENER_CLASS: &str = "dev.frust.nativewidgets.FrustNativeListener";

/// The application classloader, resolved once from the first `Context` this
/// plugin is handed (module doc's *Why the classloader*).
static CLASS_LOADER: OnceLock<Global<JObject<'static>>> = OnceLock::new();

/// Classes resolved through [`CLASS_LOADER`], keyed by binary name.
///
/// One global reference per class for the process lifetime — deliberate, and
/// excluded from the per-control leak bar (`crate::registry`), exactly like
/// camera's `HOST_CLASS`: the count is bounded by the number of distinct
/// control classes this backend constructs (a handful), not by live controls.
static CLASSES: LazyLock<Mutex<HashMap<&'static str, Global<JClass<'static>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Lock a module-global mutex, recovering from poisoning rather than
/// panicking near an FFI boundary (the camera backend's `lock` shape) — the
/// guarded data is a plain cache, coherent even after a caught panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The scoped JNI context handed to every [`NativeWidget`](crate::runtime::NativeWidget)
/// method — see the module doc.
///
/// `'local` is the JNI stack frame every reference this context produces is
/// tied to; `'env` is the borrow of the export's own `Env`.
pub(crate) struct NativeCtx<'local, 'env> {
    env: &'env mut Env<'local>,
    activity: Option<&'env JObject<'local>>,
    context: Option<&'env JObject<'local>>,
}

impl<'local, 'env> NativeCtx<'local, 'env> {
    /// The `createView` context: the factory's `Activity` and `Context` are
    /// both live for the duration of the call.
    pub(crate) fn for_create(
        env: &'env mut Env<'local>,
        activity: &'env JObject<'local>,
        context: &'env JObject<'local>,
    ) -> Self {
        Self {
            env,
            activity: Some(activity),
            context: Some(context),
        }
    }

    /// The `updateParams`/`disposeView`/listener context: a live `Env`, no
    /// `Activity`/`Context` (module doc's *Which calls have a `Context`*).
    pub(crate) fn detached(env: &'env mut Env<'local>) -> Self {
        Self {
            env,
            activity: None,
            context: None,
        }
    }

    /// The live `Env` — the escape hatch for anything the helpers below do
    /// not cover (a control's own cached `JMethodID`s and
    /// `call_method_unchecked` hot path).
    pub(crate) fn env(&mut self) -> &mut Env<'local> {
        self.env
    }

    /// The hosting `Context` (the factory passes the `Activity` as one).
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] on a call path that has none — a
    /// control needing a `Context` outside `create` must retain what it needs
    /// in its own state instead.
    pub(crate) fn context(&self) -> Result<&'env JObject<'local>, NativeWidgetError> {
        self.context.ok_or_else(|| {
            NativeWidgetError::Platform(
                "no Android Context on this call path (only createView carries one)".into(),
            )
        })
    }

    /// The hosting `Activity`; see [`Self::context`] for the availability
    /// rule.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] on a call path that has none.
    pub(crate) fn activity(&self) -> Result<&'env JObject<'local>, NativeWidgetError> {
        self.activity.ok_or_else(|| {
            NativeWidgetError::Platform(
                "no Android Activity on this call path (only createView carries one)".into(),
            )
        })
    }

    /// Run a sequence of JNI calls, converting a pending Java exception into a
    /// [`NativeWidgetError::Platform`] naming `op` (module doc's *typed
    /// errors*).
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] on a thrown exception or any other JNI
    /// failure.
    pub(crate) fn run_jni<T>(
        &mut self,
        op: &str,
        f: impl FnOnce(&mut Env<'local>) -> Result<T, jni::errors::Error>,
    ) -> Result<T, NativeWidgetError> {
        run_jni(self.env, op, f)
    }

    /// Resolve `binary_name` (dotted form) through the application
    /// classloader, cached process-wide; the returned local reference dies
    /// with the caller's JNI frame.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the class cannot be loaded — for
    /// an app class that means its Kotlin file is not in the app module (see
    /// `plugins/native-widgets/platform/android/`).
    pub(crate) fn class(
        &mut self,
        binary_name: &'static str,
    ) -> Result<JClass<'local>, NativeWidgetError> {
        {
            let cached = lock(&CLASSES);
            if let Some(global) = cached.get(binary_name) {
                return run_jni(self.env, "NewLocalRef(class)", |env| {
                    env.new_local_ref(&**global)
                });
            }
        }

        let loader = self.class_loader()?;
        let class = run_jni(
            self.env,
            &format!("ClassLoader.loadClass(\"{binary_name}\")"),
            |env| {
                let name = env.new_string(binary_name)?;
                let class = env
                    .call_method(
                        loader,
                        jni_str!("loadClass"),
                        jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
                        &[JValue::Object(&name)],
                    )?
                    .l()?;
                env.cast_local::<JClass>(class)
            },
        )?;
        let global = run_jni(self.env, "NewGlobalRef(class)", |env| {
            env.new_global_ref(&class)
        })?;
        lock(&CLASSES).entry(binary_name).or_insert(global);
        Ok(class)
    }

    /// `new <binary_name>(context)` — the one-argument `Context` constructor
    /// every `android.widget` view has.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the class cannot be loaded, the
    /// call path carries no `Context`, or the constructor throws.
    pub(crate) fn new_view(
        &mut self,
        binary_name: &'static str,
    ) -> Result<JObject<'local>, NativeWidgetError> {
        let class = self.class(binary_name)?;
        let context = self.context()?;
        run_jni(self.env, &format!("new {binary_name}(Context)"), |env| {
            env.new_object(
                &class,
                jni_sig!("(Landroid/content/Context;)V"),
                &[JValue::Object(context)],
            )
        })
    }

    /// Promote a local reference to a global one the control retains past
    /// this call — every such reference must end up owned by the
    /// [`NativeView`](crate::android::NativeView) `create` returns, so
    /// disposal pair-deletes it.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the JVM cannot allocate the
    /// reference (ART aborts the process at 51,200 live global refs, so this
    /// failing at all is a leak signal).
    pub(crate) fn retain(
        &mut self,
        object: &JObject<'_>,
    ) -> Result<Global<JObject<'static>>, NativeWidgetError> {
        run_jni(self.env, "NewGlobalRef", |env| env.new_global_ref(object))
    }

    /// Call a `void` method on `object` — the shape almost every property
    /// setter takes.
    ///
    /// The signature is compile-time checked (`jni_sig!`); a control setting
    /// the *same* property every frame should cache the `JMethodID` and use
    /// `Env::call_method_unchecked` from [`Self::env`] instead, which measured
    /// at ~0.14–0.27 µs per crossing.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the method is missing or throws.
    pub(crate) fn call_void<'sig, 'sig_args>(
        &mut self,
        object: &JObject<'_>,
        name: &JNIStr,
        signature: impl AsRef<MethodSignature<'sig, 'sig_args>>,
        args: &[JValue<'_>],
    ) -> Result<(), NativeWidgetError> {
        run_jni(self.env, &format!("{name}"), |env| {
            env.call_method(object, name, signature, args)?.v()
        })
    }

    /// `parent.addView(child)` — hierarchy building, the piece no platform
    /// factory offered before this runtime.
    ///
    /// Load-bearing: it is what
    /// `crate::component::ComponentCtx::add_child` calls, and what an earlier
    /// device experiment built 50 native children in ONE slot with (inside
    /// [`Self::with_frame`], global refs back to zero after disposal).
    /// **The platform lays the resulting subtree out**,
    /// not frust: a `LinearLayout` parent measures and positions its own
    /// children, and frust still sees one opaque slot with one rect.
    ///
    /// The Java parent holds its own strong reference to `child`, so a child
    /// that is merely *attached* needs no global ref at all; a component that
    /// wants to keep talking to one retains it via
    /// `crate::component::ComponentCtx::retain_child`, whose handle is
    /// released with the component's state.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the call throws (e.g. `child`
    /// already has a parent).
    pub(crate) fn add_child(
        &mut self,
        parent: &JObject<'_>,
        child: &JObject<'_>,
    ) -> Result<(), NativeWidgetError> {
        self.call_void(
            parent,
            jni_str!("addView"),
            jni_sig!("(Landroid/view/View;)V"),
            &[JValue::Object(child)],
        )
    }

    /// `view.setEnabled(enabled)` — shared by every control, so it lives here
    /// rather than in six control modules.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the call throws.
    pub(crate) fn set_enabled(
        &mut self,
        view: &JObject<'_>,
        enabled: bool,
    ) -> Result<(), NativeWidgetError> {
        self.call_void(
            view,
            jni_str!("setEnabled"),
            jni_sig!("(Z)V"),
            &[JValue::Bool(enabled)],
        )
    }

    /// `view.setContentDescription(label)` — the accessibility label every
    /// control's props carry. Native controls are exposed to TalkBack by the
    /// platform itself, never through frust's semantics pass.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the call throws.
    pub(crate) fn set_content_description(
        &mut self,
        view: &JObject<'_>,
        label: &str,
    ) -> Result<(), NativeWidgetError> {
        let text = run_jni(self.env, "NewStringUTF(contentDescription)", |env| {
            env.new_string(label)
        })?;
        self.call_void(
            view,
            jni_str!("setContentDescription"),
            jni_sig!("(Ljava/lang/CharSequence;)V"),
            &[JValue::Object(&text)],
        )
    }

    /// `new FrustNativeListener(slot_id)` — the ONE generic listener class
    /// (`crate::android`'s contract table). Attach it with
    /// [`Self::set_on_click_listener`]/[`Self::set_on_checked_change_listener`]/
    /// [`Self::set_on_seek_bar_change_listener`]/[`Self::init_date_picker`]
    /// (whichever interface the control needs), and retain it in the control's
    /// [`NativeView`](crate::android::NativeView) `extra` list so it is
    /// pair-deleted with the view. A `NativeComponent` reaches the same
    /// constructor through [`Self::attach_listener`], never with an id of its
    /// own.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the class is missing from the app
    /// module or the constructor throws.
    pub(crate) fn new_listener(
        &mut self,
        slot_id: SlotId,
    ) -> Result<JObject<'local>, NativeWidgetError> {
        let class = self.class(LISTENER_CLASS)?;
        run_jni(self.env, "new FrustNativeListener(long)", |env| {
            env.new_object(
                &class,
                jni_sig!("(J)V"),
                &[JValue::Long(slot_id_to_jlong(slot_id))],
            )
        })
    }

    /// `view.setOnClickListener(listener)` — `Button`'s listener attach.
    /// Pass `&JObject::null()` to detach (the trait's `dispose` contract).
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the call throws.
    pub(crate) fn set_on_click_listener(
        &mut self,
        view: &JObject<'_>,
        listener: &JObject<'_>,
    ) -> Result<(), NativeWidgetError> {
        self.call_void(
            view,
            jni_str!("setOnClickListener"),
            jni_sig!("(Landroid/view/View$OnClickListener;)V"),
            &[JValue::Object(listener)],
        )
    }

    /// `CompoundButton.setOnCheckedChangeListener(listener)` — `Switch`'s
    /// value listener attach. Pass `&JObject::null()` to detach.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the call throws.
    pub(crate) fn set_on_checked_change_listener(
        &mut self,
        view: &JObject<'_>,
        listener: &JObject<'_>,
    ) -> Result<(), NativeWidgetError> {
        self.call_void(
            view,
            jni_str!("setOnCheckedChangeListener"),
            jni_sig!("(Landroid/widget/CompoundButton$OnCheckedChangeListener;)V"),
            &[JValue::Object(listener)],
        )
    }

    /// `SeekBar.setOnSeekBarChangeListener(listener)` — `Slider`'s value
    /// listener attach. Pass `&JObject::null()` to detach.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the call throws.
    pub(crate) fn set_on_seek_bar_change_listener(
        &mut self,
        view: &JObject<'_>,
        listener: &JObject<'_>,
    ) -> Result<(), NativeWidgetError> {
        self.call_void(
            view,
            jni_str!("setOnSeekBarChangeListener"),
            jni_sig!("(Landroid/widget/SeekBar$OnSeekBarChangeListener;)V"),
            &[JValue::Object(listener)],
        )
    }

    /// `DatePicker.init(year, monthOfYear, dayOfMonth, listener)` — the date
    /// picker's initial date **and** its `OnDateChangedListener` attach in
    /// one framework call (`init` sets the date before it stores the
    /// listener, so the initial date never reaches the listener).
    ///
    /// `date` is a 1-based [`CivilDate`]; the 0-based `monthOfYear`
    /// `DatePicker` wants is derived here, the one place this helper's
    /// callers never have to think about it.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the call throws.
    pub(crate) fn init_date_picker(
        &mut self,
        view: &JObject<'_>,
        date: CivilDate,
        listener: &JObject<'_>,
    ) -> Result<(), NativeWidgetError> {
        self.call_void(
            view,
            jni_str!("init"),
            jni_sig!("(IIILandroid/widget/DatePicker$OnDateChangedListener;)V"),
            &[
                JValue::Int(date.year),
                JValue::Int(i32::from(date.month) - 1),
                JValue::Int(i32::from(date.day)),
                JValue::Object(listener),
            ],
        )
    }

    /// `DatePicker.setOnDateChangedListener(listener)` (API 26, inside this
    /// plugin's `minSdk = 26` floor) — the date picker's detach: pass
    /// `&JObject::null()`. The attach itself goes through
    /// [`Self::init_date_picker`], which sets the initial date in the same
    /// call.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the call throws.
    pub(crate) fn set_on_date_changed_listener(
        &mut self,
        view: &JObject<'_>,
        listener: &JObject<'_>,
    ) -> Result<(), NativeWidgetError> {
        self.call_void(
            view,
            jni_str!("setOnDateChangedListener"),
            jni_sig!("(Landroid/widget/DatePicker$OnDateChangedListener;)V"),
            &[JValue::Object(listener)],
        )
    }

    /// `new FrustNativeListener(slot_id)` set on `view` as each requested
    /// interface — `setOnClickListener` (`click`, any `View`),
    /// `setOnCheckedChangeListener` (`checked_change`, a `CompoundButton`) and
    /// `setOnSeekBarChangeListener` (`seek_bar_change`, a `SeekBar`) — then
    /// one global reference to `view`, which the caller keeps to detach later.
    ///
    /// The **public** `NativeComponent` path's attach
    /// (`crate::component::ComponentCtx::attach_listener`): the same one
    /// listener class and the same setters the built-in controls use, with
    /// the slot id supplied by the component's context rather than by a
    /// control's props — it never reaches the component. The view holds the
    /// listener itself, so no reference to the listener is kept.
    ///
    /// A setter that throws after an earlier one succeeded is unwound before
    /// this returns: [`Self::detach_listener`] nulls exactly the interfaces
    /// that were already set, best effort, so no stray interface survives a
    /// part-way failure (this method's `# Errors`). The slot the caller
    /// tracks (`ComponentCtx`'s `attached`, a control's own registry entry)
    /// never learns of an attach that returned `Err`, so an unwind failure
    /// only means a listener the runtime already considers absent stays
    /// wired a little longer — logged, not propagated, since propagating it
    /// would replace the original failure with a less informative one.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when the listener class is missing,
    /// its constructor throws, or a setter throws — typically
    /// `NoSuchMethodError` for an interface `view`'s class does not have.
    pub(crate) fn attach_listener(
        &mut self,
        view: &JObject<'_>,
        slot_id: SlotId,
        click: bool,
        checked_change: bool,
        seek_bar_change: bool,
    ) -> Result<Global<JObject<'static>>, NativeWidgetError> {
        let listener = self.new_listener(slot_id)?;

        let mut click_set = false;
        let mut checked_change_set = false;

        if click {
            self.set_on_click_listener(view, &listener)?;
            click_set = true;
        }
        if checked_change {
            if let Err(error) = self.set_on_checked_change_listener(view, &listener) {
                self.unwind_partial_attach(view, click_set, checked_change_set, &error);
                return Err(error);
            }
            checked_change_set = true;
        }
        if seek_bar_change {
            if let Err(error) = self.set_on_seek_bar_change_listener(view, &listener) {
                self.unwind_partial_attach(view, click_set, checked_change_set, &error);
                return Err(error);
            }
        }
        self.retain(view)
    }

    /// [`Self::attach_listener`]'s failure unwind: null whatever it had
    /// already set (`click_set`/`checked_change_set`) through the same
    /// [`Self::detach_listener`] the explicit detach path and
    /// [`crate::component::ListenerHandle`]'s Android `Drop` both call —
    /// one function nulling the three interfaces everywhere this crate needs
    /// to, so the setter list is written once.
    ///
    /// `original` is what the caller returns either way; a failure nulling
    /// one of these interfaces is logged rather than replacing it, since the
    /// setter that actually threw is the more useful of the two errors.
    fn unwind_partial_attach(
        &mut self,
        view: &JObject<'_>,
        click_set: bool,
        checked_change_set: bool,
        original: &NativeWidgetError,
    ) {
        if let Err(unwind_error) = self.detach_listener(view, click_set, checked_change_set, false)
        {
            log::warn!(
                "android native-widgets: attach_listener failed ({original}) and unwinding the \
                 interface(s) it had already set also failed: {unwind_error}"
            );
        }
    }

    /// [`Self::attach_listener`]'s inverse: set each requested interface back
    /// to `null` on `view`. Shared by three callers: the explicit detach path
    /// (`crate::component::ComponentCtx::detach_listener`, a control's own
    /// `dispose`), [`Self::unwind_partial_attach`]'s part-way-failure unwind,
    /// and the Android arm of [`crate::component::ListenerHandle`]'s `Drop`,
    /// which is what makes a handle dropped without an explicit detach null
    /// the interfaces it holds too (`ListenerHandle`'s own doc) rather than
    /// only deleting the global reference to the view.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] when a setter throws. A setter that
    /// already ran before the failing one stays nulled; only the later ones
    /// are skipped.
    pub(crate) fn detach_listener(
        &mut self,
        view: &JObject<'_>,
        click: bool,
        checked_change: bool,
        seek_bar_change: bool,
    ) -> Result<(), NativeWidgetError> {
        let none = JObject::null();
        if click {
            self.set_on_click_listener(view, &none)?;
        }
        if checked_change {
            self.set_on_checked_change_listener(view, &none)?;
        }
        if seek_bar_change {
            self.set_on_seek_bar_change_listener(view, &none)?;
        }
        Ok(())
    }

    /// Run `f` inside a pushed JNI local frame, so the references it creates
    /// are released the moment it returns — mandatory around any loop
    /// building more than a handful of objects (module doc's *local-frame
    /// discipline*).
    ///
    /// The inner context re-derives its `Activity`/`Context` as references in
    /// the new frame, so view construction works inside the loop exactly as
    /// it does outside.
    ///
    /// # Errors
    /// Whatever `f` reported, or [`NativeWidgetError::Platform`] when the
    /// frame itself cannot be pushed.
    pub(crate) fn with_frame<T>(
        &mut self,
        capacity: usize,
        f: impl FnOnce(&mut NativeCtx<'_, '_>) -> Result<T, NativeWidgetError>,
    ) -> Result<T, NativeWidgetError> {
        let activity = self.activity;
        let context = self.context;
        self.env.with_local_frame(capacity, |env| {
            let activity = activity.map(|a| env.new_local_ref(a)).transpose()?;
            let context = context.map(|c| env.new_local_ref(c)).transpose()?;
            let mut inner = NativeCtx {
                env,
                activity: activity.as_ref(),
                context: context.as_ref(),
            };
            f(&mut inner)
        })
    }

    /// The application classloader, resolved from this call's `Context` the
    /// first time and cached for the process (module doc).
    fn class_loader(&mut self) -> Result<&'static Global<JObject<'static>>, NativeWidgetError> {
        if let Some(loader) = CLASS_LOADER.get() {
            return Ok(loader);
        }
        let context = self.context()?;
        let loader = run_jni(self.env, "Context.getClassLoader", |env| {
            let loader = env
                .call_method(
                    context,
                    jni_str!("getClassLoader"),
                    jni_sig!("()Ljava/lang/ClassLoader;"),
                    &[],
                )?
                .l()?;
            env.new_global_ref(&loader)
        })?;
        // A racing loser's reference drops immediately, so at most one global
        // reference survives.
        Ok(CLASS_LOADER.get_or_init(|| loader))
    }
}

/// Slot ids are a `u64` counter (`frust_core::widget::next_slot_id`) and Java
/// has no unsigned `long`; the round trip is exact for every id short of
/// 2^63, which the counter cannot reach in a process lifetime.
fn slot_id_to_jlong(slot_id: SlotId) -> i64 {
    slot_id as i64
}

/// The free-function form of [`NativeCtx::run_jni`], for the paths that hold
/// an `Env` without a context (the exports' own prologue).
///
/// `jni` 0.22 returns `Err(Error::JavaException)` and leaves the exception
/// **pending** — undefined behaviour for the next JNI call — so the check and
/// clear happen here whatever `f` reported (the `frust-camera`/
/// `frust-secure-storage` shape).
///
/// # Errors
/// [`NativeWidgetError::Platform`] naming `op`, with the exception's class and
/// message preserved when there was one.
pub(crate) fn run_jni<'local, T>(
    env: &mut Env<'local>,
    op: &str,
    f: impl FnOnce(&mut Env<'local>) -> Result<T, jni::errors::Error>,
) -> Result<T, NativeWidgetError> {
    let result = f(env);
    if env.exception_check() {
        return Err(take_pending_exception(env, op));
    }
    result.map_err(|e| NativeWidgetError::Platform(format!("android native-widgets: {op}: {e}")))
}

/// Extract, **clear**, and describe the pending Java exception. Clears first
/// (mirroring `jni`'s own `exception_catch`) so the class/message queries
/// below run without a pending exception; the final check covers the unlikely
/// case one of those queries throws in turn.
fn take_pending_exception(env: &mut Env<'_>, op: &str) -> NativeWidgetError {
    let Some(throwable) = env.exception_occurred() else {
        env.exception_clear();
        return NativeWidgetError::Platform(format!(
            "android native-widgets: {op}: JNI reported an exception with no throwable"
        ));
    };
    env.exception_clear();

    let class_name = match env.get_object_class(&throwable) {
        Ok(class) => match class.get_name(env) {
            Ok(name) => name.to_string(),
            Err(_) => "<unknown exception class>".to_string(),
        },
        Err(_) => "<unknown exception class>".to_string(),
    };
    let message = match throwable.get_message(env) {
        Ok(message) => message.to_string(),
        Err(_) => "<no message>".to_string(),
    };

    if env.exception_check() {
        env.exception_clear();
    }

    NativeWidgetError::Platform(format!(
        "android native-widgets: {op}: {class_name}: {message}"
    ))
}
