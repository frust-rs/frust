//! The WGSL shader module registry: [`ShaderLibrary`] and [`ShaderId`].
//!
//! Every shader the engine ever draws with is registered **once**, at
//! library-build time, and referred to afterwards by an opaque [`ShaderId`].
//! That indirection is what lets [`crate::pipeline::RenderPipelineDesc`] stay
//! a cheap, hashable value: a pipeline variant names its program by id instead
//! of carrying a module handle or a source string around.
//!
//! ## Registered at build time, created on first use
//!
//! Registering a shader ([`ShaderLibrary::register_wgsl`]) stores its name and
//! its WGSL source and mints the id; it does not touch the device. The
//! `wgpu::ShaderModule` is created on the first request for that id
//! ([`ShaderLibrary::module`]) by whichever thread asks — the pipeline warm-up
//! worker for a listed variant, the render thread for one it steals, or a
//! thread started only to create modules ahead of a frame
//! ([`crate::pipeline::PipelineCache::warm_up_shaders`]) — and exactly once:
//! each entry holds its module in a once-cell, so a second thread asking for a
//! module that is mid-creation blocks until the first one publishes it rather
//! than creating a second copy. The source is dropped once its module exists.
//!
//! The deferral is the point: a module's creation is wgpu parsing and
//! validating the whole WGSL text (naga), which costs milliseconds per module
//! on a phone and grows with the source's length, so creating every module of
//! a large program family at start-up put all of it in front of the first
//! frame. [`ShaderLibrary::insert_wgsl`] is the eager form — register and
//! create on the calling thread at once — and [`ShaderLibrary::compile_all`]
//! creates every module still missing, for a caller that wants the old
//! behaviour.
//!
//! The library is append-only and keyed by name, so building it is a
//! start-up-time act and everything after it is a lookup. Once built it is
//! wrapped in an `Arc` and shared — a `wgpu::Device` and a
//! `wgpu::ShaderModule` are `Send + Sync`, so the pipeline warm-up worker
//! compiles against exactly the same modules the render thread does, with no
//! second compile of the source.
//!
//! Every module is created with a label (`frust-gpu shader: <name>`) so a
//! capture in RenderDoc/Xcode names the shader rather than showing an
//! anonymous module.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

/// An opaque handle to a WGSL module held by one [`ShaderLibrary`].
///
/// Only [`ShaderLibrary::register_wgsl`] (and its eager form,
/// [`ShaderLibrary::insert_wgsl`]) mints one, and an id is only meaningful
/// against the library that minted it — [`ShaderLibrary::module`] on a foreign
/// id returns `None` rather than a wrong module, but the pipeline layer treats
/// that as a caller bug (see [`crate::pipeline::PipelineCache::get_or_create`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShaderId(u32);

impl ShaderId {
    /// The library-local index this id addresses — the value the pipeline
    /// key packs into its program field.
    pub(crate) const fn raw(self) -> u32 {
        self.0
    }

    /// Rebuilds an id from a raw index, for the pipeline layer's key packing
    /// and for device-free tests that need an id without a library.
    pub(crate) const fn from_raw(index: u32) -> Self {
        Self(index)
    }
}

/// One entry: the name it was registered under, its source until the module
/// exists, and the module once some thread has asked for it.
struct Entry {
    name: String,
    /// The WGSL text, taken (and dropped) by the creation that succeeds. Only
    /// the thread running the once-cell's initializer ever locks it.
    source: Mutex<Option<Cow<'static, str>>>,
    module: OnceLock<wgpu::ShaderModule>,
}

impl std::fmt::Debug for Entry {
    // The source is tens of kilobytes of WGSL for the engine's strip
    // programs; a debug print names the entry and its state instead.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("name", &self.name)
            .field("created", &self.module.get().is_some())
            .finish()
    }
}

impl Entry {
    /// The module, created now by the calling thread if no thread has created
    /// it yet, or waited for if another thread is creating it right now.
    fn module(&self, device: &wgpu::Device) -> &wgpu::ShaderModule {
        self.module.get_or_init(|| {
            let mut source = self.source.lock().unwrap_or_else(PoisonError::into_inner);
            let module = {
                let wgsl: &str = source.as_deref().unwrap_or_else(|| {
                    // Unreachable: the source is taken only after a creation
                    // returned, and the once-cell then holds that module. An
                    // empty module fails validation through the device's
                    // error handler, like any malformed one.
                    log::error!(
                        "frust-gpu: shader `{}` has no source left to create its module from",
                        self.name
                    );
                    ""
                });
                device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(&format!("frust-gpu shader: {}", self.name)),
                    source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(wgsl)),
                })
            };
            // Taken only once the creation has returned: a creation that
            // unwinds (wgpu's default error handler panics on a malformed
            // module) leaves the source for the next request to retry with.
            *source = None;
            module
        })
    }
}

/// A name-keyed, append-only set of WGSL modules, each created on first use.
///
/// Build it at start-up, then share it (`Arc<ShaderLibrary>`) with the
/// pipeline cache and its warm-up worker. Registration is idempotent per name:
/// re-registering a name that is already present returns the existing id and
/// keeps the first source, so a caller that cannot easily prove it only
/// registers once still pays for one module.
#[derive(Debug, Default)]
pub struct ShaderLibrary {
    entries: Vec<Entry>,
    by_name: HashMap<String, ShaderId>,
}

impl ShaderLibrary {
    /// An empty library.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `src` as WGSL under `name` and returns its id, without
    /// creating its module: that happens on the first [`Self::module`]
    /// request for the id, on whichever thread makes it.
    ///
    /// If `name` is already registered the existing id is returned and `src`
    /// is dropped — the library is append-only, so a name never changes
    /// meaning once it is in.
    pub fn register_wgsl(
        &mut self,
        name: impl Into<String>,
        src: impl Into<Cow<'static, str>>,
    ) -> ShaderId {
        let name = name.into();
        if let Some(existing) = self.by_name.get(&name) {
            return *existing;
        }
        let id = ShaderId::from_raw(self.entries.len() as u32);
        self.entries.push(Entry {
            name: name.clone(),
            source: Mutex::new(Some(src.into())),
            module: OnceLock::new(),
        });
        self.by_name.insert(name, id);
        id
    }

    /// Registers `src` under `name` ([`Self::register_wgsl`]) and creates its
    /// module on the calling thread at once — the eager form, for a caller
    /// that wants the module to exist before any pipeline asks for it.
    ///
    /// The module is labelled with `name`. If `name` is already registered the
    /// existing id is returned, `src` is not used, and the registered source's
    /// module is created if it does not exist yet.
    ///
    /// Compilation errors are not raised here: wgpu reports a malformed module
    /// through the device's error scope / uncaptured-error handler, and the
    /// resulting pipeline creation fails there. This mirrors `frust-render`'s
    /// shader-effect path, which records the failure rather than panicking.
    pub fn insert_wgsl(
        &mut self,
        device: &wgpu::Device,
        name: impl Into<String>,
        src: &str,
    ) -> ShaderId {
        let id = self.register_wgsl(name, src.to_owned());
        let _module = self.module(device, id);
        id
    }

    /// The module `id` addresses, creating it now if no thread has yet —
    /// or waiting for the thread creating it right now — or `None` if `id`
    /// was minted by a different library.
    ///
    /// Each module is created at most once however many threads ask at the
    /// same time. Errors surface as for [`Self::insert_wgsl`], on the thread
    /// that creates the module.
    #[must_use]
    pub fn module(&self, device: &wgpu::Device, id: ShaderId) -> Option<&wgpu::ShaderModule> {
        self.entries
            .get(id.raw() as usize)
            .map(|entry| entry.module(device))
    }

    /// Creates every registered module that does not exist yet, on the
    /// calling thread — the eager start-up behaviour, for tests and for a
    /// caller that wants every module in place before its first frame.
    pub fn compile_all(&self, device: &wgpu::Device) {
        for entry in &self.entries {
            let _module = entry.module(device);
        }
    }

    /// The module `id` addresses if it has already been created, or `None` —
    /// either because no thread has asked for it yet (see [`Self::module`]),
    /// or because `id` was minted by a different library.
    #[must_use]
    pub fn get(&self, id: ShaderId) -> Option<&wgpu::ShaderModule> {
        self.entries
            .get(id.raw() as usize)
            .and_then(|entry| entry.module.get())
    }

    /// Whether `id`'s module has been created (`false` for a foreign id).
    #[must_use]
    pub fn is_compiled(&self, id: ShaderId) -> bool {
        self.get(id).is_some()
    }

    /// The id registered under `name`, if any.
    #[must_use]
    pub fn id_of(&self, name: &str) -> Option<ShaderId> {
        self.by_name.get(name).copied()
    }

    /// The name `id` was registered under — used for pipeline labels.
    #[must_use]
    pub fn name_of(&self, id: ShaderId) -> Option<&str> {
        self.entries.get(id.raw() as usize).map(|e| e.name.as_str())
    }

    /// How many distinct modules are registered, created or not.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// How many of the registered modules have been created so far.
    #[must_use]
    pub fn compiled_len(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.module.get().is_some())
            .count()
    }

    /// Whether no module is registered yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_library_resolves_nothing() {
        let lib = ShaderLibrary::new();
        assert!(lib.is_empty());
        assert_eq!(lib.len(), 0);
        assert_eq!(lib.compiled_len(), 0);
        assert!(lib.get(ShaderId::from_raw(0)).is_none());
        assert!(!lib.is_compiled(ShaderId::from_raw(0)));
        assert!(lib.name_of(ShaderId::from_raw(0)).is_none());
        assert!(lib.id_of("missing").is_none());
    }

    #[test]
    fn ids_round_trip_through_their_raw_index() {
        for index in [0u32, 1, 7, u32::MAX] {
            assert_eq!(ShaderId::from_raw(index).raw(), index);
        }
        assert_ne!(ShaderId::from_raw(0), ShaderId::from_raw(1));
    }

    /// Registration needs no device and creates no module: the ids, names and
    /// count are all there, and nothing is compiled until a module is asked
    /// for. A repeated name keeps its first id and its first source.
    #[test]
    fn registering_mints_ids_without_creating_a_module() {
        let mut lib = ShaderLibrary::new();
        let a = lib.register_wgsl("a", "@fragment fn fs_main() {}");
        let b = lib.register_wgsl("b", String::from("@fragment fn fs_main() {}"));
        assert_ne!(a, b);
        assert_eq!(lib.register_wgsl("a", "something else"), a);
        assert_eq!(lib.len(), 2);
        assert!(!lib.is_empty());
        assert_eq!(lib.compiled_len(), 0, "registration created a module");
        assert!(lib.get(a).is_none());
        assert!(!lib.is_compiled(b));
        assert_eq!(lib.id_of("b"), Some(b));
        assert_eq!(lib.name_of(a), Some("a"));
        assert!(
            format!("{lib:?}").contains("created: false"),
            "a debug print names each entry's state, not its source"
        );
    }
}
