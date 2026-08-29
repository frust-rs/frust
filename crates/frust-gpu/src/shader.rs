//! The WGSL shader module registry: [`ShaderLibrary`] and [`ShaderId`].
//!
//! Every shader the engine ever draws with is compiled into a
//! `wgpu::ShaderModule` **once**, at library-build time, and referred to
//! afterwards by an opaque [`ShaderId`]. That indirection is what lets
//! [`crate::pipeline::RenderPipelineDesc`] stay a cheap, hashable value: a
//! pipeline variant names its program by id instead of carrying a module
//! handle or a source string around.
//!
//! The library is append-only and keyed by name, so building it is a
//! start-up-time act (`insert_wgsl` per shader) and everything after it is a
//! lookup. Once built it is wrapped in an `Arc` and shared — a
//! `wgpu::ShaderModule` is `Send + Sync`, so the pipeline warm-up worker
//! compiles against exactly the same modules the render thread does, with no
//! second compile of the source.
//!
//! Every module is created with a label (`frust-gpu shader: <name>`) so a
//! capture in RenderDoc/Xcode names the shader rather than showing an
//! anonymous module.

use std::collections::HashMap;

/// An opaque handle to a WGSL module held by one [`ShaderLibrary`].
///
/// Only [`ShaderLibrary::insert_wgsl`] mints one, and an id is only meaningful
/// against the library that minted it — [`ShaderLibrary::get`] on a foreign id
/// returns `None` rather than a wrong module, but the pipeline layer treats
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

/// One entry: the compiled module plus the name it was registered under.
#[derive(Debug)]
struct Entry {
    name: String,
    module: wgpu::ShaderModule,
}

/// A name-keyed, append-only set of compiled WGSL modules.
///
/// Build it at start-up, then share it (`Arc<ShaderLibrary>`) with the
/// pipeline cache and its warm-up worker. Insertion is idempotent per name:
/// re-inserting a name that is already present returns the existing id
/// without recompiling, so a caller that cannot easily prove it only
/// registers once still pays for one compile.
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

    /// Compiles `src` as WGSL and registers it under `name`, returning its id.
    ///
    /// The module is labelled with `name`. If `name` is already registered the
    /// existing id is returned and `src` is *not* compiled again — the library
    /// is append-only, so a name never changes meaning once it is in.
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
        let name = name.into();
        if let Some(existing) = self.by_name.get(&name) {
            return *existing;
        }

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&format!("frust-gpu shader: {name}")),
            source: wgpu::ShaderSource::Wgsl(src.into()),
        });

        let id = ShaderId::from_raw(self.entries.len() as u32);
        self.entries.push(Entry {
            name: name.clone(),
            module,
        });
        self.by_name.insert(name, id);
        id
    }

    /// The module `id` addresses, or `None` if it was minted by a different
    /// library.
    #[must_use]
    pub fn get(&self, id: ShaderId) -> Option<&wgpu::ShaderModule> {
        self.entries.get(id.raw() as usize).map(|e| &e.module)
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

    /// How many distinct modules are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
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
        assert!(lib.get(ShaderId::from_raw(0)).is_none());
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
}
