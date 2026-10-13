//! [`qr_code`]: the QR view, a `canvas` child under an `Image`-role wrapper.
//!
//! # Pixel snapping
//!
//! The symbol (modules plus quiet zone on every side) is laid out in a square
//! of side `s`. The module side is `floor(s / (modules + 2 * quiet))` logical
//! px, so every module edge lands on a whole pixel at 1x and the code stays
//! crisp; the leftover `s - module * total` is split evenly (floored) around
//! the symbol and painted in the light colour. Only when `s` is smaller than
//! one pixel per module does the module side fall back to the exact fraction,
//! because a code with zero-width modules cannot scan at all.

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, Point, Role, SemanticsCtx, Size, View, Widget, build_child,
    rebuild_child, route_event_single, teardown_child,
};
use frust::{CanvasView, Color, canvas};

use crate::EccLevel;
use crate::encode::{QrError, QrMatrix, dark_runs, encode};

const DEFAULT_LABEL: &str = "QR code";
const DEFAULT_QUIET: u32 = 4;

/// Create a QR view of `payload`; see the [module docs](self) for sizing.
///
/// An unencodable payload (empty or over capacity) paints only the light
/// background; inspect it with [`QrCodeView::error`].
pub fn qr_code<S: 'static>(payload: impl Into<String>) -> QrCodeView<S> {
    let mut view = QrCodeView {
        payload: payload.into(),
        ecc: EccLevel::M,
        quiet: DEFAULT_QUIET,
        dark: Color::BLACK,
        light: Color::WHITE,
        size: None,
        label: DEFAULT_LABEL.to_string(),
        matrix: None,
        error: None,
        _state: std::marker::PhantomData,
    };
    view.reencode();
    view
}

/// The declarative QR view built by [`qr_code`].
pub struct QrCodeView<S: 'static> {
    payload: String,
    ecc: EccLevel,
    quiet: u32,
    dark: Color,
    light: Color,
    size: Option<f64>,
    label: String,
    matrix: Option<QrMatrix>,
    error: Option<QrError>,
    _state: std::marker::PhantomData<fn(&mut S)>,
}

impl<S: 'static> QrCodeView<S> {
    /// Error-correction level (default [`EccLevel::M`]).
    pub fn ecc(mut self, ecc: EccLevel) -> Self {
        self.ecc = ecc;
        self.reencode();
        self
    }

    /// Quiet zone in modules on every side (default 4, the QR specification's minimum).
    pub fn quiet_zone(mut self, modules: u32) -> Self {
        self.quiet = modules;
        self
    }

    /// Dark and light colours (default black on white). Scanners need contrast
    /// and, for most, dark modules darker than the light ones.
    pub fn colors(mut self, dark: Color, light: Color) -> Self {
        self.dark = dark;
        self.light = light;
        self
    }

    /// Fix the square side in logical px (clamped into the constraints).
    /// Default: the smaller of the available width and height.
    pub fn size(mut self, side: f64) -> Self {
        self.size = Some(side);
        self
    }

    /// The accessible name of the image node (default "QR code").
    pub fn semantic_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Why the payload could not be encoded, if it could not.
    pub fn error(&self) -> Option<&QrError> {
        self.error.as_ref()
    }

    /// Encode once, at construction (and when the ECC level changes); never in paint.
    fn reencode(&mut self) {
        match encode(self.payload.as_bytes(), self.ecc) {
            Ok(m) => (self.matrix, self.error) = (Some(m), None),
            Err(e) => (self.matrix, self.error) = (None, Some(e)),
        }
    }

    fn body(&self) -> CanvasView<S> {
        let matrix = self.matrix.clone();
        let (quiet, dark, light) = (self.quiet, self.dark, self.light);
        canvas(
            move |scene: &mut dyn PaintScene, size: Size, _ctx: &PaintCtx| {
                paint_symbol(scene, size, matrix.as_ref(), quiet, dark, light);
            },
        )
        .repaint_key((
            self.payload.as_str(),
            self.ecc,
            color_key(self.dark),
            color_key(self.light),
            self.quiet,
        ))
    }

    /// Whether `prev` differs in anything the retained widget caches.
    fn layout_changed(&self, prev: &Self) -> bool {
        self.size != prev.size || self.quiet != prev.quiet || self.matrix != prev.matrix
    }
}

fn color_key(c: Color) -> [u32; 4] {
    c.components.map(f32::to_bits)
}

/// Module side and the symbol's offset inside a square of side `side`.
fn grid(side: f64, modules: usize, quiet: u32) -> (f64, f64) {
    let total = (modules + 2 * quiet as usize) as f64;
    let module = (side / total).floor();
    let module = if module >= 1.0 { module } else { side / total };
    let offset = ((side - module * total) / 2.0).floor().max(0.0);
    (module, offset)
}

fn paint_symbol(
    scene: &mut dyn PaintScene,
    size: Size,
    matrix: Option<&QrMatrix>,
    quiet: u32,
    dark: Color,
    light: Color,
) {
    scene.fill_rect(Point::ZERO, size, light);
    let Some(matrix) = matrix else { return };
    let side = size.width.min(size.height);
    let (module, offset) = grid(side, matrix.width(), quiet);
    let origin = offset + module * f64::from(quiet);
    for y in 0..matrix.width() {
        for (start, len) in dark_runs(matrix.row(y)) {
            scene.fill_rect(
                Point::new(origin + module * start as f64, origin + module * y as f64),
                Size::new(module * len as f64, module),
                dark,
            );
        }
    }
}

impl<S: 'static> View<S> for QrCodeView<S> {
    type Element = QrCodeWidget<S>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> QrCodeWidget<S> {
        let body = self.body();
        let pod = build_child(&body, ctx);
        QrCodeWidget {
            body,
            pod,
            side: self.size,
            label: self.label.clone(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut QrCodeWidget<S>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let body = self.body();
        let mut flags = rebuild_child(&element.body, &body, &mut element.pod, ctx);
        element.body = body;
        if element.side != self.size || self.layout_changed(prev) {
            element.side = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.label != self.label {
            element.label = self.label.clone();
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut QrCodeWidget<S>, ctx: &mut BuildCtx<'_>) {
        teardown_child(&element.body, &mut element.pod, ctx);
    }
}

/// The retained widget behind [`QrCodeView`].
pub struct QrCodeWidget<S: 'static> {
    body: CanvasView<S>,
    pod: ChildPod,
    side: Option<f64>,
    label: String,
}

impl<S: 'static> Widget for QrCodeWidget<S> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let max = bc.max();
        let finite = |v: f64| if v.is_finite() { v } else { f64::MAX };
        let available = finite(max.width).min(finite(max.height));
        let side = self.side.unwrap_or(available).min(available);
        let side = if side.is_finite() && side < f64::MAX {
            side
        } else {
            0.0
        };
        let size = bc.constrain(Size::new(side, side));
        self.pod.layout_child(ctx, &BoxConstraints::tight(size));
        self.pod.set_origin(Point::ZERO);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.pod.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event_single(&mut self.pod, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Image,
            |node| node.set_label(self.label.as_str()),
            |ctx| self.pod.semantics_child(ctx),
        );
    }

    frust::authoring::visit_children!(pod);
}

#[cfg(test)]
mod tests {
    use super::grid;

    #[test]
    fn grid_snaps_to_whole_pixels() {
        let (module, offset) = grid(200.0, 21, 4);
        assert_eq!((module, offset), (6.0, 13.0));
        let (module, _) = grid(20.0, 21, 4);
        assert!(module < 1.0 && module > 0.0);
    }
}
