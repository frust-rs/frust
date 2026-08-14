//! `frust-i18n-macros`: the `locales!` compile-time Fluent-bundle loader for
//! `frust-i18n`.
//!
//! # Charter
//!
//! A plain proc-macro crate, split from `frust-i18n` only because
//! `proc-macro = true` crates cannot also export ordinary items — the
//! mechanical reason a `<crate>-macros` sibling exists, not a layering
//! choice. `frust-i18n` re-exports [`locales!`](macro@locales) at its own
//! crate root (`frust_i18n::locales`), so a caller never depends on this
//! crate directly, and every path the expansion names is spelled
//! `::frust_i18n::…` for the same reason.
//!
//! # Layout
//!
//! [`parse`] discovers the locale tree, reads it, and syntax-checks it;
//! [`typed`] turns the fallback locale's messages into the generated `keys`
//! module. This file owns the macro's argument grammar, the fallback-locale
//! rule, and the `LocaleSet`/`Engine` registration the two feed.

mod parse;
mod typed;

use std::path::Path;

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Ident, LitStr, Token};
use unic_langid::LanguageIdentifier;

use parse::LocaleDir;

/// Loads a directory of per-locale Fluent bundles at compile time.
///
/// ```ignore
/// frust_i18n::locales!("locales");
/// frust_i18n::locales!("locales", fallback: "en-GB");
/// ```
///
/// The argument is a directory path relative to the invoking crate's
/// `CARGO_MANIFEST_DIR` holding one subdirectory per locale, each named by
/// its BCP-47 identifier (`en`, `en-US`, `zh-Hans-CN`) and holding one or
/// more `.ftl` files (nested directories are walked; everything that is not
/// a `.ftl` file, and every name starting with `.`, is ignored).
///
/// # What it expands to
///
/// Three items, in the module the macro is invoked from — so one invocation
/// per module:
///
/// - `locale_set()` — a `frust_i18n::LocaleSet` naming every locale and
///   embedding every `.ftl` source through `include_str!`. Build an engine
///   from it directly when the app needs to register Fluent functions or
///   switch bidi isolation off.
/// - `engine()` — the same set, built once into a shared
///   `&'static frust_i18n::Engine`.
/// - `keys` — one function per message of the fallback locale, taking the
///   resolver to format through plus one argument per `$variable` the
///   message references. A typo'd key is a missing function, i.e. a
///   compile error; a resolution failure at runtime logs a warning and
///   renders the key itself.
///
/// # Compile-time validation
///
/// Every `.ftl` file is parsed while the macro expands: a malformed message
/// fails compilation naming the file, line, column, and the parser's own
/// message. A directory name that is not a well-formed BCP-47 identifier
/// fails the same way, as does a tree with no locale directory at all.
///
/// # Fallback locale
///
/// `fallback:` names the locale that closes every negotiated chain and whose
/// message set defines the typed `keys` surface. It defaults to `en` when a
/// matching directory exists; with no `en` directory and no explicit
/// argument, expansion fails asking for one. Messages a non-fallback locale
/// is missing stay fallback-served at runtime and are reported as a
/// warning printed at expansion time (proc-macro output reaches the build
/// log; an unchanged, cached expansion does not re-print it).
#[proc_macro]
pub fn locales(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        return syn::Error::new(
            Span::call_site(),
            "`locales!`: CARGO_MANIFEST_DIR is unset — the locales directory is resolved \
             relative to the invoking crate, so the macro cannot run outside a cargo build",
        )
        .to_compile_error()
        .into();
    };

    let expansion = expand(input.into(), Path::new(&manifest_dir));
    for warning in &expansion.warnings {
        eprintln!("warning: frust-i18n: locales!: {warning}");
    }
    expansion.tokens.into()
}

/// A macro expansion: the tokens it produced plus the non-fatal notes worth
/// printing.
///
/// Kept separate from the token stream so the warning path is a value this
/// crate's own tests can assert on rather than terminal output only a human
/// ever sees.
struct Expansion {
    tokens: TokenStream,
    warnings: Vec<String>,
}

impl From<syn::Error> for Expansion {
    fn from(error: syn::Error) -> Self {
        Self {
            tokens: error.to_compile_error(),
            warnings: Vec::new(),
        }
    }
}

/// The macro's parsed argument list.
struct Args {
    /// The locale-tree path, relative to `CARGO_MANIFEST_DIR`.
    path: LitStr,
    /// The explicit `fallback:` tag, if the caller named one.
    fallback: Option<LitStr>,
}

impl Parse for Args {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let path: LitStr = input.parse()?;
        let mut fallback = None;

        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let option: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            let value: LitStr = input.parse()?;
            if option != "fallback" {
                return Err(syn::Error::new(
                    option.span(),
                    format!(
                        "`locales!`: unknown option `{option}` — the only option is \
                         `fallback: \"<bcp47 tag>\"`"
                    ),
                ));
            }
            if fallback.is_some() {
                return Err(syn::Error::new(
                    option.span(),
                    "`locales!`: `fallback` is given twice",
                ));
            }
            fallback = Some(value);
        }

        Ok(Args { path, fallback })
    }
}

/// Expands the macro against `manifest_dir`, the directory the path
/// argument is resolved relative to.
///
/// Split from the `proc_macro` entry point above so both the generated
/// tokens and the warnings are testable without a cargo build around them.
fn expand(input: TokenStream, manifest_dir: &Path) -> Expansion {
    let args = match syn::parse2::<Args>(input) {
        Ok(args) => args,
        Err(error) => return error.into(),
    };
    match build(&args, manifest_dir) {
        Ok(expansion) => expansion,
        Err(error) => error.into(),
    }
}

/// Reads, validates, and generates — the macro's whole body.
fn build(args: &Args, manifest_dir: &Path) -> Result<Expansion, syn::Error> {
    let span = args.path.span();
    let rel = args.path.value();
    let rel = rel.trim_end_matches('/');
    let locales = parse::discover(&manifest_dir.join(rel), rel, span)?;

    let mut parsed = Vec::with_capacity(locales.len());
    let mut failure = None;
    for locale in &locales {
        match parse::check(locale, span) {
            Ok(files) => parsed.push(files),
            Err(error) => parse::combine(&mut failure, error),
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }

    let fallback = fallback_index(args, &locales, rel, span)?;
    let messages = typed::messages(&parsed[fallback]);
    let keys = typed::keys_module(&messages, span)?;

    let mut warnings = Vec::new();
    for (index, locale) in locales.iter().enumerate() {
        if locale.files.is_empty() {
            warnings.push(format!(
                "`{rel}/{}` holds no `.ftl` file — the locale registers no messages",
                locale.tag
            ));
        }
        if index != fallback
            && let Some(report) = typed::missing(
                &locale.tag,
                &locales[fallback].tag,
                &typed::ids(&parsed[index]),
                &messages,
            )
        {
            warnings.push(report);
        }
    }

    let registration = registration(&locales[fallback].tag, &locales, span);
    Ok(Expansion {
        tokens: quote! { #registration #keys },
        warnings,
    })
}

/// Resolves which discovered locale is the fallback.
///
/// An explicit `fallback:` must name a directory that exists — a fallback
/// with no messages at all is what the engine falls through *to*, so a typo
/// there would silently empty every chain. With no explicit argument the
/// rule is `en`-if-present, and otherwise a compile error demanding one
/// rather than a guess at which of several locales is authoritative.
fn fallback_index(
    args: &Args,
    locales: &[LocaleDir],
    rel: &str,
    span: Span,
) -> Result<usize, syn::Error> {
    /// The tag assumed when the caller names none.
    const DEFAULT: &str = "en";

    let (tag, explicit) = match &args.fallback {
        Some(fallback) => (fallback.value(), true),
        None => (DEFAULT.to_owned(), false),
    };
    let wanted: LanguageIdentifier = tag.parse().map_err(|error| {
        syn::Error::new(
            args.fallback.as_ref().map_or(span, LitStr::span),
            format!("`locales!`: `{tag}` is not a BCP-47 locale identifier: {error}"),
        )
    })?;

    let found = locales
        .iter()
        .position(|locale| locale.tag.parse::<LanguageIdentifier>().ok() == Some(wanted.clone()));
    match (found, explicit) {
        (Some(index), _) => Ok(index),
        (None, true) => Err(syn::Error::new(
            args.fallback.as_ref().map_or(span, LitStr::span),
            format!("`locales!`: no `{rel}/{tag}` directory for the declared fallback locale"),
        )),
        (None, false) => Err(syn::Error::new(
            span,
            format!(
                "`locales!`: no `{rel}/{DEFAULT}` directory, so there is no default fallback \
                 locale — name one explicitly: locales!(\"{rel}\", fallback: \"{}\")",
                locales[0].tag
            ),
        )),
    }
}

/// Generates the `LocaleSet`/`Engine` registration.
///
/// Sources are embedded by `include_str!` rather than pasted in as literals
/// (see [`parse`]'s *Rebuild tracking* note), and the tags are re-parsed at
/// runtime from strings this macro already validated — hence the `expect`s,
/// which are app-side and unreachable rather than an unchecked FFI-adjacent
/// panic.
fn registration(fallback: &str, locales: &[LocaleDir], span: Span) -> TokenStream {
    /// The `expect` message every generated tag parse carries.
    const VALIDATED: &str = "`locales!` validated this locale tag at compile time";

    let fallback = LitStr::new(fallback, span);
    let entries = locales.iter().map(|locale| {
        let tag = LitStr::new(&locale.tag, span);
        let files = locale.files.iter().map(|file| file.embed(span));
        quote! {
            .with_locale(#tag.parse().expect(#VALIDATED), [#(#files),*])
        }
    });

    quote! {
        /// The compiled-in message set: every locale directory found at
        /// build time, each source embedded through `include_str!`.
        ///
        /// Build an `Engine` from this directly to register Fluent
        /// functions or switch bidi isolation off; `engine()` below is the
        /// shared, unconfigured build of it.
        #[allow(dead_code)]
        pub fn locale_set() -> ::frust_i18n::LocaleSet {
            ::frust_i18n::LocaleSet::new(#fallback.parse().expect(#VALIDATED))
                #(#entries)*
        }

        /// The process-wide engine built from `locale_set()`, on first use.
        #[allow(dead_code)]
        pub fn engine() -> &'static ::frust_i18n::Engine {
            static ENGINE: ::std::sync::OnceLock<::frust_i18n::Engine> =
                ::std::sync::OnceLock::new();
            ENGINE.get_or_init(|| {
                ::frust_i18n::Engine::new(locale_set())
                    .expect("`locales!` validated every embedded source at compile time")
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `frust-i18n` crate directory — the manifest dir a caller's
    /// invocation resolves against, here pointed at this crate's parent so
    /// the fixture trees under `plugins/i18n/tests/fixtures` are shared with
    /// the compile-fail suite rather than duplicated.
    fn manifest_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
    }

    fn expand_str(input: &str) -> Expansion {
        expand(
            input.parse().expect("test input parses as tokens"),
            &manifest_dir(),
        )
    }

    fn rendered(input: &str) -> String {
        expand_str(input).tokens.to_string()
    }

    #[test]
    fn a_valid_tree_registers_every_locale_and_source() {
        let output = rendered(r#""tests/fixtures/valid""#);

        assert!(!output.contains("compile_error"), "{output}");
        assert!(output.contains("\"de\" . parse ()"), "{output}");
        assert!(output.contains("\"en\" . parse ()"), "{output}");
        assert!(output.contains("\"app.ftl\""), "{output}");
    }

    #[test]
    fn every_consumed_file_is_embedded_by_include_str() {
        let output = rendered(r#""tests/fixtures/valid""#);

        // Rebuild tracking: two locales, one `.ftl` file each, so editing
        // either re-triggers compilation and re-runs the syntax check.
        assert_eq!(output.matches("include_str !").count(), 2, "{output}");
        assert!(
            output.contains("tests/fixtures/valid/en/app.ftl"),
            "{output}"
        );
    }

    #[test]
    fn typed_keys_are_generated_from_the_fallback_locale() {
        let output = rendered(r#""tests/fixtures/valid""#);

        assert!(output.contains("pub mod keys"), "{output}");
        assert!(output.contains("pub fn greeting"), "{output}");
        assert!(output.contains("pub fn cart_items"), "{output}");
        // `only-en` lives in the fallback alone and is still typed.
        assert!(output.contains("pub fn only_en"), "{output}");
    }

    #[test]
    fn a_malformed_file_names_its_path_and_position() {
        let output = rendered(r#""tests/fixtures/malformed""#);

        assert!(output.contains("compile_error"), "{output}");
        assert!(
            output.contains("tests/fixtures/malformed/en/broken.ftl:3:2"),
            "{output}"
        );
    }

    #[test]
    fn a_non_bcp47_directory_name_is_a_compile_error() {
        let output = rendered(r#""tests/fixtures/bad-locale-name""#);

        assert!(output.contains("compile_error"), "{output}");
        assert!(
            output.contains("is not a BCP-47 locale identifier"),
            "{output}"
        );
        assert!(output.contains("en_US.UTF-8"), "{output}");
    }

    #[test]
    fn a_missing_default_fallback_demands_an_explicit_one() {
        let output = rendered(r#""tests/fixtures/no-fallback""#);

        assert!(output.contains("compile_error"), "{output}");
        assert!(output.contains("fallback"), "{output}");
    }

    #[test]
    fn an_explicit_fallback_selects_its_locale() {
        let output = rendered(r#""tests/fixtures/no-fallback", fallback: "de""#);

        assert!(!output.contains("compile_error"), "{output}");
        assert!(
            output.contains("LocaleSet :: new (\"de\" . parse ()"),
            "{output}"
        );
    }

    #[test]
    fn an_explicit_fallback_with_no_directory_is_a_compile_error() {
        let output = rendered(r#""tests/fixtures/valid", fallback: "fr""#);

        assert!(output.contains("compile_error"), "{output}");
        assert!(output.contains("fr"), "{output}");
    }

    #[test]
    fn a_missing_tree_names_the_path_the_caller_wrote() {
        let output = rendered(r#""tests/fixtures/does-not-exist""#);

        assert!(output.contains("compile_error"), "{output}");
        assert!(output.contains("tests/fixtures/does-not-exist"), "{output}");
    }

    #[test]
    fn an_unknown_option_is_rejected() {
        let output = rendered(r#""tests/fixtures/valid", isolating: "false""#);

        assert!(output.contains("compile_error"), "{output}");
        assert!(output.contains("isolating"), "{output}");
    }

    #[test]
    fn a_half_translated_locale_is_reported_as_a_warning() {
        let warnings = expand_str(r#""tests/fixtures/valid""#).warnings;

        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("`de` is missing"), "{warnings:?}");
        assert!(warnings[0].contains("only-en"), "{warnings:?}");
    }
}
