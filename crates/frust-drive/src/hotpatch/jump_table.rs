//! The jump table a patch is applied with: base link-time address → patch
//! link-time address for every symbol the two images share by name, plus
//! both images' anchor addresses so the runtime can rebase each side.
//!
//! A port of dioxus-cli 0.7.10's `create_native_jump_table`
//! (`build/patch.rs`), anchored on `__frust_hotpatch_anchor` instead of
//! `main`. Identity is purely by name: a function new in the patch gets no
//! entry, and one whose name survived a signature change gets one, which is
//! why the patch's admissibility is decided elsewhere. The table travels as
//! [`JumpTableWire`], which carries no library path.
//!
//! The runtime (`frust_hotpatch::apply_patch`) adds
//! `anchor_runtime - aslr_reference` to every key and the patch's
//! `anchor_runtime - new_base_address` to every value, refusing the table
//! when either offset is not that image's slide.

use frust_devtools_protocol::JumpTableWire;
use object::SymbolKind;

use super::HotpatchError;
use super::symbols::{CachedSymbol, ImageSymbols, SymbolCache};

/// Whether an entry may appear in the table: defined in a section of its
/// image, and not a thread-local (whose "address" is a TLS offset or a
/// descriptor, never code to jump to).
fn mappable(symbol: &CachedSymbol) -> bool {
    symbol.is_section_defined()
        && !matches!(
            symbol.kind,
            SymbolKind::Tls | SymbolKind::Section | SymbolKind::File
        )
}

/// Build the table mapping `cache`'s base image onto the patch image whose
/// symbol table is `patch`.
pub fn create_jump_table(
    cache: &SymbolCache,
    patch: &ImageSymbols,
) -> Result<JumpTableWire, HotpatchError> {
    let target = cache.target();
    if patch.target() != target {
        return Err(HotpatchError::unsupported(format!(
            "the patch was read for {:?}, the base image for {target:?}",
            patch.target()
        )));
    }

    let anchor = target.anchor_symbol();
    let new_base_address = patch
        .get(&anchor)
        .filter(|s| s.is_section_defined() && s.kind == SymbolKind::Text)
        .map(|s| s.address)
        .ok_or_else(|| {
            HotpatchError::unsupported(format!(
                "the patch library does not export the `{anchor}` anchor"
            ))
        })?;

    // Sorted, so when two names share a base address (folded code) the
    // entry that wins is the same on every build.
    let mut shared: Vec<(&str, u64, u64)> = patch
        .iter()
        .filter(|(_, new)| mappable(new))
        .filter_map(|(name, new)| {
            let old = cache.symbols().get(name).filter(|old| mappable(old))?;
            Some((name, old.address, new.address))
        })
        .collect();
    shared.sort_unstable();
    let map = shared.into_iter().map(|(_, old, new)| (old, new)).collect();

    Ok(JumpTableWire {
        map,
        aslr_reference: cache.anchor_address(),
        new_base_address,
        ifunc_count: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::super::symbols::fixtures::{Def, TRIPLES, object};
    use super::super::symbols::{ANCHOR_SYMBOL, Target};
    use super::*;

    fn base(target: Target) -> SymbolCache {
        let bytes = object(
            target,
            &[
                Def::Text("old_only_fn", 24),
                Def::Text("foo_fn", 16),
                Def::Text(ANCHOR_SYMBOL, 4),
                Def::Text("bar_fn", 16),
                // Relocatable ELF sections all start at 0: padding keeps
                // data addresses clear of text addresses.
                Def::Data("PAD_DATA", &[0; 64]),
                Def::Data("SHARED_DATA", &[1, 2, 3, 4]),
                Def::Tls("SHARED_TLS", &[5, 6, 7, 8]),
                Def::Undefined("libc_import"),
            ],
        );
        SymbolCache::from_bytes("base", &bytes, target).unwrap()
    }

    fn patch(target: Target) -> ImageSymbols {
        let bytes = object(
            target,
            &[
                Def::Text("bar_fn", 40),
                Def::Text("new_only_fn", 8),
                Def::Text(ANCHOR_SYMBOL, 4),
                Def::Text("foo_fn", 24),
                Def::Data("PAD_DATA_PATCH", &[0; 64]),
                Def::Data("SHARED_DATA", &[9, 9, 9, 9]),
                Def::Tls("SHARED_TLS", &[5, 6, 7, 8]),
                Def::Undefined("libc_import"),
            ],
        );
        ImageSymbols::parse(&bytes, target, "patch").unwrap()
    }

    /// The runtime's rebasing rule (`apply_patch_with_anchor`): each side's
    /// offset is its runtime anchor minus the table's link-time anchor, and
    /// must equal that image's slide.
    fn rebase(
        table: &JumpTableWire,
        base_anchor_runtime: u64,
        patch_anchor_runtime: u64,
        base_slide: u64,
        patch_slide: u64,
    ) -> std::collections::HashMap<u64, u64> {
        let old_offset = base_anchor_runtime.wrapping_sub(table.aslr_reference);
        let new_offset = patch_anchor_runtime.wrapping_sub(table.new_base_address);
        assert_eq!(old_offset, base_slide, "AnchorMismatch on the base image");
        assert_eq!(new_offset, patch_slide, "AnchorMismatch on the patch image");
        table
            .map
            .iter()
            .map(|(old, new)| (old.wrapping_add(old_offset), new.wrapping_add(new_offset)))
            .collect()
    }

    #[test]
    fn only_name_matched_symbols_map_and_the_anchors_fill_the_header() {
        for triple in TRIPLES {
            let target = Target::from_triple(triple).unwrap();
            let cache = base(target);
            let patch = patch(target);
            let table = create_jump_table(&cache, &patch).unwrap();
            let raw = |name: &str| target.raw_symbol_name(name);
            let old = |name: &str| cache.symbols().defined_address(&raw(name)).unwrap();
            let new = |name: &str| patch.defined_address(&raw(name)).unwrap();

            assert_eq!(table.aslr_reference, cache.anchor_address(), "{triple}");
            assert_eq!(table.aslr_reference, old(ANCHOR_SYMBOL), "{triple}");
            assert_eq!(table.new_base_address, new(ANCHOR_SYMBOL), "{triple}");
            assert_eq!(table.ifunc_count, 0);

            let mut expected = std::collections::HashMap::new();
            for name in ["foo_fn", "bar_fn", ANCHOR_SYMBOL, "SHARED_DATA"] {
                expected.insert(old(name), new(name));
            }
            assert_eq!(table.map, expected, "{triple}");
            // Neither the base-only nor the patch-only function has an entry,
            // the import is never a key, and no TLS address is mapped.
            assert!(!table.map.contains_key(&old("old_only_fn")), "{triple}");
            assert!(
                !table.map.values().any(|v| *v == new("new_only_fn")),
                "{triple}"
            );
        }
    }

    #[test]
    fn rebasing_with_the_runtime_rule_reaches_the_runtime_addresses() {
        let base_slide = 0x0000_7f00_1234_0000_u64;
        let patch_slide = 0x0000_7f55_8000_0000_u64;
        for triple in TRIPLES {
            let target = Target::from_triple(triple).unwrap();
            let cache = base(target);
            let patch = patch(target);
            let table = create_jump_table(&cache, &patch).unwrap();
            let raw = |name: &str| target.raw_symbol_name(name);
            let old = |name: &str| cache.symbols().defined_address(&raw(name)).unwrap();
            let new = |name: &str| patch.defined_address(&raw(name)).unwrap();

            // What the process reports and the loader produces.
            let base_anchor_runtime = old(ANCHOR_SYMBOL) + base_slide;
            let patch_anchor_runtime = new(ANCHOR_SYMBOL) + patch_slide;

            let rebased = rebase(
                &table,
                base_anchor_runtime,
                patch_anchor_runtime,
                base_slide,
                patch_slide,
            );
            for name in ["foo_fn", "bar_fn"] {
                assert_eq!(
                    rebased.get(&(old(name) + base_slide)),
                    Some(&(new(name) + patch_slide)),
                    "{triple}: {name}"
                );
            }
        }
    }

    #[test]
    fn the_wire_table_carries_no_library_path() {
        let target = Target::from_triple("aarch64-apple-darwin").unwrap();
        let table = create_jump_table(&base(target), &patch(target)).unwrap();
        let json = serde_json::to_value(&table).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["aslr_reference", "ifunc_count", "map", "new_base_address"]
        );
    }

    #[test]
    fn a_patch_without_the_anchor_is_refused() {
        for triple in TRIPLES {
            let target = Target::from_triple(triple).unwrap();
            let bytes = object(target, &[Def::Text("foo_fn", 8)]);
            let patch = ImageSymbols::parse(&bytes, target, "patch").unwrap();
            let err = create_jump_table(&base(target), &patch).unwrap_err();
            assert!(
                matches!(&err, HotpatchError::BuilderUnsupported { detail } if detail.contains(ANCHOR_SYMBOL)),
                "{triple}: {err:?}"
            );
        }
    }

    #[test]
    fn a_patch_for_another_target_is_refused() {
        let darwin = Target::from_triple("aarch64-apple-darwin").unwrap();
        let x86 = Target::from_triple("x86_64-apple-darwin").unwrap();
        let err = create_jump_table(&base(darwin), &patch(x86)).unwrap_err();
        assert!(
            matches!(err, HotpatchError::BuilderUnsupported { .. }),
            "{err:?}"
        );

        // And an image in an unknown format never becomes a patch table.
        let err = ImageSymbols::parse(b"\x7fELF but truncated", darwin, "patch").unwrap_err();
        assert!(
            matches!(err, HotpatchError::BuilderUnsupported { .. }),
            "{err:?}"
        );
    }
}
