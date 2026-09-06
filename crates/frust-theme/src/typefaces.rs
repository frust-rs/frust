//! [`NativeTypefaces`]: the native-control typeface binding a design system
//! attaches as a [`crate::extensions::ThemeExtensions`] payload.
//!
//! Every widget this repo paints itself resolves its face from
//! [`crate::typography::TypeScale`] (a `frust_text::TextStyle` family name,
//! resolved by the text engine's own font stack). A **native** control —
//! `frust-native-widgets`' Android `TextView`/iOS `UILabel` family — cannot:
//! the platform resolves fonts itself, so its host half needs the raw face
//! *bytes* to register with `Typeface.createFromFile`/CoreText before any
//! control can name the face at all.
//!
//! This extension is that seam, and the **only** route those bytes take: a
//! design system attaches its two faces here, and a native host reads them
//! back with `theme.extension::<NativeTypefaces>()`. Two slots — a
//! button/display face and a body face — mirroring the split
//! `frust-native-widgets`' theme ladder already applies (`Button` on one
//! face, `Label`/`Switch` on the other), and matching the two-payload shape
//! its platform publish seam already carries; a design system with one face
//! for everything uses [`NativeTypefaces::uniform`].
//!
//! # No CORE baseline attaches this
//!
//! Unlike [`StatusPalette`](crate::status::StatusPalette) (attached to
//! [`Theme::neutral`](crate::theme::Theme::neutral)), nothing this crate
//! constructs attaches `NativeTypefaces`. Attaching it is exclusively a
//! design system's job, and its absence is what tells a native host to leave
//! the platform's own face alone — `frust_glyph`'s baseline is the shipped
//! example of a design system that *does* attach it, which is how its
//! monospace faces reach native controls at all. See
//! `docs/NATIVE_WIDGETS_ARCHITECTURE.md`'s theme ladder.
//!
//! # Why `&'static [u8]`
//!
//! A design system embeds its faces with `include_bytes!`, so `'static` bytes
//! are what a caller already has and what a platform registration call
//! already wants (no copy, no `Arc`, no lifetime threading through the
//! theme). It also keeps [`FontFace`] `Copy` and keeps
//! [`Theme`](crate::theme::Theme)'s own `Clone`/`PartialEq` cheap.
//!
//! Note that [`ThemeExtensions`](crate::extensions::ThemeExtensions)'
//! `PartialEq` compares the *set of attached types*, never their values (see
//! that module) — so two `Theme`s differing only in which faces this
//! extension carries compare **equal**. A consumer that must react to a face
//! swap therefore keys off the payload itself (identity or content), never
//! off `Theme` equality.
//!
//! # Examples
//!
//! ```
//! use frust_theme::{FontFace, NativeTypefaces, Theme};
//!
//! // A design system's own embedded faces (`include_bytes!` in real code).
//! static DISPLAY: &[u8] = b"<display face bytes>";
//! static BODY: &[u8] = b"<body face bytes>";
//!
//! let theme = Theme::builder(Theme::neutral())
//!     .extension(NativeTypefaces {
//!         button: Some(FontFace::new("Acme Display", DISPLAY)),
//!         body: Some(FontFace::new("Acme Text", BODY)),
//!     })
//!     .build();
//!
//! let faces = theme.extension::<NativeTypefaces>().expect("attached above");
//! assert_eq!(faces.button.map(|f| f.family), Some("Acme Display"));
//! ```

/// One font face a native host can register: a stable family name plus the
/// raw face bytes.
///
/// `family` is for diagnostics and de-duplication only — it is never handed
/// to a platform font *lookup* (the whole point of carrying bytes is that the
/// platform has no way to look the family up), so it may be any stable,
/// human-meaningful name for the face.
///
/// `bytes` is one face's complete font file (TTF/OTF). Equality is by
/// *content* (derived), which is rarely what a hot path wants — see the
/// module doc's note on keying off payload identity instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontFace {
    /// A stable family name for this face — diagnostics/de-duplication only.
    pub family: &'static str,
    /// The face's complete font-file bytes (TTF/OTF).
    pub bytes: &'static [u8],
}

impl FontFace {
    /// A face from its family name and embedded bytes — `const` so a design
    /// system can declare its faces as `const`/`static` items alongside the
    /// `include_bytes!` payload they wrap.
    pub const fn new(family: &'static str, bytes: &'static [u8]) -> Self {
        Self { family, bytes }
    }
}

/// The native-control typeface binding: a display/button face and a body
/// face, either of which may be left unset (falling back to whatever the
/// native host resolves without it — see the module doc).
///
/// [`Default`] is both slots unset, i.e. "attached but selecting nothing" —
/// equivalent to not attaching the extension at all as far as a host's
/// fallback ladder is concerned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NativeTypefaces {
    /// The face a native *button*/display control resolves to.
    pub button: Option<FontFace>,
    /// The face a native *body*-text control (label, switch, and any other
    /// text-bearing control) resolves to.
    pub body: Option<FontFace>,
}

impl NativeTypefaces {
    /// Both slots from one face — the common case for a design system with a
    /// single UI face.
    pub const fn uniform(face: FontFace) -> Self {
        Self {
            button: Some(face),
            body: Some(face),
        }
    }

    /// Whether neither slot carries a face (see [`Default`]).
    pub const fn is_empty(&self) -> bool {
        self.button.is_none() && self.body.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    static DISPLAY: &[u8] = b"display-face-bytes";
    static BODY: &[u8] = b"body-face-bytes";

    fn display() -> FontFace {
        FontFace::new("Acme Display", DISPLAY)
    }

    fn body() -> FontFace {
        FontFace::new("Acme Text", BODY)
    }

    #[test]
    fn builder_attach_then_theme_extension_round_trips() {
        let theme = Theme::builder(Theme::neutral())
            .extension(NativeTypefaces {
                button: Some(display()),
                body: Some(body()),
            })
            .build();

        let faces = theme
            .extension::<NativeTypefaces>()
            .expect("attached just above");
        assert_eq!(faces.button, Some(display()));
        assert_eq!(faces.body, Some(body()));
        assert_eq!(faces.button.expect("set").bytes, DISPLAY);
    }

    #[test]
    fn direct_insert_round_trips_too() {
        // The `extensions.insert` route an app takes on an already-built
        // `Theme`, rather than through the builder.
        let mut theme = Theme::neutral();
        theme.extensions.insert(NativeTypefaces::uniform(body()));
        assert_eq!(
            theme.extension::<NativeTypefaces>(),
            Some(&NativeTypefaces::uniform(body()))
        );
    }

    #[test]
    fn re_attaching_replaces_last_write_wins() {
        let theme = Theme::builder(Theme::neutral())
            .extension(NativeTypefaces::uniform(display()))
            .extension(NativeTypefaces::uniform(body()))
            .build();
        assert_eq!(
            theme.extension::<NativeTypefaces>(),
            Some(&NativeTypefaces::uniform(body()))
        );
    }

    #[test]
    fn uniform_sets_both_slots_and_is_not_empty() {
        let faces = NativeTypefaces::uniform(body());
        assert_eq!(faces.button, Some(body()));
        assert_eq!(faces.body, Some(body()));
        assert!(!faces.is_empty());
    }

    #[test]
    fn default_leaves_both_slots_unset() {
        let faces = NativeTypefaces::default();
        assert!(faces.button.is_none());
        assert!(faces.body.is_none());
        assert!(faces.is_empty());
        // A half-filled binding is still not empty.
        assert!(
            !NativeTypefaces {
                button: Some(display()),
                ..NativeTypefaces::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn no_core_baseline_attaches_this_extension() {
        // The module doc's contract: attaching is exclusively a design
        // system's job, and absence is what tells a native host to leave the
        // platform's own face alone. `Theme::neutral()` is the only baseline
        // this crate constructs, so it is the whole of "no CORE baseline"; a
        // design system's baseline (`frust_glyph`'s, say) deliberately DOES
        // attach one, and carries its own test for that.
        assert!(Theme::neutral().extension::<NativeTypefaces>().is_none());
    }

    #[test]
    fn theme_equality_ignores_a_face_swap() {
        // `ThemeExtensions`' PartialEq compares the attached *type set*, so a
        // consumer must not diff themes to notice a face swap (module doc).
        let with_display = Theme::builder(Theme::neutral())
            .extension(NativeTypefaces::uniform(display()))
            .build();
        let with_body = Theme::builder(Theme::neutral())
            .extension(NativeTypefaces::uniform(body()))
            .build();
        assert_eq!(with_display, with_body);
        assert_ne!(
            with_display.extension::<NativeTypefaces>(),
            with_body.extension::<NativeTypefaces>(),
            "the payloads themselves still differ — which is what a consumer \
             keys off"
        );
    }
}
