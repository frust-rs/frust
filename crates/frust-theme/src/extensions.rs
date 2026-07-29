//! [`ThemeExtensions`]: the no-lock-in typed extension slot [`crate::theme::Theme`]
//! carries (a Flutter `ThemeExtension` analog).
//!
//! `Theme` is a fixed 9(+1)-field struct — every widget in this repo reads a
//! named field, and that stays true. But a third-party design system (or an
//! app) that wants to carry its own token types alongside the built-in ones
//! (without forking `frust-theme` to add a field `Theme` doesn't otherwise
//! need) has nowhere to put them. `ThemeExtensions` is that seam: a small
//! persistent type-map keyed by [`TypeId`], `Arc`-backed so a `Theme` clone
//! (required at both delivery paths — the process-global `set_app_theme`
//! `Mutex` slot and the reactive `provide_context` copy, see
//! `docs/ARCHITECTURE.md`'s Theme delivery) stays cheap regardless of how
//! many extension types are attached.
//!
//! [`StatusPalette`](crate::status::StatusPalette) is the first consumer —
//! see that module.
//!
//! # Examples
//!
//! ```
//! use frust_theme::Theme;
//!
//! // A third-party (or app-local) extension type — nothing `frust-theme`
//! // needs to know about ahead of time.
//! #[derive(Debug, Clone, PartialEq)]
//! struct BrandTokens {
//!     logo_glow: bool,
//! }
//!
//! let mut theme = Theme::m3_baseline();
//! theme.extensions.insert(BrandTokens { logo_glow: true });
//!
//! let tokens = theme.extension::<BrandTokens>().expect("inserted above");
//! assert!(tokens.logo_glow);
//!
//! // A type that was never inserted comes back `None`, never a panic.
//! #[derive(Debug)]
//! struct NeverInserted;
//! assert!(theme.extension::<NeverInserted>().is_none());
//! ```

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// A persistent, typed extension map: any `'static + Send + Sync` type can be
/// attached once (a later [`insert`](Self::insert) of the same type replaces
/// it) and recovered by type via [`get`](Self::get).
///
/// `Send + Sync` bounds match [`crate::theme::Theme`]'s own crossing of
/// `frust-shell-common`'s process-global `Mutex<OverrideSlot>` (see that
/// module's thread contract) — an extension type that isn't `Send + Sync`
/// simply can't be inserted, a compile-time guarantee rather than a runtime
/// one.
#[derive(Clone, Default)]
pub struct ThemeExtensions {
    map: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl ThemeExtensions {
    /// An empty extension map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach `ext`, keyed by its concrete type `T`. A later `insert::<T>`
    /// call replaces whatever `T` was previously attached (last write wins,
    /// mirroring `HashMap::insert`).
    pub fn insert<T: Any + Send + Sync>(&mut self, ext: T) {
        self.map.insert(TypeId::of::<T>(), Arc::new(ext));
    }

    /// Recover the attached `T`, or `None` if nothing of that type was ever
    /// [`insert`](Self::insert)ed — never a panic on a type that isn't there.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.map.get(&TypeId::of::<T>())?.downcast_ref::<T>()
    }

    /// Whether any extension is attached at all — mostly useful for tests.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// How many distinct extension types are attached.
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

impl fmt::Debug for ThemeExtensions {
    // `Arc<dyn Any + Send + Sync>` carries no `Debug` impl (trait objects
    // can't require one without foreclosing arbitrary extension types), so
    // this reports the attached type count rather than any value.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThemeExtensions")
            .field("len", &self.map.len())
            .finish()
    }
}

impl PartialEq for ThemeExtensions {
    // Trait objects behind `Arc<dyn Any>` aren't structurally comparable (no
    // blanket `PartialEq` for arbitrary `T`), so this compares the *set of
    // attached types* rather than their values — good enough for `Theme`'s
    // existing identity-style comparisons (`frust-shell-common`'s override
    // watcher tests `assert_eq!` a polled `Theme` against a clone of the
    // exact value that was set, which trivially shares the same type set)
    // without requiring every future extension type to implement `Eq`.
    fn eq(&self, other: &Self) -> bool {
        self.map.len() == other.map.len() && self.map.keys().all(|k| other.map.contains_key(k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Foo(u32);

    #[derive(Debug, PartialEq)]
    struct Bar(&'static str);

    #[test]
    fn insert_then_get_round_trips() {
        let mut ext = ThemeExtensions::new();
        ext.insert(Foo(42));
        assert_eq!(ext.get::<Foo>(), Some(&Foo(42)));
    }

    #[test]
    fn missing_type_is_none_not_a_panic() {
        let ext = ThemeExtensions::new();
        assert!(ext.get::<Foo>().is_none());
    }

    #[test]
    fn distinct_types_coexist() {
        let mut ext = ThemeExtensions::new();
        ext.insert(Foo(1));
        ext.insert(Bar("hi"));
        assert_eq!(ext.get::<Foo>(), Some(&Foo(1)));
        assert_eq!(ext.get::<Bar>(), Some(&Bar("hi")));
    }

    #[test]
    fn re_insert_of_same_type_replaces() {
        let mut ext = ThemeExtensions::new();
        ext.insert(Foo(1));
        ext.insert(Foo(2));
        assert_eq!(ext.get::<Foo>(), Some(&Foo(2)));
        assert_eq!(ext.len(), 1);
    }

    #[test]
    fn empty_map_reports_empty() {
        let ext = ThemeExtensions::new();
        assert!(ext.is_empty());
        assert_eq!(ext.len(), 0);
    }

    #[test]
    fn clone_is_independent_and_shares_attached_values() {
        let mut ext = ThemeExtensions::new();
        ext.insert(Foo(7));
        let cloned = ext.clone();
        assert_eq!(cloned.get::<Foo>(), Some(&Foo(7)));

        // Mutating the original after cloning doesn't affect the clone —
        // `Clone` on the map itself, not a shared `Arc<Mutex<..>>`.
        ext.insert(Foo(99));
        assert_eq!(ext.get::<Foo>(), Some(&Foo(99)));
        assert_eq!(cloned.get::<Foo>(), Some(&Foo(7)));
    }

    #[test]
    fn equality_compares_attached_type_set() {
        let mut a = ThemeExtensions::new();
        a.insert(Foo(1));
        let mut b = ThemeExtensions::new();
        b.insert(Foo(999)); // different value, same type set
        assert_eq!(a, b);

        let mut c = ThemeExtensions::new();
        c.insert(Bar("x")); // different type set
        assert_ne!(a, c);
    }

    #[test]
    fn debug_reports_len_not_values() {
        let mut ext = ThemeExtensions::new();
        ext.insert(Foo(1));
        let s = format!("{ext:?}");
        assert!(s.contains("ThemeExtensions"));
        assert!(s.contains('1'));
    }
}
