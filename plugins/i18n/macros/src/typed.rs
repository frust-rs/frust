//! The generated `keys` module: one function per message, arguments typed
//! from the message's own `$variable` references.
//!
//! # Why the fallback locale defines the surface
//!
//! The typed surface is built from the **fallback** locale's messages, which
//! is the same set the engine's chain walk always ends at: a message only a
//! translation has is unreachable by that chain for anyone whose negotiated
//! chain doesn't include that translation, while a message only the fallback
//! has still resolves for everyone. A message missing from a non-fallback
//! locale is therefore not an error — it is fallback-served at runtime — but
//! it is reported by [`missing`] so a half-translated catalog is visible at
//! build time.
//!
//! # What a generated function is
//!
//! A thin, compile-checked call into the dynamic resolver: it packs its
//! arguments into `FluentArgs` and hands them to
//! `frust_i18n::Resolve::resolve_message`, so
//! there is exactly one resolution path in the crate, not two. Failure is
//! softened the same way the dynamic sugar softens it — a warn-level log
//! plus the key itself as the rendered string — because a missing
//! translation must never take a screen down.
//!
//! # Boundaries of the typed surface
//!
//! - **Message values only.** A message with attributes but no value
//!   (`save =` + `.tooltip = …`) contributes no function, and no function is
//!   generated *for* an attribute; attributes stay reachable through the
//!   dynamic `key.attribute` lookup.
//! - **Named arguments from two variables up.** Zero or one variable keeps a
//!   plain signature; two or more take a generated `<PascalKey>Args` struct
//!   (one generic field per variable), so a swapped pair is a compile error
//!   rather than a silently wrong string.
//! - **Direct variable references only.** Variables a message reaches
//!   indirectly, through a message reference (`{ other-message }`), are not
//!   part of its signature — Fluent's own tooling doesn't chase them either,
//!   and a term's parameters are supplied by the message, not the caller.

use std::collections::{BTreeMap, BTreeSet};

use fluent_syntax::ast;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

use crate::parse::combine;

/// The Rust keywords a snake-cased Fluent id can collide with.
///
/// A raw identifier (`r#type`) would keep the name exact but forces every
/// call site to spell it that way; a trailing underscore is one rule that
/// covers every keyword, including the four (`crate`, `self`, `super`,
/// `Self`) that cannot be raw at all.
const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
    "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl",
    "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof",
    "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

/// One message the typed surface covers.
pub struct Message {
    /// The Fluent message id, as written in the `.ftl` file.
    pub key: String,
    /// The `$variables` its value references, sorted and de-duplicated.
    pub vars: Vec<String>,
}

/// Collects a locale's messages in registration order, first definition
/// winning.
///
/// That is deliberately the same rule the engine's bundle concatenation
/// follows (`fluent-bundle`'s `add_resource` keeps the earliest definition
/// of a duplicated id), so the generated signature always describes the
/// message that will actually resolve.
pub fn messages(files: &[ast::Resource<&str>]) -> Vec<Message> {
    let mut seen = BTreeMap::new();
    for file in files {
        for entry in &file.body {
            let ast::Entry::Message(message) = entry else {
                continue;
            };
            let Some(value) = &message.value else {
                continue;
            };
            if seen.contains_key(message.id.name) {
                continue;
            }
            let mut vars = BTreeSet::new();
            pattern_vars(value, &mut vars);
            seen.insert(
                message.id.name,
                Message {
                    key: message.id.name.to_owned(),
                    vars: vars.into_iter().map(ToOwned::to_owned).collect(),
                },
            );
        }
    }
    seen.into_values().collect()
}

/// The message ids one locale defines a value for.
pub fn ids(files: &[ast::Resource<&str>]) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for file in files {
        for entry in &file.body {
            if let ast::Entry::Message(message) = entry
                && message.value.is_some()
            {
                ids.insert(message.id.name.to_owned());
            }
        }
    }
    ids
}

/// Reports the fallback-defined messages `tag` does not have, capped so a
/// wholly untranslated locale prints a line rather than a wall.
///
/// Returns `None` when the locale is complete.
pub fn missing(
    tag: &str,
    fallback: &str,
    defined: &BTreeSet<String>,
    all: &[Message],
) -> Option<String> {
    /// How many missing keys a single report names before summarizing.
    const LISTED: usize = 8;

    let missing: Vec<&str> = all
        .iter()
        .map(|message| message.key.as_str())
        .filter(|key| !defined.contains(*key))
        .collect();
    if missing.is_empty() {
        return None;
    }

    let listed = missing
        .iter()
        .take(LISTED)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    let rest = missing.len().saturating_sub(LISTED);
    let tail = if rest == 0 {
        String::new()
    } else {
        format!(" (and {rest} more)")
    };
    Some(format!(
        "`{tag}` is missing {} of `{fallback}`'s messages, which stay \
         fallback-served: {listed}{tail}",
        missing.len()
    ))
}

/// Builds the `keys` module from the fallback locale's messages.
///
/// # Errors
///
/// A [`syn::Error`] when two message ids, or two of one message's variables,
/// snake-case onto the same Rust identifier — the generated code would
/// otherwise fail to compile with an error pointing at expanded source
/// nobody wrote.
pub fn keys_module(messages: &[Message], span: Span) -> Result<TokenStream, syn::Error> {
    let mut taken: BTreeMap<String, &str> = BTreeMap::new();
    let mut args_taken: BTreeMap<String, &str> = BTreeMap::new();
    let mut failure: Option<syn::Error> = None;
    let mut functions = Vec::with_capacity(messages.len());

    for message in messages {
        let name = ident_for(&message.key);
        if let Some(previous) = taken.insert(name.clone(), &message.key) {
            combine(
                &mut failure,
                syn::Error::new(
                    span,
                    format!(
                        "`locales!`: messages `{previous}` and `{}` both name the generated \
                         function `keys::{name}` — rename one",
                        message.key
                    ),
                ),
            );
            continue;
        }
        let args_name = pascal_for(&name);
        if message.vars.len() > 1
            && let Some(previous) = args_taken.insert(args_name.clone(), &message.key)
        {
            combine(
                &mut failure,
                syn::Error::new(
                    span,
                    format!(
                        "`locales!`: messages `{previous}` and `{}` both name the generated \
                         struct `keys::{args_name}Args` — rename one",
                        message.key
                    ),
                ),
            );
            continue;
        }
        match function(message, &name, span) {
            Ok(tokens) => functions.push(tokens),
            Err(error) => combine(&mut failure, error),
        }
    }

    if let Some(error) = failure {
        return Err(error);
    }

    Ok(quote! {
        /// Compile-checked message keys, one function per message of the
        /// fallback locale.
        ///
        /// Each takes the resolver to format through — an
        /// `Engine::with_chain` pairing, or any other `Resolve`
        /// implementation — and returns the formatted string. A message
        /// with one `$variable` takes it as a second parameter; a message
        /// with two or more takes a single generated `<Key>Args` struct
        /// with one named field per variable, so two values of the same
        /// type cannot be transposed at a call site. A message that
        /// cannot be resolved logs a warning and renders as its own key,
        /// so a translation gap degrades a label rather than a screen.
        pub mod keys {
            #(#functions)*

            /// The single resolution path every generated function takes.
            #[doc(hidden)]
            #[allow(dead_code)]
            fn __frust_resolve_or_key(
                resolver: &impl ::frust_i18n::Resolve,
                key: &str,
                args: ::core::option::Option<
                    &::frust_i18n::__private::fluent_bundle::FluentArgs<'_>,
                >,
            ) -> ::std::string::String {
                match ::frust_i18n::Resolve::resolve_message(resolver, key, args) {
                    ::core::result::Result::Ok(text) => text,
                    ::core::result::Result::Err(error) => {
                        ::frust_i18n::__private::log::warn!("frust-i18n: `{key}`: {error}");
                        ::std::string::ToString::to_string(key)
                    }
                }
            }
        }
    })
}

/// Builds one message's function.
fn function(message: &Message, name: &str, span: Span) -> Result<TokenStream, syn::Error> {
    let key = &message.key;
    let ident = Ident::new(name, span);
    let doc = format!("Formats the `{key}` message.");

    // A Fluent identifier always starts with a letter, so the snake-casing of
    // a `$variable` can never start with an underscore. Every name the
    // generated body introduces itself — the `_resolver` parameter, the
    // `__frust_args` local, the `__frust_arguments` struct parameter and the
    // `__frust_resolve_or_key` helper — starts with one, which is what makes
    // them collision-proof against a `$variable` of any name.
    let mut fields = Vec::with_capacity(message.vars.len());
    let mut taken: BTreeMap<String, &str> = BTreeMap::new();
    for var in &message.vars {
        let name = ident_for(var);
        if let Some(previous) = taken.insert(name.clone(), var) {
            return Err(syn::Error::new(
                span,
                format!(
                    "`locales!`: message `{key}` references both `${previous}` and `${var}`, \
                     which name the same generated parameter `{name}` — rename one"
                ),
            ));
        }
        fields.push((Ident::new(&name, span), var.as_str()));
    }

    if fields.is_empty() {
        return Ok(quote! {
            #[doc = #doc]
            #[allow(dead_code)]
            pub fn #ident(_resolver: &impl ::frust_i18n::Resolve) -> ::std::string::String {
                __frust_resolve_or_key(_resolver, #key, ::core::option::Option::None)
            }
        });
    }

    let value = quote! {
        ::core::convert::Into<::frust_i18n::__private::fluent_bundle::FluentValue<'a>>
    };

    if let [(param, var)] = fields.as_slice() {
        return Ok(quote! {
            #[doc = #doc]
            #[allow(dead_code)]
            pub fn #ident<'a>(
                _resolver: &impl ::frust_i18n::Resolve,
                #param: impl #value,
            ) -> ::std::string::String {
                let mut __frust_args = ::frust_i18n::__private::fluent_bundle::FluentArgs::new();
                __frust_args.set(#var, #param);
                __frust_resolve_or_key(_resolver, #key, ::core::option::Option::Some(&__frust_args))
            }
        });
    }

    // Two or more variables: a struct with one named field each, so a call
    // site cannot transpose two values of the same type.
    let args_ident = Ident::new(&format!("{}Args", pascal_for(name)), span);
    let args_doc = format!("The named arguments of the `{key}` message.");
    let generics: Vec<Ident> = (0..fields.len())
        .map(|index| Ident::new(&format!("T{index}"), span))
        .collect();
    let field_defs = fields.iter().zip(&generics).map(|((field, var), generic)| {
        let field_doc = format!("The `${var}` value.");
        quote! {
            #[doc = #field_doc]
            pub #field: #generic
        }
    });
    let bounds = generics.iter().map(|generic| quote! { #generic: #value });
    let sets = fields.iter().map(|(field, var)| {
        quote! { __frust_args.set(#var, __frust_arguments.#field); }
    });

    Ok(quote! {
        #[doc = #args_doc]
        #[allow(dead_code)]
        pub struct #args_ident<#(#generics),*> {
            #(#field_defs,)*
        }

        #[doc = #doc]
        #[allow(dead_code)]
        pub fn #ident<'a, #(#bounds),*>(
            _resolver: &impl ::frust_i18n::Resolve,
            __frust_arguments: #args_ident<#(#generics),*>,
        ) -> ::std::string::String {
            let mut __frust_args = ::frust_i18n::__private::fluent_bundle::FluentArgs::new();
            #(#sets)*
            __frust_resolve_or_key(_resolver, #key, ::core::option::Option::Some(&__frust_args))
        }
    })
}

/// Pascal-cases an already snake-cased identifier (`editor_notice` becomes
/// `EditorNotice`; a keyword's trailing underscore is dropped).
fn pascal_for(snake: &str) -> String {
    snake
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect()
}

/// Snake-cases a Fluent identifier into a Rust one.
///
/// `cart-items` and `cartItems` both become `cart_items`; a name that lands
/// on a Rust keyword takes a trailing underscore (see [`KEYWORDS`]). Two ids
/// that collapse onto the same identifier are a compile error, not a silent
/// pick.
fn ident_for(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    let mut name = String::with_capacity(id.len());
    for (index, &current) in chars.iter().enumerate() {
        if current == '-' || current == '_' || current == '.' {
            if !name.ends_with('_') && !name.is_empty() {
                name.push('_');
            }
            continue;
        }
        if current.is_uppercase() {
            let after_lower =
                index > 0 && (chars[index - 1].is_lowercase() || chars[index - 1].is_numeric());
            let before_lower = chars.get(index + 1).is_some_and(|next| next.is_lowercase());
            if (after_lower || (index > 0 && chars[index - 1].is_uppercase() && before_lower))
                && !name.ends_with('_')
                && !name.is_empty()
            {
                name.push('_');
            }
        }
        name.extend(current.to_lowercase());
    }

    if KEYWORDS.contains(&name.as_str()) {
        name.push('_');
    }
    name
}

/// Collects every `$variable` a pattern references directly.
fn pattern_vars<'a>(pattern: &ast::Pattern<&'a str>, out: &mut BTreeSet<&'a str>) {
    for element in &pattern.elements {
        if let ast::PatternElement::Placeable { expression } = element {
            expression_vars(expression, out);
        }
    }
}

/// Collects every `$variable` an expression references directly.
fn expression_vars<'a>(expression: &ast::Expression<&'a str>, out: &mut BTreeSet<&'a str>) {
    match expression {
        ast::Expression::Inline(inline) => inline_vars(inline, out),
        ast::Expression::Select { selector, variants } => {
            inline_vars(selector, out);
            for variant in variants {
                pattern_vars(&variant.value, out);
            }
        }
    }
}

/// Collects every `$variable` an inline expression references directly.
fn inline_vars<'a>(inline: &ast::InlineExpression<&'a str>, out: &mut BTreeSet<&'a str>) {
    match inline {
        ast::InlineExpression::VariableReference { id } => {
            out.insert(id.name);
        }
        ast::InlineExpression::FunctionReference { arguments, .. } => call_vars(arguments, out),
        ast::InlineExpression::TermReference { arguments, .. } => {
            if let Some(arguments) = arguments {
                call_vars(arguments, out);
            }
        }
        ast::InlineExpression::Placeable { expression } => expression_vars(expression, out),
        ast::InlineExpression::StringLiteral { .. }
        | ast::InlineExpression::NumberLiteral { .. }
        | ast::InlineExpression::MessageReference { .. } => {}
    }
}

/// Collects every `$variable` a call's arguments reference.
fn call_vars<'a>(arguments: &ast::CallArguments<&'a str>, out: &mut BTreeSet<&'a str>) {
    for positional in &arguments.positional {
        inline_vars(positional, out);
    }
    for named in &arguments.named {
        inline_vars(&named.value, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_cases_kebab_and_camel_identifiers() {
        assert_eq!(ident_for("cart-items"), "cart_items");
        assert_eq!(ident_for("cartItems"), "cart_items");
        assert_eq!(ident_for("hello"), "hello");
        assert_eq!(ident_for("HTTPStatus"), "http_status");
        assert_eq!(ident_for("already_snake"), "already_snake");
    }

    #[test]
    fn pascal_cases_a_snake_identifier() {
        assert_eq!(pascal_for("editor_notice"), "EditorNotice");
        assert_eq!(pascal_for("type_"), "Type");
        assert_eq!(pascal_for("hello"), "Hello");
    }

    #[test]
    fn keywords_take_a_trailing_underscore() {
        assert_eq!(ident_for("type"), "type_");
        assert_eq!(ident_for("crate"), "crate_");
        assert_eq!(ident_for("loop"), "loop_");
    }
}
