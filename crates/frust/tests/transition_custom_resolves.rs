//! Proof that a custom page transition is authorable with `frust` as the only
//! dependency: `PageTransition::Custom` returns `Layer`s, and both `Layer` and
//! `resolve_layers` resolve through the facade.

use frust::kurbo::Size;
use frust::{Layer, PageTransition, TransitionSpec, resolve_layers};

fn slide_fade(progress: f64, is_pop: bool, _size: Size) -> (Layer, Layer) {
    let dir = if is_pop { -1.0 } else { 1.0 };
    let entering = Layer {
        dx: dir * 24.0 * (1.0 - progress),
        alpha: progress as f32,
        ..Layer::IDENTITY
    };
    let leaving = Layer {
        dx: -dir * 24.0 * progress,
        alpha: (1.0 - progress) as f32,
        ..Layer::IDENTITY
    };
    (entering, leaving)
}

#[test]
fn identity_layer_fields() {
    let l = Layer::IDENTITY;
    assert_eq!((l.dx, l.dy, l.alpha, l.scale), (0.0, 0.0, 1.0, 1.0));
}

#[test]
fn none_preset_resolves_to_identity() {
    let size = Size::new(200.0, 100.0);
    assert_eq!(
        resolve_layers(PageTransition::None, 0.5, false, size),
        (Layer::IDENTITY, Layer::IDENTITY)
    );
}

#[test]
fn custom_transition_endpoints() {
    let size = Size::new(200.0, 100.0);
    let spec = TransitionSpec::duration(PageTransition::Custom(slide_fade));
    let (entering0, leaving0) = resolve_layers(spec.preset, 0.0, false, size);
    assert_eq!(
        entering0,
        Layer {
            dx: 24.0,
            alpha: 0.0,
            ..Layer::IDENTITY
        }
    );
    assert_eq!(leaving0, Layer::IDENTITY);
    let (entering1, leaving1) = resolve_layers(spec.preset, 1.0, false, size);
    assert_eq!(entering1, Layer::IDENTITY);
    assert_eq!(
        leaving1,
        Layer {
            dx: -24.0,
            alpha: 0.0,
            ..Layer::IDENTITY
        }
    );
}
