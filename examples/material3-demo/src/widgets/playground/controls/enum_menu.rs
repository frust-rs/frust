//! Discrete enum control via a dropdown menu — the reference's
//! `PlayEnumMenu`, the better fit once a segmented button would have too
//! many options.
//!
//! Two pieces, not one — see [`crate::widgets::playground`]'s module docs
//! for why: [`play_enum_menu_field`] is the trigger a control panel mounts
//! in its row list; [`play_enum_menu_panel`] is the popup a page mounts at
//! its own outer `Stack`, anchored to the field through a shared
//! `frust_material::OverlayAnchor` the page holds in its knob state.
//!
//! The reference's `_PlayEnumMenuState` exists only to swallow a
//! Flutter-specific selection echo during `didUpdateWidget` sync — there is
//! no analogous echo here (`frust_material::dropdown`'s panel never fires
//! `on_change` from `rebuild`), so nothing here needs the deferred
//! post-frame guard it applies.

use frust::{AnyView, Column, CrossAxisAlignment, EdgeInsets, Padding, SizedBox, any, text};
use frust_material::{
    DropdownItem, MaterialSpacing, OverlayAnchor, dropdown, dropdown_field, dropdown_item,
};

use crate::widgets::playground::ambient_theme;

/// The dropdown item list `values` resolves to, keyed by `label_of` (so a
/// label must be unique across `values` for a round trip through the panel's
/// `on_change` to resolve back to the right `T`).
fn menu_items<T: Copy>(values: &[T], label_of: fn(T) -> &'static str) -> Vec<DropdownItem> {
    values
        .iter()
        .map(|v| dropdown_item(label_of(*v), label_of(*v)))
        .collect()
}

/// The `T` a reported dropdown selection names, or `None` if it names
/// nothing in `values` — factored out of [`play_enum_menu_panel`]'s
/// callback so it is unit-testable without a widget tree.
fn resolve_selection<T: PartialEq + Copy>(
    values: &[T],
    label_of: fn(T) -> &'static str,
    selection: &[String],
) -> Option<T> {
    let picked = selection.first()?;
    values
        .iter()
        .copied()
        .find(|v| label_of(*v) == picked.as_str())
}

/// The trigger half: a labeled field showing `value`'s label, requesting
/// [`play_enum_menu_panel`] open/closed through `on_open`.
pub fn play_enum_menu_field<State: 'static, T: PartialEq + Copy + 'static>(
    label: impl Into<String>,
    value: T,
    values: &[T],
    label_of: fn(T) -> &'static str,
    anchor: &OverlayAnchor,
    open: bool,
    on_open: impl Fn(&mut State, bool) + 'static,
) -> AnyView<State> {
    let label = label.into();
    let theme = ambient_theme();
    let mut body = theme.type_scale.body_large.clone();
    body.color = theme.scheme().on_surface;

    let items = menu_items(values, label_of);
    let selected = vec![label_of(value).to_string()];

    any(Padding(
        EdgeInsets {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: MaterialSpacing::MD,
        },
        Column(vec![
            any(text(label.clone()).style(body)),
            any(SizedBox::<State>(None, Some(MaterialSpacing::SM))),
            any(dropdown_field(items)
                .anchor(anchor)
                .selected(selected)
                .hint(label)
                .open(open)
                .on_open(on_open)),
        ])
        .cross_axis(CrossAxisAlignment::Stretch),
    ))
}

/// The popup half: the option panel, anchored to whichever
/// [`play_enum_menu_field`] shares its `anchor`. Mount at the page's own
/// outer `Stack`, kept mounted (per `frust_material::dropdown`'s own
/// kept-mounted pattern) so a close plays its exit ramp.
pub fn play_enum_menu_panel<State: 'static, T: PartialEq + Copy + 'static>(
    value: T,
    values: &[T],
    label_of: fn(T) -> &'static str,
    anchor: &OverlayAnchor,
    open: bool,
    on_open: impl Fn(&mut State, bool) + 'static,
    on_changed: impl Fn(&mut State, T) + 'static,
) -> AnyView<State> {
    let items = menu_items(values, label_of);
    let selected = vec![label_of(value).to_string()];
    let values: Vec<T> = values.to_vec();

    any(
        dropdown(items, move |state: &mut State, selection: Vec<String>| {
            if let Some(picked) = resolve_selection(&values, label_of, &selection) {
                on_changed(state, picked);
            }
        })
        .anchor(anchor)
        .selected(selected)
        .open(open)
        .on_open(on_open),
    )
}

#[cfg(test)]
mod tests {
    use frust_material::OverlayAnchor;

    use super::{menu_items, play_enum_menu_field, play_enum_menu_panel, resolve_selection};

    struct TestState(Choice);

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Choice {
        A,
        B,
        C,
    }

    impl Choice {
        fn label(self) -> &'static str {
            match self {
                Choice::A => "A",
                Choice::B => "B",
                Choice::C => "C",
            }
        }
    }

    const VALUES: [Choice; 3] = [Choice::A, Choice::B, Choice::C];

    #[test]
    fn items_are_keyed_by_label() {
        let items = menu_items(&VALUES, Choice::label);
        assert_eq!(items.len(), 3);
        assert_eq!(items[1].label(), "B");
        assert_eq!(items[1].value(), "B");
    }

    #[test]
    fn resolves_a_reported_label_back_to_its_value() {
        assert_eq!(
            resolve_selection(&VALUES, Choice::label, &["B".to_string()]),
            Some(Choice::B)
        );
    }

    #[test]
    fn an_unknown_label_resolves_to_nothing() {
        assert_eq!(
            resolve_selection(&VALUES, Choice::label, &["Z".to_string()]),
            None
        );
    }

    #[test]
    fn an_empty_selection_resolves_to_nothing() {
        assert_eq!(resolve_selection(&VALUES, Choice::label, &[]), None);
    }

    #[test]
    fn field_and_panel_build_off_the_same_anchor() {
        let anchor = OverlayAnchor::new();
        let _field = play_enum_menu_field::<TestState, Choice>(
            "Choice",
            Choice::A,
            &VALUES,
            Choice::label,
            &anchor,
            false,
            |_state: &mut TestState, _open: bool| {},
        );
        let _panel = play_enum_menu_panel::<TestState, Choice>(
            Choice::A,
            &VALUES,
            Choice::label,
            &anchor,
            false,
            |_state: &mut TestState, _open: bool| {},
            |state: &mut TestState, next: Choice| state.0 = next,
        );
    }
}
