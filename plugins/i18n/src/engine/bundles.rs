//! Per-locale `FluentBundle` construction.
//!
//! One [`Bundle`] is built per registered locale when an
//! [`Engine`](super::Engine) is constructed and is never mutated afterward —
//! see `mod.rs`'s *Concurrency* section for why that lets the engine hand
//! out `&self` resolution with no lock of its own.
//!
//! # Concatenation rule: first definition wins, duplicates are logged
//!
//! A locale directory may hold several `.ftl` files; they are added to one
//! bundle in the order the caller registered them. `fluent-bundle`'s
//! `add_resource` keeps the **earliest** definition of a duplicated message
//! id and reports the collision, so this module keeps that rule (rather
//! than `add_resource_overriding`'s silent last-one-wins) and logs every
//! collision at warn level with the file that lost — a duplicate key is an
//! authoring mistake worth surfacing, but never worth failing an app's
//! whole message set over.
//!
//! # Function seam
//!
//! [`build`] takes a slice of `(name, FluentFunction)` pairs registered into
//! every bundle before its resources, so a formatting layer can plug
//! `NUMBER`/`DATETIME` in without touching this file. Fluent's own
//! `add_builtins()` (which registers a `NUMBER`) is deliberately **not**
//! called here: it would occupy that id and make a later, ICU-backed
//! registration fail as an override.

use std::fmt::Display;
use std::sync::Arc;

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};

use crate::{I18nError, Locale};

/// The bundle flavor this crate builds: `fluent-bundle`'s **concurrent**
/// specialization (a `Mutex`-backed `IntlLangMemoizer`) over reference-counted
/// resources, which is what makes a built bundle `Send + Sync`.
pub type Bundle = FluentBundle<Arc<FluentResource>>;

/// A Fluent function (`{ NUMBER($n) }`) registered into every bundle.
///
/// `Arc<dyn Fn…>` rather than `fluent-bundle`'s own boxed `FluentFunction`
/// (which that crate does not export publicly): the same list is installed
/// into one bundle per locale, so the seam has to hand out clones instead of
/// moving a single boxed value.
pub type FluentFunction =
    Arc<dyn for<'a> Fn(&[FluentValue<'a>], &FluentArgs<'_>) -> FluentValue<'a> + Send + Sync>;

/// A registered locale paired with the bundle holding its messages.
pub struct LocaleBundle {
    /// The locale this bundle speaks.
    pub locale: Locale,
    /// Every message registered for that locale.
    pub bundle: Bundle,
}

/// Builds one locale's bundle from its `(virtual file name, FTL source)`
/// pairs.
///
/// `use_isolating` controls Unicode bidi isolation (FSI/PDI around every
/// interpolated placeable) — see [`LocaleSet::with_isolating`](super::LocaleSet::with_isolating).
///
/// # Errors
///
/// [`I18nError::Format`] if a source fails to parse or a function id is
/// already taken. Parse failure is defense in depth: the `locales!` macro
/// syntax-checks every `.ftl` file at compile time, so a source reaching
/// here malformed means it came from somewhere else.
pub fn build(
    locale: &Locale,
    files: &[(&'static str, &'static str)],
    use_isolating: bool,
    functions: &[(&'static str, FluentFunction)],
) -> Result<Bundle, I18nError> {
    let mut bundle: Bundle = FluentBundle::new_concurrent(vec![super::lang_id(locale)]);
    bundle.set_use_isolating(use_isolating);

    for (name, function) in functions {
        let function = Arc::clone(function);
        bundle
            .add_function(name, move |positional, named| function(positional, named))
            .map_err(|error| {
                I18nError::Format(format!("{locale}: registering function `{name}`: {error}"))
            })?;
    }

    for &(file, source) in files {
        let resource = FluentResource::try_new(source.to_owned()).map_err(|(_, errors)| {
            I18nError::Format(format!("{locale}/{file}: {}", join(&errors)))
        })?;

        // `Err` here is the duplicate-id report, not a failed add: every
        // non-colliding entry of this file is registered either way.
        if let Err(collisions) = bundle.add_resource(Arc::new(resource)) {
            for collision in &collisions {
                log::warn!(
                    "frust-i18n: {locale}/{file}: {collision} — keeping the earlier definition"
                );
            }
        }
    }

    Ok(bundle)
}

/// Renders a list of upstream errors into one `; `-joined line.
pub fn join<E: Display>(errors: &[E]) -> String {
    errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locale(tag: &str) -> Locale {
        tag.parse().expect("valid locale")
    }

    fn format(bundle: &Bundle, key: &str) -> String {
        let message = bundle.get_message(key).expect("message registered");
        let pattern = message.value().expect("message has a value");
        let mut errors = Vec::new();
        let formatted = bundle
            .format_pattern(pattern, None, &mut errors)
            .into_owned();
        assert!(errors.is_empty(), "unexpected format errors: {errors:?}");
        formatted
    }

    #[test]
    fn concatenates_files_first_definition_wins() {
        let bundle = build(
            &locale("en"),
            &[
                ("a.ftl", "shared = from a\nonly-a = A"),
                ("b.ftl", "shared = from b\nonly-b = B"),
            ],
            true,
            &[],
        )
        .expect("bundle builds");

        assert_eq!(format(&bundle, "shared"), "from a");
        assert_eq!(format(&bundle, "only-a"), "A");
        assert_eq!(format(&bundle, "only-b"), "B");
    }

    #[test]
    fn malformed_source_is_a_format_error() {
        // `Bundle` has no `Debug`, so the success arm can't be unwrapped
        // through `expect_err`.
        let Err(error) = build(&locale("en"), &[("broken.ftl", "= no id")], true, &[]) else {
            panic!("malformed FTL must be rejected");
        };

        let rendered = error.to_string();
        assert!(rendered.contains("en/broken.ftl"), "{rendered}");
    }

    #[test]
    fn registered_function_is_callable_from_a_message() {
        let shout: FluentFunction = Arc::new(
            |positional: &[FluentValue<'_>], _named: &FluentArgs<'_>| match positional {
                [FluentValue::String(text)] => FluentValue::from(text.to_uppercase()),
                _ => FluentValue::Error,
            },
        );

        let bundle = build(
            &locale("en"),
            &[("f.ftl", r#"loud = { SHOUT("hi") }"#)],
            false,
            &[("SHOUT", shout)],
        )
        .expect("bundle builds");

        assert_eq!(format(&bundle, "loud"), "HI");
    }

    #[test]
    fn duplicate_function_id_is_a_format_error() {
        let noop: FluentFunction =
            Arc::new(|_positional: &[FluentValue<'_>], _named: &FluentArgs<'_>| FluentValue::None);

        let Err(error) = build(
            &locale("en"),
            &[],
            true,
            &[("SHOUT", Arc::clone(&noop)), ("SHOUT", noop)],
        ) else {
            panic!("a repeated function id must be rejected");
        };

        assert!(error.to_string().contains("SHOUT"), "{error}");
    }
}
