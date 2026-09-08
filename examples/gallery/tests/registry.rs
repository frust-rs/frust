//! Registry-wide sanity checks: every case's slug is unique, every case's
//! `View` constructor actually builds, every `(Design, Variant)` pairing
//! resolves to a theme, and every slug the interactive side table names
//! actually exists in the registry.

use std::collections::HashSet;

use frust_gallery::case::{Design, Variant};

#[test]
fn slugs_are_unique() {
    let mut seen = HashSet::new();
    for case in frust_gallery::cases() {
        assert!(seen.insert(case.slug), "duplicate case slug: {}", case.slug);
    }
}

#[test]
fn every_case_builds() {
    for case in frust_gallery::cases() {
        // A pure, no-runtime `View` construction — calling it must not
        // panic, and it must be reachable through `find` by its own slug.
        let _view = (case.build)();
        assert_eq!(
            frust_gallery::find(case.slug).map(|c| c.slug),
            Some(case.slug)
        );
    }
}

#[test]
fn find_returns_none_for_unknown_slug() {
    assert!(frust_gallery::find("does-not-exist").is_none());
}

// ---- the interactive side table -------------------------------------------
//
// `frust_gallery::interactive` pairs a stateful constructor to a case by SLUG
// STRING rather than by a field on `Case` (see that module's docs for why the
// snapshot oracle demands the separation). A string pairing has exactly one
// failure mode the compiler cannot see: a typo'd or renamed slug registers
// nothing at all, silently, and a live host quietly keeps serving the
// non-interactive `Case::build`. These tests are what close it.

#[test]
fn every_interactive_slug_exists_in_the_registry() {
    for (slug, _) in frust_gallery::interactive::entries() {
        assert!(
            frust_gallery::find(slug).is_some(),
            "interactive table registers slug {slug:?}, which no case in the registry carries \
             — a typo here registers nothing and fails silently at runtime"
        );
    }
}

#[test]
fn interactive_slugs_are_unique() {
    let mut seen = HashSet::new();
    for (slug, _) in frust_gallery::interactive::entries() {
        assert!(
            seen.insert(*slug),
            "duplicate interactive slug: {slug} — `find_interactive` resolves only the first, \
             so the second registration would be dead"
        );
    }
}

#[test]
fn every_interactive_constructor_builds() {
    for (slug, build) in frust_gallery::interactive::entries() {
        // The same no-runtime construction `every_case_builds` performs: a
        // `Component`'s `init` does not run until the view is BUILT, so this
        // stays a pure `View` construction with no owner attached.
        let _view = build();
        assert!(
            frust_gallery::find_interactive(slug).is_some(),
            "interactive constructor for {slug} is not reachable through `find_interactive`"
        );
    }
}

#[test]
fn find_interactive_returns_none_for_a_case_with_no_stateful_constructor() {
    // `radio` is in the registry and deliberately NOT in the side table, so a
    // host must fall back to its own `Case::build`.
    assert!(frust_gallery::find("radio").is_some());
    assert!(frust_gallery::find_interactive("radio").is_none());
    assert!(frust_gallery::find_interactive("does-not-exist").is_none());
}

const ALL_DESIGNS: &[Design] = &[
    Design::Base,
    Design::Material,
    Design::Cupertino,
    Design::Glyph,
    Design::Shadcn,
    Design::Beui,
];

const ALL_VARIANTS: &[Variant] = &[Variant::Light, Variant::Dark];

/// Map a [`Design`] to its slug prefix tag (e.g., `material`, `shadcn`).
fn design_tag(design: Design) -> &'static str {
    match design {
        Design::Base => "base",
        Design::Material => "material",
        Design::Cupertino => "cupertino",
        Design::Glyph => "glyph",
        Design::Shadcn => "shadcn",
        Design::Beui => "beui",
    }
}

#[test]
fn every_design_has_a_theme_for_both_variants() {
    use frust_theme::Brightness;

    for &design in ALL_DESIGNS {
        for &variant in ALL_VARIANTS {
            let theme = frust_gallery::theme(design, variant);
            let expected = match variant {
                Variant::Light => Brightness::Light,
                Variant::Dark => Brightness::Dark,
            };
            assert_eq!(
                theme.brightness, expected,
                "design {design:?} variant {variant:?} did not resolve the requested brightness"
            );
        }
    }
}

#[test]
fn slug_prefix_matches_design_tag() {
    for case in frust_gallery::cases() {
        if let Some((prefix, _stem)) = case.slug.split_once('/') {
            // Prefixed slug: prefix must match the design's tag
            let expected_tag = design_tag(case.design);
            assert_eq!(
                prefix, expected_tag,
                "case slug {}: prefix '{}' does not match design tag '{}'",
                case.slug, prefix, expected_tag
            );
        } else {
            // Un-prefixed slug: must be Design::Base
            assert_eq!(
                case.design,
                Design::Base,
                "case slug {}: un-prefixed slug must have Design::Base, found {:?}",
                case.slug,
                case.design
            );
        }
    }
}
