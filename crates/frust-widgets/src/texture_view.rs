//! The `TextureView`/`TextureWidget` leaf: draws an externally rendered scene
//! texture — a video frame, a camera preview, a 3D view — by the id its
//! producer registered it under.
//!
//! [`texture_view`] takes that id and nothing else: the renderer's frame bus
//! reports it (`TextureRegistration::scene_texture_id`), as does any other
//! external pass's own `SceneTextureId::get`. This crate never sees the
//! texture itself; it paints a `draw_scene_texture(id, dest)` command and the
//! engine composites whatever is bound under the id that frame (nothing, if
//! nothing is bound yet — never an error).
//!
//! `.fit(..)` places the texture in the widget's box like [`crate::Image`]'s
//! fit does, given the texture's own extent from `.source_size(..)` (the
//! widget cannot read it back from the GPU); without one every fit fills the
//! box. `.corner_radius(..)` clips the box to rounded corners. Sizing is
//! `.expand()` by default, or `.size(..)`.
//!
//! # Inside a retained bracket
//!
//! The texture changes behind the display list's back — a new frame is
//! uploaded without the widget repainting — so a retained subtree holding a
//! texture view cannot be replayed from a cached body: the engine accepts the
//! draw inside a retained bracket but marks that body unretainable and paints
//! it every frame. Keep a texture view outside a repaint boundary's subtree
//! when the rest of that subtree is worth retaining.
//!
//! ```
//! use frust_core::View;
//! use frust_widgets::{TextureFit, texture_view};
//! use kurbo::Size;
//!
//! fn player(video_id: u64) -> impl View<()> + use<> {
//!     texture_view(video_id)
//!         .source_size(Size::new(1920.0, 1080.0))
//!         .fit(TextureFit::Contain)
//!         .corner_radius(12.0)
//!         .label("Video")
//! }
//! # let _ = player;
//! ```

use frust_core::accesskit::Role;
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View,
    Widget,
};
use kurbo::{Rect, Size};

/// How a [`TextureView`] places its texture in the widget's box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextureFit {
    /// Scale (preserving aspect ratio) to fit entirely inside the box,
    /// centred; the other axis is letterboxed.
    #[default]
    Contain,
    /// Scale (preserving aspect ratio) to cover the box entirely, centred;
    /// the overflow is clipped to the box.
    Cover,
    /// Stretch to exactly the box, ignoring aspect ratio.
    Fill,
}

/// How a [`TextureView`] resolves its layout size.
#[derive(Clone, Copy, Debug, PartialEq)]
enum TextureSize {
    /// Fill the incoming constraints' maximum.
    Expand,
    /// A fixed size, clamped into the incoming constraints.
    Explicit(Size),
}

/// The rect (relative to the widget's own origin) a `natural`-sized texture
/// is drawn into inside a `dest`-sized box under `fit`.
///
/// `Fill` — or a degenerate `natural`/`dest`, which has no aspect ratio to
/// keep — is exactly the box. `Contain` is centred and no larger than the box
/// on either axis; `Cover` is centred and no smaller (the caller clips).
#[must_use]
pub fn texture_fit_rect(natural: Size, dest: Size, fit: TextureFit) -> Rect {
    let degenerate = !(natural.width > 0.0
        && natural.height > 0.0
        && dest.width > 0.0
        && dest.height > 0.0
        && natural.width.is_finite()
        && natural.height.is_finite());
    let fit_width = dest.width / natural.width;
    let fit_height = dest.height / natural.height;
    let scale = match fit {
        _ if degenerate => return Rect::from_origin_size((0.0, 0.0), dest),
        TextureFit::Fill => return Rect::from_origin_size((0.0, 0.0), dest),
        TextureFit::Contain => fit_width.min(fit_height),
        TextureFit::Cover => fit_width.max(fit_height),
    };
    let width = natural.width * scale;
    let height = natural.height * scale;
    let x = (dest.width - width) / 2.0;
    let y = (dest.height - height) / 2.0;
    Rect::new(x, y, x + width, y + height)
}

/// A declarative view of an externally rendered scene texture. See the
/// [module docs](self).
#[derive(Clone, Debug, PartialEq)]
pub struct TextureView {
    id: u64,
    fit: TextureFit,
    source_size: Option<Size>,
    size: TextureSize,
    corner_radius: f64,
    label: Option<String>,
}

/// A view drawing the scene texture registered under `scene_texture_id`,
/// full-bleed and [`TextureFit::Contain`] by default. See the
/// [module docs](self).
#[must_use]
pub fn texture_view(scene_texture_id: u64) -> TextureView {
    TextureView {
        id: scene_texture_id,
        fit: TextureFit::default(),
        source_size: None,
        size: TextureSize::Expand,
        corner_radius: 0.0,
        label: None,
    }
}

impl TextureView {
    /// How the texture is placed in the box. Needs [`Self::source_size`] to
    /// keep an aspect ratio; without it every fit fills the box.
    #[must_use]
    pub fn fit(mut self, fit: TextureFit) -> Self {
        self.fit = fit;
        self
    }

    /// The texture's own extent (a video's resolution), which `fit` keeps the
    /// aspect ratio of. Only the ratio matters.
    #[must_use]
    pub fn source_size(mut self, size: Size) -> Self {
        self.source_size = Some(size);
        self
    }

    /// A fixed size, clamped into the parent's constraints. Overrides
    /// [`Self::expand`] when called after it.
    #[must_use]
    pub fn size(mut self, size: Size) -> Self {
        self.size = TextureSize::Explicit(size);
        self
    }

    /// Fill the parent's maximum constraints — the default.
    #[must_use]
    pub fn expand(mut self) -> Self {
        self.size = TextureSize::Expand;
        self
    }

    /// Clip the box to corners of `radius`; `0.0` (the default) is square.
    #[must_use]
    pub fn corner_radius(mut self, radius: f64) -> Self {
        self.corner_radius = radius.max(0.0);
        self
    }

    /// An accessible label, contributing a [`Role::Image`] semantics node; an
    /// unlabelled texture view is decorative and reports nothing.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl<State: 'static> View<State> for TextureView {
    type Element = TextureWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TextureWidget {
        TextureWidget { view: self.clone() }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TextureWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev == self {
            return ChangeFlags::NONE;
        }
        let mut flags = ChangeFlags::PAINT;
        if prev.size != self.size {
            flags |= ChangeFlags::LAYOUT;
        }
        element.view = self.clone();
        flags
    }
}

/// The retained widget for a [`TextureView`].
pub struct TextureWidget {
    view: TextureView,
}

impl TextureWidget {
    /// The scene texture id this widget draws.
    #[must_use]
    pub fn scene_texture_id(&self) -> u64 {
        self.view.id
    }
}

impl Widget for TextureWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        match self.view.size {
            TextureSize::Explicit(size) => bc.constrain(size),
            TextureSize::Expand => bc.max(),
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();
        let natural = self.view.source_size.unwrap_or(size);
        let local = texture_fit_rect(natural, size, self.view.fit);
        let dest = local + origin.to_vec2();

        let overflows =
            local.x0 < 0.0 || local.y0 < 0.0 || local.x1 > size.width || local.y1 > size.height;
        let clip = if self.view.corner_radius > 0.0 {
            scene.push_clip_rounded(origin, size, self.view.corner_radius);
            true
        } else if overflows {
            scene.push_clip(origin, size);
            true
        } else {
            false
        };
        scene.draw_scene_texture(self.view.id, dest);
        if clip {
            scene.pop_clip();
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if let Some(label) = &self.view.label {
            ctx.push_node(Role::Image, |node| {
                node.set_label(label.as_str());
            });
        }
    }
}
