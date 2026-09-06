//! Texture handles and render-target descriptors: [`TextureDesc`],
//! [`Texture`], [`RenderTarget`]/[`Attachment`], and the two id types a
//! texture crosses a layer boundary under.
//!
//! [`TextureId`] is a caller-assigned handle for an external binding (a
//! plugin import, a platform surface's backing texture); [`SceneTextureId`]
//! is minted by [`Texture::as_scene_texture`] and is the key a future
//! render-engine [`TextureRegistry`] resolves `Command::SceneTexture`
//! against — the same process-wide-`AtomicU64`, stable-across-`Clone`
//! mechanism `frust_scene::ShaderProgram::id` uses.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Process-unique id counter backing [`SceneTextureId::mint`].
static NEXT_SCENE_TEXTURE_ID: AtomicU64 = AtomicU64::new(1);

/// The bit [`SceneTextureId::for_shader_program`] sets so an offscreen shader
/// effect's target id can never be one [`SceneTextureId::mint`] hands out.
///
/// `mint` counts up from 1 off a single process-wide counter, so an id with
/// the top bit set is one it would only reach after 2^63 mints — more textures
/// than a process can create. Reserving that bit makes the two id spaces
/// disjoint by construction rather than by luck, which is what lets a
/// fragment program's own ordinal be reused as the id its rendered target is
/// registered under without ever colliding with a host texture carrying the
/// same ordinal.
const SHADER_PROGRAM_NAMESPACE: u64 = 1 << 63;

/// Describes a texture to create: dimensions, format, usage, and an optional
/// debug label.
///
/// `sample_count` is deliberately **not** a field here — this crate creates
/// no multisampled texture, and [`Self::sample_count`] always answers `1`;
/// there is no setter to request anything else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureDesc {
    /// Texture width in texels.
    pub width: u32,
    /// Texture height in texels.
    pub height: u32,
    /// The texture's pixel format.
    pub format: wgpu::TextureFormat,
    /// How the texture may be used (render attachment, sampled binding,
    /// copy source/destination, …).
    pub usage: wgpu::TextureUsages,
    /// Optional debug label, surfaced in adapter/validation diagnostics.
    pub label: Option<String>,
}

impl TextureDesc {
    /// The sample count every texture built from this descriptor uses.
    /// Always `1` — no MSAA. Not a field, so there is nothing to set.
    pub const fn sample_count(&self) -> u32 {
        1
    }
}

/// A caller-assigned id for a texture crossing an external binding boundary
/// — a plugin's imported texture, or a platform surface's backing texture.
///
/// Distinct from [`SceneTextureId`]: this one is supplied by the caller, not
/// minted by this crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureId(pub u64);

/// A process-unique id [`Texture::as_scene_texture`] mints once per
/// [`Texture`] and shares across `Clone` — the [`TextureRegistry`] key
/// `Command::SceneTexture` resolves against.
///
/// Mirrors `frust_scene::ShaderProgram::id`: minted fresh only at
/// construction (never re-minted on `Clone`), off a process-wide
/// [`AtomicU64`] so two distinct textures never collide even when built on
/// different threads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SceneTextureId(u64);

impl SceneTextureId {
    /// Mints a fresh, process-unique id.
    ///
    /// [`Texture::new`] mints one per texture and shares it across `Clone`,
    /// and that stays the ordinary route. This is public for the caller that
    /// has no [`Texture`] to mint from: a texture rendered *externally* — one
    /// whose `wgpu::TextureView` its owner creates, re-creates and binds
    /// itself every frame — still needs one stable id to be named by across
    /// those re-creations, and minting here is what hands it one out of the
    /// same process-wide counter. The shader-program id space stays reserved
    /// against every such mint by construction (see
    /// [`Self::for_shader_program`]).
    #[must_use]
    pub fn mint() -> Self {
        Self(NEXT_SCENE_TEXTURE_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// The id under which the offscreen target of the fragment program
    /// `program_id` is registered.
    ///
    /// Derived from the program's own ordinal rather than minted, because both
    /// halves of the seam have to name the same texture while holding
    /// different things: the pre-pass that renders the program has the target,
    /// and the display-list walk that draws it has only
    /// `frust_scene::ShaderProgram::id`. A pure function of the program id
    /// lets each compute the id the other used, with no shared side table to
    /// keep in step.
    ///
    /// Disjoint from [`Self::mint`]'s range by construction — see
    /// [`SHADER_PROGRAM_NAMESPACE`] — so a host texture and a shader target
    /// can never answer to the same id. A `program_id` large enough to reach
    /// the reserved bit itself is impossible for the same counting reason its
    /// minter is.
    #[must_use]
    pub const fn for_shader_program(program_id: u64) -> Self {
        Self(program_id | SHADER_PROGRAM_NAMESPACE)
    }

    /// The opaque value this id wraps.
    ///
    /// The counterpart of `frust_scene::ShaderProgram::id`, and for the same
    /// reason: the scene layer carries an externally owned texture as a plain
    /// `u64` so it depends on no GPU crate, and a caller that registered a
    /// [`Texture`] needs some way to say which one a display-list command
    /// means. This only reads back what [`Self::mint`] handed out; it never
    /// mints.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Whether `self` carries the bit [`Self::for_shader_program`] sets —
    /// i.e. whether it names a shader program's own offscreen target rather
    /// than something an ordinary caller minted.
    ///
    /// Lets a caller outside this module test which namespace an id belongs
    /// to without the reserved bit itself being exported — a check on the id
    /// stays valid even if the bit's exact value ever changed.
    #[must_use]
    pub const fn is_shader_program(self) -> bool {
        self.0 & SHADER_PROGRAM_NAMESPACE != 0
    }
}

/// A GPU texture: the underlying texture/view pair, the [`TextureDesc`] it
/// was created from, and a [`SceneTextureId`] minted once at construction
/// and shared across `Clone`.
///
/// Generic over the wrapped texture (`T`, default `wgpu::Texture`) and view
/// (`V`, default `wgpu::TextureView`) types rather than hardcoded to them, so
/// [`Self::as_scene_texture`]'s id-uniqueness and clone-stability contract
/// stays host-testable with plain stand-in values — no GPU device needed to
/// mint a real `wgpu::Texture`/`wgpu::TextureView` pair just to exercise the
/// id logic. The real engine always instantiates the default `Texture`
/// (`Texture<wgpu::Texture, wgpu::TextureView>`).
#[derive(Clone, Debug)]
pub struct Texture<T = wgpu::Texture, V = wgpu::TextureView> {
    texture: T,
    view: V,
    desc: TextureDesc,
    scene_id: SceneTextureId,
}

impl<T, V> Texture<T, V> {
    /// Wraps an already-created texture/view pair with the [`TextureDesc`]
    /// it was built from, minting a fresh [`SceneTextureId`].
    ///
    /// Device/texture/view creation itself is a caller concern (this crate's
    /// device-init work is tracked separately) — this constructor only
    /// assembles the handle and mints its scene id.
    pub fn new(texture: T, view: V, desc: TextureDesc) -> Self {
        Self {
            texture,
            view,
            desc,
            scene_id: SceneTextureId::mint(),
        }
    }

    /// The underlying texture.
    pub fn texture(&self) -> &T {
        &self.texture
    }

    /// The texture's full-extent view.
    pub fn view(&self) -> &V {
        &self.view
    }

    /// `(width, height)` in texels, from the originating [`TextureDesc`].
    pub fn size(&self) -> (u32, u32) {
        (self.desc.width, self.desc.height)
    }

    /// The texture's pixel format, from the originating [`TextureDesc`].
    pub fn format(&self) -> wgpu::TextureFormat {
        self.desc.format
    }

    /// The originating [`TextureDesc`].
    pub fn desc(&self) -> &TextureDesc {
        &self.desc
    }

    /// The process-unique [`SceneTextureId`] minted for this texture at
    /// construction, stable across `Clone`.
    pub fn as_scene_texture(&self) -> SceneTextureId {
        self.scene_id
    }
}

/// One attachment of a [`RenderTarget`]: the view rendered into, and the
/// load/store ops framing that render pass's access to it.
///
/// Generic over the clear-value type (`V`) because a color attachment's
/// clear value is `wgpu::Color` while a depth attachment's is `f32` — the
/// same split `wgpu::RenderPassColorAttachment`/`RenderPassDepthStencilAttachment`
/// draw between themselves.
#[derive(Clone, Debug)]
pub struct Attachment<'a, V> {
    /// The view rendered into.
    pub view: &'a wgpu::TextureView,
    /// How the attachment's prior contents are treated at pass start.
    pub load: wgpu::LoadOp<V>,
    /// Whether the attachment's contents are kept or discarded at pass end.
    pub store: wgpu::StoreOp,
}

/// A [`RenderTarget`]'s color attachment: clear value is `wgpu::Color`.
pub type ColorAttachment<'a> = Attachment<'a, wgpu::Color>;

/// A [`RenderTarget`]'s depth attachment: clear value is a plain `f32`
/// depth, matching `wgpu::RenderPassDepthStencilAttachment`'s depth op.
pub type DepthAttachment<'a> = Attachment<'a, f32>;

/// The set of attachments a render pass targets: zero or more color
/// attachments plus an optional depth attachment.
#[derive(Clone, Debug, Default)]
pub struct RenderTarget<'a> {
    /// Color attachments, in binding order.
    pub color: Vec<ColorAttachment<'a>>,
    /// The depth attachment, if the pass writes depth.
    pub depth: Option<DepthAttachment<'a>>,
}

/// The engine's live map from a minted [`SceneTextureId`] to the view
/// `Command::SceneTexture` resolves against.
///
/// Generic over the stored view type (default `wgpu::TextureView`, the real
/// engine's usage) rather than hardcoded to it, so the registry's own
/// insert/get/remove behavior stays host-testable with a plain stand-in
/// value — no GPU device needed to exercise the map logic itself.
#[derive(Debug)]
pub struct TextureRegistry<V = wgpu::TextureView> {
    views: HashMap<SceneTextureId, V>,
}

impl<V> Default for TextureRegistry<V> {
    fn default() -> Self {
        Self {
            views: HashMap::new(),
        }
    }
}

impl<V> TextureRegistry<V> {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts (or replaces) the view registered under `id`, returning the
    /// previous entry, if any.
    pub fn insert(&mut self, id: SceneTextureId, view: V) -> Option<V> {
        self.views.insert(id, view)
    }

    /// The view registered under `id`, if any.
    pub fn get(&self, id: SceneTextureId) -> Option<&V> {
        self.views.get(&id)
    }

    /// Removes and returns the view registered under `id`, if any.
    pub fn remove(&mut self, id: SceneTextureId) -> Option<V> {
        self.views.remove(&id)
    }

    /// The number of registered entries.
    pub fn len(&self) -> usize {
        self.views.len()
    }

    /// Whether the registry holds no entries.
    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_desc() -> TextureDesc {
        TextureDesc {
            width: 64,
            height: 64,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            label: Some("test".to_string()),
        }
    }

    #[test]
    fn sample_count_is_always_one_and_has_no_setter() {
        // `TextureDesc` exposes no `sample_count` field to set — the type
        // only has `sample_count()`, which always answers `1`.
        let desc = fake_desc();
        assert_eq!(desc.sample_count(), 1);
    }

    /// A stand-in `Texture` with no GPU dependency: `u32` fills in for both
    /// the wrapped texture and view, so `Texture::new` and
    /// `as_scene_texture` are exercised exactly as the real
    /// `Texture<wgpu::Texture, wgpu::TextureView>` would, with no device.
    fn fake_texture() -> Texture<u32, u32> {
        Texture::new(0, 0, fake_desc())
    }

    #[test]
    fn a_shader_program_id_never_collides_with_a_minted_one() {
        // Every minted id stays in the counter's own range; every shader
        // target id carries the reserved bit, so the two can never meet.
        for _ in 0..64 {
            let minted = fake_texture().as_scene_texture();
            assert_eq!(minted.get() & SHADER_PROGRAM_NAMESPACE, 0);
            assert_ne!(minted, SceneTextureId::for_shader_program(minted.get()));
        }
    }

    #[test]
    fn is_shader_program_distinguishes_the_two_namespaces() {
        let minted = fake_texture().as_scene_texture();
        let shader = SceneTextureId::for_shader_program(minted.get());
        assert!(!minted.is_shader_program());
        assert!(shader.is_shader_program());
    }

    #[test]
    fn a_shader_program_id_is_a_pure_function_of_the_program() {
        // Both halves of the seam derive the same id from the same program,
        // and distinct programs stay distinct.
        assert_eq!(
            SceneTextureId::for_shader_program(9),
            SceneTextureId::for_shader_program(9)
        );
        assert_ne!(
            SceneTextureId::for_shader_program(9),
            SceneTextureId::for_shader_program(10)
        );
    }

    #[test]
    fn as_scene_texture_mints_unique_ids() {
        let a = fake_texture();
        let b = fake_texture();
        assert_ne!(a.as_scene_texture(), b.as_scene_texture());
    }

    #[test]
    fn as_scene_texture_mints_unique_ids_across_threads() {
        let handles: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(|| fake_texture().as_scene_texture()))
            .collect();
        let mut ids: Vec<SceneTextureId> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        ids.sort_by_key(|id| id.0);
        ids.dedup();
        assert_eq!(ids.len(), 8, "all ids must be unique across threads");
    }

    #[test]
    fn as_scene_texture_is_stable_across_clone() {
        let a = fake_texture();
        let b = a.clone();
        assert_eq!(a.as_scene_texture(), b.as_scene_texture());
    }

    #[test]
    fn texture_id_wraps_the_caller_supplied_value() {
        let id = TextureId(42);
        assert_eq!(id.0, 42);
        assert_eq!(id, TextureId(42));
        assert_ne!(id, TextureId(43));
    }

    #[test]
    fn render_target_defaults_to_no_attachments() {
        let target: RenderTarget = RenderTarget::default();
        assert!(target.color.is_empty());
        assert!(target.depth.is_none());
    }

    #[test]
    fn registry_insert_get_remove_round_trip() {
        let mut registry: TextureRegistry<&'static str> = TextureRegistry::new();
        let id = SceneTextureId::mint();
        assert!(registry.is_empty());

        assert_eq!(registry.insert(id, "view-a"), None);
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.get(id), Some(&"view-a"));

        assert_eq!(registry.insert(id, "view-b"), Some("view-a"));
        assert_eq!(registry.get(id), Some(&"view-b"));

        assert_eq!(registry.remove(id), Some("view-b"));
        assert_eq!(registry.get(id), None);
        assert!(registry.is_empty());
    }

    #[test]
    fn registry_get_and_remove_miss_on_unknown_id() {
        let mut registry: TextureRegistry<u32> = TextureRegistry::new();
        let unknown = SceneTextureId::mint();
        assert_eq!(registry.get(unknown), None);
        assert_eq!(registry.remove(unknown), None);
    }

    #[test]
    fn registry_distinguishes_ids() {
        let mut registry: TextureRegistry<u32> = TextureRegistry::new();
        let a = SceneTextureId::mint();
        let b = SceneTextureId::mint();
        registry.insert(a, 1);
        registry.insert(b, 2);
        assert_eq!(registry.get(a), Some(&1));
        assert_eq!(registry.get(b), Some(&2));
        assert_eq!(registry.len(), 2);
    }
}
