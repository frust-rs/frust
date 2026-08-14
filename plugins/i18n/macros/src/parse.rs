//! Locale-tree discovery and compile-time Fluent syntax checking.
//!
//! # What "compile time" buys
//!
//! Every `.ftl` file under the macro's argument is read and parsed with
//! `fluent-syntax` while the macro expands, so a malformed message is a
//! *compilation* failure naming the file, line, and column — not a runtime
//! `I18nError::Format` an app only discovers when the screen holding that
//! message opens.
//!
//! # Diagnostics name the path the caller wrote
//!
//! A file is reported as `<macro argument>/<locale>/<file>` — the path
//! relative to the invoking crate's `CARGO_MANIFEST_DIR`, i.e. exactly what
//! the caller typed plus what was found underneath it. Absolute paths are
//! deliberately kept out of the message: they differ per machine, which
//! would make the compile-fail suite's expected output unportable.
//!
//! # Rebuild tracking
//!
//! Reading a file inside a proc macro creates no dependency edge — `rustc`
//! has no idea the expansion consumed it. [`SourceFile::embed`] is what
//! closes that: every consumed file is embedded through `include_str!`, so
//! editing a `.ftl` both re-triggers compilation and re-runs the syntax
//! check above.

use std::fs;
use std::path::{Path, PathBuf};

use fluent_syntax::ast;
use fluent_syntax::parser::{self, ParserError};
use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::LitStr;
use unic_langid::LanguageIdentifier;

/// One locale directory: its BCP-47 tag and the `.ftl` files beneath it.
pub struct LocaleDir {
    /// The directory name, already validated as a BCP-47 identifier.
    pub tag: String,
    /// Every `.ftl` file under the directory, sorted by relative path.
    pub files: Vec<SourceFile>,
}

/// One `.ftl` file: where it came from and what it holds.
pub struct SourceFile {
    /// Path relative to the locale directory (`app.ftl`, `forms/cart.ftl`) —
    /// the name registered with `LocaleSet`, carried for diagnostics only.
    pub name: String,
    /// Path relative to the invoking crate, for compile diagnostics.
    pub display: String,
    /// Absolute path, for the `include_str!` the expansion embeds.
    pub path: PathBuf,
    /// The file's contents, read at expansion time for the syntax check.
    pub text: String,
}

impl SourceFile {
    /// The `(name, source)` pair `LocaleSet::with_locale` takes, with the
    /// source embedded through `include_str!` rather than pasted inline.
    ///
    /// Embedding by reference is what registers the rebuild dependency (see
    /// this module's *Rebuild tracking* note); it also keeps the expansion
    /// small no matter how large the message catalog is.
    pub fn embed(&self, span: Span) -> TokenStream {
        let name = LitStr::new(&self.name, span);
        let path = LitStr::new(&self.path.to_string_lossy(), span);
        quote! { (#name, ::core::include_str!(#path)) }
    }
}

/// Enumerates `<root>/<locale>/**/*.ftl`, validating every directory name as
/// a BCP-47 identifier.
///
/// Non-directory entries directly under `root`, hidden entries (a leading
/// `.`, so an editor's or the OS's droppings never become a "locale"), and
/// non-`.ftl` files inside a locale directory are all skipped. The result is
/// sorted by tag, and each directory's files by relative path, so the
/// expansion is byte-identical across machines and filesystems.
///
/// # Errors
///
/// A [`syn::Error`] carrying `span` when `root` cannot be read, when a
/// directory name is not a well-formed BCP-47 identifier, or when the tree
/// holds no locale directory at all.
pub fn discover(root: &Path, rel: &str, span: Span) -> Result<Vec<LocaleDir>, syn::Error> {
    let entries = fs::read_dir(root).map_err(|error| {
        syn::Error::new(
            span,
            format!(
                "`locales!`: cannot read the locales directory `{rel}` \
                 (resolved against the crate's CARGO_MANIFEST_DIR): {error}"
            ),
        )
    })?;

    let mut locales = Vec::new();
    let mut failure: Option<syn::Error> = None;
    for entry in entries {
        let entry = entry.map_err(|error| {
            syn::Error::new(span, format!("`locales!`: reading `{rel}`: {error}"))
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !entry.path().is_dir() {
            continue;
        }

        if let Err(error) = name.parse::<LanguageIdentifier>() {
            combine(
                &mut failure,
                syn::Error::new(
                    span,
                    format!(
                        "`locales!`: `{rel}/{name}` is not a BCP-47 locale identifier: {error} \
                         — every directory directly under `{rel}` names one locale \
                         (`en`, `en-US`, `zh-Hans-CN`)"
                    ),
                ),
            );
            continue;
        }

        let mut files = Vec::new();
        collect(&entry.path(), "", &mut files, span, rel, &name)?;
        files.sort_by(|a, b| a.name.cmp(&b.name));
        locales.push(LocaleDir { tag: name, files });
    }

    if let Some(error) = failure {
        return Err(error);
    }
    if locales.is_empty() {
        return Err(syn::Error::new(
            span,
            format!(
                "`locales!`: `{rel}` holds no locale directory — expected one \
                 directory per locale (`{rel}/en/app.ftl`)"
            ),
        ));
    }

    locales.sort_by(|a, b| a.tag.cmp(&b.tag));
    Ok(locales)
}

/// Recursively gathers a locale directory's `.ftl` files into `out`.
fn collect(
    dir: &Path,
    prefix: &str,
    out: &mut Vec<SourceFile>,
    span: Span,
    rel: &str,
    tag: &str,
) -> Result<(), syn::Error> {
    let entries = fs::read_dir(dir).map_err(|error| {
        syn::Error::new(span, format!("`locales!`: reading `{rel}/{tag}`: {error}"))
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            syn::Error::new(span, format!("`locales!`: reading `{rel}/{tag}`: {error}"))
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let nested = format!("{prefix}{name}");

        if entry.path().is_dir() {
            collect(&entry.path(), &format!("{nested}/"), out, span, rel, tag)?;
            continue;
        }
        if !name.ends_with(".ftl") {
            continue;
        }

        let path = entry.path();
        let text = fs::read_to_string(&path).map_err(|error| {
            syn::Error::new(
                span,
                format!("`locales!`: reading `{rel}/{tag}/{nested}`: {error}"),
            )
        })?;
        out.push(SourceFile {
            display: format!("{rel}/{tag}/{nested}"),
            name: nested,
            path,
            text,
        });
    }

    Ok(())
}

/// Syntax-checks every file of one locale, returning their ASTs in
/// registration order.
///
/// # Errors
///
/// One [`syn::Error`] per malformed file, combined into a single value —
/// each names the file, line, column, and the parser's own message, and
/// every file is checked even after one fails (a first-error-only report
/// would hide the rest of a freshly translated catalog's mistakes).
pub fn check(locale: &LocaleDir, span: Span) -> Result<Vec<ast::Resource<&str>>, syn::Error> {
    let mut parsed = Vec::with_capacity(locale.files.len());
    let mut failure: Option<syn::Error> = None;

    for source in &locale.files {
        match parser::parse(source.text.as_str()) {
            Ok(resource) => parsed.push(resource),
            Err((_, errors)) => {
                combine(&mut failure, syn::Error::new(span, report(source, &errors)));
            }
        }
    }

    match failure {
        Some(error) => Err(error),
        None => Ok(parsed),
    }
}

/// Renders one file's parse errors as a multi-line diagnostic.
fn report(source: &SourceFile, errors: &[ParserError]) -> String {
    let mut message = format!(
        "`locales!`: malformed Fluent syntax in `{}`",
        source.display
    );
    for error in errors {
        let (line, column) = position(&source.text, error.pos.start);
        message.push_str(&format!("\n  {}:{line}:{column}: {error}", source.display));
    }
    message
}

/// Maps a byte offset into 1-based `(line, column)`, counting the column in
/// characters so a diagnostic points where an editor's cursor would sit.
fn position(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let column = match before.rfind('\n') {
        Some(newline) => before[newline + 1..].chars().count() + 1,
        None => before.chars().count() + 1,
    };
    (line, column)
}

/// Accumulates `error` into `slot`, so one expansion reports every problem
/// it found rather than only the first.
pub fn combine(slot: &mut Option<syn::Error>, error: syn::Error) {
    match slot {
        Some(existing) => existing.combine(error),
        None => *slot = Some(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_counts_lines_and_characters() {
        let text = "a = 1\nbb = 2\n";

        assert_eq!(position(text, 0), (1, 1));
        assert_eq!(position(text, 6), (2, 1));
        assert_eq!(position(text, 8), (2, 3));
    }

    #[test]
    fn position_clamps_an_out_of_range_offset() {
        assert_eq!(position("a", 99), (1, 2));
    }

    #[test]
    fn combine_accumulates_every_error() {
        let mut slot = None;
        combine(&mut slot, syn::Error::new(Span::call_site(), "first"));
        combine(&mut slot, syn::Error::new(Span::call_site(), "second"));

        let rendered = slot.expect("two errors").into_iter().count();

        assert_eq!(rendered, 2);
    }
}
