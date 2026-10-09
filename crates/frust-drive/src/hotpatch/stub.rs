//! The undefined-symbol stub: one object, linked into every patch, that
//! satisfies each symbol the patch's inputs reference but do not define by
//! pointing it at the running process's own copy.
//!
//! A port of dioxus-cli 0.7.10's `create_undefined_symbol_stub`
//! (`build/patch.rs`) for ELF and Mach-O on aarch64 and x86_64 (System V).
//! Each referenced symbol is looked up in the base image's [`SymbolCache`]
//! and slid by the ASLR offset `anchor_runtime - anchor link address`:
//!
//! - **text** becomes a thunk that jumps to the absolute runtime address;
//! - **TLS** becomes a fresh thread-local seeded with the base's
//!   initializer, so each patch has its own copy and its value resets when
//!   the patch loads;
//! - **data** becomes an absolute symbol at the runtime address.
//!
//! The stub hard-codes one process's addresses, so it is built only once
//! that process has reported its anchor. Unlike dx it fails closed: a
//! strongly referenced symbol the base does not have refuses the whole stub
//! rather than leaving it out. A symbol the base itself imports is left for
//! the patch's own link to resolve, as is a weak reference the base lacks.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use object::read::{Object, ObjectSymbol};
use object::write::{self, MachOBuildVersion, StandardSection, Symbol, SymbolSection};
use object::{Endianness, SymbolFlags, SymbolKind, SymbolScope};

use super::HotpatchError;
use super::symbols::{Arch, CachedSymbol, Format, Os, RawFlags, SymbolCache, Target, parse_object};

/// How many missing names a refusal lists before summarising the rest.
const MISSING_LISTED: usize = 8;

/// The macOS version the stub's `LC_BUILD_VERSION` declares, as `xxxx.yy.zz`
/// nibbles (11.0.0, the first release with arm64).
const MACOS_MIN_VERSION: u32 = 11 << 16;

/// The iOS simulator version the stub's `LC_BUILD_VERSION` declares (14.0.0,
/// rustc's default deployment target for `aarch64-apple-ios-sim`). ld refuses
/// to link a macOS-platform object into a simulator image, and an object
/// newer than the patch's own deployment target draws a warning.
pub(crate) const IOS_SIMULATOR_MIN_VERSION: u32 = 14 << 16;

/// `ldr x16, #8` — load the 64-bit literal that follows the next instruction.
const AARCH64_LDR_X16_LITERAL: [u8; 4] = [0x50, 0x00, 0x00, 0x58];
/// `br x16`.
const AARCH64_BR_X16: [u8; 4] = [0x00, 0x02, 0x1f, 0xd6];
/// `jmp qword ptr [rip + 0]` — jump through the 64-bit address that follows.
const X86_64_JMP_RIP_INDIRECT: [u8; 6] = [0xff, 0x25, 0x00, 0x00, 0x00, 0x00];

/// The symbols a patch's inputs reference but none of them defines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UndefinedSymbols {
    /// Raw names with at least one strong (non-weak) reference.
    pub strong: BTreeSet<String>,
    /// Raw names referenced only weakly.
    pub weak: BTreeSet<String>,
}

impl UndefinedSymbols {
    /// Collect from the patch inputs: object files, and `.rlib`/`.a`
    /// archives whose `.o`/`.obj` members are read.
    pub fn collect(inputs: &[PathBuf]) -> Result<Self, HotpatchError> {
        let mut collector = Collector::default();
        for path in inputs {
            collector.add_path(path)?;
        }
        Ok(collector.finish())
    }
}

#[derive(Default)]
struct Collector {
    strong: BTreeSet<String>,
    weak: BTreeSet<String>,
    defined: BTreeSet<String>,
}

impl Collector {
    fn add_path(&mut self, path: &Path) -> Result<(), HotpatchError> {
        let bytes = std::fs::read(path)
            .map_err(|err| HotpatchError::io(format!("reading `{}`", path.display()), err))?;
        let is_archive = path
            .extension()
            .is_some_and(|ext| ext == "rlib" || ext == "a");
        if !is_archive {
            return self.add_object(&bytes, &format!("`{}`", path.display()));
        }

        let read_error =
            |err| HotpatchError::io(format!("reading archive `{}`", path.display()), err);
        let mut archive = ar::Archive::new(std::io::Cursor::new(bytes));
        while let Some(entry) = archive.next_entry() {
            let mut entry = entry.map_err(read_error)?;
            let member = String::from_utf8_lossy(entry.header().identifier()).into_owned();
            if !(member.ends_with(".o") || member.ends_with(".obj")) {
                continue;
            }
            let mut member_bytes = Vec::new();
            entry.read_to_end(&mut member_bytes).map_err(read_error)?;
            self.add_object(
                &member_bytes,
                &format!("`{member}` in `{}`", path.display()),
            )?;
        }
        Ok(())
    }

    fn add_object(&mut self, bytes: &[u8], what: &str) -> Result<(), HotpatchError> {
        let file = parse_object(bytes, what)?;
        for symbol in file.symbols() {
            let name = symbol.name().map_err(|err| {
                HotpatchError::unsupported(format!("{what} has an unreadable symbol name: {err}"))
            })?;
            if name.is_empty() {
                continue;
            }
            if symbol.is_undefined() {
                if symbol.is_weak() {
                    self.weak.insert(name.to_string());
                } else {
                    self.strong.insert(name.to_string());
                }
            } else if symbol.is_global() {
                self.defined.insert(name.to_string());
            }
        }
        Ok(())
    }

    fn finish(self) -> UndefinedSymbols {
        let strong: BTreeSet<String> = self.strong.difference(&self.defined).cloned().collect();
        let weak = self
            .weak
            .into_iter()
            .filter(|name| !self.defined.contains(name) && !strong.contains(name))
            .collect();
        UndefinedSymbols { strong, weak }
    }
}

/// Collect the patch inputs' undefined symbols and build the stub for the
/// process whose anchor is at `anchor_runtime`.
pub fn create_undefined_symbol_stub(
    cache: &SymbolCache,
    inputs: &[PathBuf],
    anchor_runtime: u64,
) -> Result<Vec<u8>, HotpatchError> {
    let undefined = UndefinedSymbols::collect(inputs)?;
    build_stub(cache, &undefined, anchor_runtime)
}

/// Build the stub object for `undefined`, every address slid for the
/// process whose anchor is at `anchor_runtime`. Every name is resolved
/// before anything is written, so a refusal never yields a partial stub.
pub fn build_stub(
    cache: &SymbolCache,
    undefined: &UndefinedSymbols,
    anchor_runtime: u64,
) -> Result<Vec<u8>, HotpatchError> {
    let target = cache.target();
    let slide = anchor_runtime
        .checked_sub(cache.anchor_address())
        .ok_or_else(|| {
            HotpatchError::unsupported(format!(
                "runtime anchor {anchor_runtime:#x} is below the base image's link-time anchor {:#x}",
                cache.anchor_address()
            ))
        })?;

    let symbols = cache.symbols();
    let mut missing = Vec::new();
    let mut resolved: Vec<(&str, CachedSymbol)> = Vec::new();
    for name in &undefined.strong {
        match symbols.get(name) {
            None => missing.push(name.as_str()),
            // Imported by the base too: the patch's own link resolves it.
            Some(symbol) if symbol.is_undefined() => {}
            Some(symbol) => resolved.push((name, *symbol)),
        }
    }
    for name in &undefined.weak {
        if let Some(symbol) = symbols.get(name).filter(|s| !s.is_undefined()) {
            resolved.push((name, *symbol));
        }
    }
    if !missing.is_empty() {
        let listed = missing
            .iter()
            .take(MISSING_LISTED)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        let more = missing.len().saturating_sub(MISSING_LISTED);
        let more = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        return Err(HotpatchError::unsupported(format!(
            "the patch references {} symbol(s) the base image `{}` does not have: {listed}{more}",
            missing.len(),
            cache.path().display()
        )));
    }

    let mut obj = write::Object::new(
        target.object_format(),
        target.object_architecture(),
        Endianness::Little,
    );
    if target.format() == Format::MachO {
        let (platform, min_version) = match target.os {
            Os::IosSim => (
                object::macho::PLATFORM_IOSSIMULATOR,
                IOS_SIMULATOR_MIN_VERSION,
            ),
            _ => (object::macho::PLATFORM_MACOS, MACOS_MIN_VERSION),
        };
        let mut version = MachOBuildVersion::default();
        version.platform = platform;
        version.minos = min_version;
        version.sdk = min_version;
        obj.set_macho_build_version(version);
    }

    for (name, symbol) in resolved {
        let stub_name = stub_symbol_name(target, name)?;
        match symbol.kind {
            SymbolKind::Text => {
                let address = runtime_address(&symbol, slide, name)?;
                add_thunk(&mut obj, target, stub_name, address);
            }
            SymbolKind::Tls => add_tls(&mut obj, cache, target, name, stub_name)?,
            SymbolKind::Data | SymbolKind::Unknown => {
                let address = runtime_address(&symbol, slide, name)?;
                add_absolute(&mut obj, target, &symbol, stub_name, address);
            }
            other => {
                return Err(HotpatchError::unsupported(format!(
                    "the patch references `{name}`, a {other:?} symbol in the base image"
                )));
            }
        }
    }

    obj.write().map_err(|err| {
        HotpatchError::unsupported(format!("writing the undefined-symbol stub failed: {err}"))
    })
}

/// The name handed to the object writer. The Mach-O writer adds Darwin's
/// leading `_` itself, so it is stripped here and the written symbol reads
/// back exactly as the patch spelled it.
fn stub_symbol_name(target: Target, raw_name: &str) -> Result<&[u8], HotpatchError> {
    match target.format() {
        Format::Elf => Ok(raw_name.as_bytes()),
        Format::MachO => raw_name
            .strip_prefix(target.symbol_prefix())
            .map(str::as_bytes)
            .ok_or_else(|| {
                HotpatchError::unsupported(format!(
                    "Mach-O symbol `{raw_name}` lacks the leading `_` every C-level name carries"
                ))
            }),
    }
}

fn runtime_address(symbol: &CachedSymbol, slide: u64, name: &str) -> Result<u64, HotpatchError> {
    symbol.address.checked_add(slide).ok_or_else(|| {
        HotpatchError::unsupported(format!(
            "`{name}` slid by {slide:#x} overflows the address space"
        ))
    })
}

/// A thunk that jumps to `address` through a scratch register (aarch64
/// `x16`, the intra-procedure-call register) or a RIP-relative indirect
/// jump (x86_64), the 64-bit address stored inline.
pub fn thunk_code(arch: Arch, address: u64) -> Vec<u8> {
    let mut code = Vec::with_capacity(16);
    match arch {
        Arch::Aarch64 => {
            code.extend_from_slice(&AARCH64_LDR_X16_LITERAL);
            code.extend_from_slice(&AARCH64_BR_X16);
        }
        Arch::X86_64 => code.extend_from_slice(&X86_64_JMP_RIP_INDIRECT),
    }
    code.extend_from_slice(&address.to_le_bytes());
    code
}

fn add_thunk(obj: &mut write::Object<'_>, target: Target, name: &[u8], address: u64) {
    let code = thunk_code(target.arch, address);
    let text = obj.section_id(StandardSection::Text);
    let offset = obj.append_section_data(text, &code, 8);
    obj.add_symbol(Symbol {
        name: name.to_vec(),
        value: offset,
        size: code.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
}

/// A fresh thread-local seeded with the base's initializer. Added through
/// `add_symbol_data`, so the Mach-O writer also emits the `__thread_vars`
/// descriptor and `$tlv$init` symbol the runtime expects.
fn add_tls(
    obj: &mut write::Object<'_>,
    cache: &SymbolCache,
    target: Target,
    raw_name: &str,
    name: &[u8],
) -> Result<(), HotpatchError> {
    let init = cache
        .symbols()
        .tls_init(raw_name)
        .filter(|init| !init.is_empty())
        .ok_or_else(|| {
            HotpatchError::unsupported(format!(
                "the base image records no initializer for thread-local `{raw_name}`"
            ))
        })?
        .to_vec();
    let align = (init.len() as u64)
        .min(target.pointer_width())
        .next_power_of_two();
    let id = obj.add_symbol(Symbol {
        name: name.to_vec(),
        value: 0,
        size: 0,
        kind: SymbolKind::Tls,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Undefined,
        flags: SymbolFlags::None,
    });
    let tls = obj.section_id(StandardSection::Tls);
    obj.add_symbol_data(id, tls, &init, align);
    Ok(())
}

/// An absolute symbol at the runtime address. It keeps the base symbol's
/// type, visibility and Mach-O description; an ELF binding is rewritten to
/// global (weak for a weak base symbol), since a local copied from a linked
/// image could not satisfy the patch's reference. Android's linker rejects
/// the copied ELF flags, so it gets none.
fn add_absolute(
    obj: &mut write::Object<'_>,
    target: Target,
    symbol: &CachedSymbol,
    name: &[u8],
    address: u64,
) {
    let flags = match (target.os, symbol.flags) {
        (Os::Android, _) | (_, RawFlags::None) => SymbolFlags::None,
        (_, RawFlags::Elf { st_info, st_other }) => {
            let binding = if symbol.is_weak {
                object::elf::STB_WEAK
            } else {
                object::elf::STB_GLOBAL
            };
            SymbolFlags::Elf {
                st_info: (binding << 4) | (st_info & 0xf),
                st_other,
            }
        }
        (_, RawFlags::MachO { n_desc }) => SymbolFlags::MachO { n_desc },
    };
    obj.add_symbol(Symbol {
        name: name.to_vec(),
        value: address,
        size: 0,
        kind: match symbol.kind {
            SymbolKind::Unknown => SymbolKind::Data,
            kind => kind,
        },
        scope: SymbolScope::Linkage,
        weak: symbol.is_weak,
        section: SymbolSection::Absolute,
        flags,
    });
}

#[cfg(test)]
mod tests {
    use object::read::{File, ObjectSection};

    use super::super::symbols::fixtures::{Def, TRIPLES, object};
    use super::super::symbols::{ANCHOR_SYMBOL, ImageSymbols, TLV_INIT_SUFFIX};
    use super::*;

    const TLS_INIT: [u8; 8] = [0xef, 0xbe, 0xad, 0xde, 0x01, 0x02, 0x03, 0x04];
    const SLIDE: u64 = 0x1234_5000;

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-hotpatch-stub-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn base_cache(target: Target) -> SymbolCache {
        let bytes = object(
            target,
            &[
                Def::Text("padding_fn", 32),
                Def::Text("foo_fn", 16),
                Def::Text("foo2_fn", 16),
                Def::Text(ANCHOR_SYMBOL, 4),
                Def::Data("BAR_DATA", &[1, 2, 3, 4]),
                Def::Tls("TLS_VAR", &TLS_INIT),
                Def::Text("weak_present", 4),
                Def::Undefined("libc_import"),
            ],
        );
        SymbolCache::from_bytes("base", &bytes, target).unwrap()
    }

    /// Two patch inputs: a loose object and an rlib holding a second object
    /// plus metadata the collector must skip.
    fn patch_inputs(target: Target, dir: &Path) -> Vec<PathBuf> {
        let loose = object(
            target,
            &[
                Def::Undefined("foo_fn"),
                Def::Undefined("foo2_fn"),
                Def::Undefined("BAR_DATA"),
                Def::Undefined("TLS_VAR"),
                Def::Undefined("libc_import"),
                Def::Undefined("patched_fn"),
                Def::WeakUndefined("weak_absent"),
                Def::WeakUndefined("weak_present"),
                Def::Text("local_def", 8),
            ],
        );
        let loose_path = dir.join("tip.rcgu.o");
        std::fs::write(&loose_path, loose).unwrap();

        let member = object(target, &[Def::Text("patched_fn", 8)]);
        let rlib_path = dir.join("libdep-0123.rlib");
        let mut builder = ar::Builder::new(std::fs::File::create(&rlib_path).unwrap());
        let metadata = b"not an object";
        builder
            .append(
                &ar::Header::new(b"lib.rmeta".to_vec(), metadata.len() as u64),
                &metadata[..],
            )
            .unwrap();
        builder
            .append(
                &ar::Header::new(b"dep-0123.dep.0.rcgu.o".to_vec(), member.len() as u64),
                &member[..],
            )
            .unwrap();
        drop(builder);
        vec![loose_path, rlib_path]
    }

    fn find<'a>(file: &'a File<'a>, raw: &str) -> Option<object::read::Symbol<'a, 'a>> {
        file.symbols()
            .find(|s| s.name() == Ok(raw) && !s.is_undefined())
    }

    #[test]
    fn collect_subtracts_defined_symbols_and_reads_rlib_members() {
        let target = Target::from_triple("aarch64-linux-android").unwrap();
        let dir = temp_dir("collect");
        let undefined = UndefinedSymbols::collect(&patch_inputs(target, &dir)).unwrap();
        let strong: Vec<&str> = undefined.strong.iter().map(String::as_str).collect();
        assert_eq!(
            strong,
            ["BAR_DATA", "TLS_VAR", "foo2_fn", "foo_fn", "libc_import"]
        );
        let weak: Vec<&str> = undefined.weak.iter().map(String::as_str).collect();
        assert_eq!(weak, ["weak_absent", "weak_present"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_stub_points_every_reference_at_the_running_base() {
        for triple in TRIPLES {
            let target = Target::from_triple(triple).unwrap();
            let cache = base_cache(target);
            let dir = temp_dir("stub");
            let inputs = patch_inputs(target, &dir);
            let stub =
                create_undefined_symbol_stub(&cache, &inputs, cache.anchor_address() + SLIDE)
                    .unwrap();
            let _ = std::fs::remove_dir_all(&dir);

            let file = File::parse(&*stub).unwrap();
            assert_eq!(file.format(), target.object_format(), "{triple}");
            assert_eq!(
                file.architecture(),
                target.object_architecture(),
                "{triple}"
            );
            let raw = |name: &str| target.raw_symbol_name(name);
            let base = |name: &str| cache.symbols().defined_address(&raw(name)).unwrap();

            // One thunk per text symbol, jumping to base address + slide.
            for name in ["foo_fn", "foo2_fn", "weak_present"] {
                let thunk = find(&file, &raw(name)).unwrap_or_else(|| panic!("{triple}: {name}"));
                assert_eq!(thunk.kind(), SymbolKind::Text, "{triple}: {name}");
                let section = file
                    .section_by_index(thunk.section_index().unwrap())
                    .unwrap();
                let data = section.data().unwrap();
                let start = (thunk.address() - section.address()) as usize;
                let code = &data[start..start + 16.min(data.len() - start)];
                let expected = base(name) + SLIDE;
                match target.arch {
                    Arch::Aarch64 => {
                        assert_eq!(code[..4], AARCH64_LDR_X16_LITERAL, "{triple}");
                        assert_eq!(code[4..8], AARCH64_BR_X16, "{triple}");
                        assert_eq!(code[8..16], expected.to_le_bytes(), "{triple}: {name}");
                    }
                    Arch::X86_64 => {
                        assert_eq!(code[..6], X86_64_JMP_RIP_INDIRECT, "{triple}");
                        assert_eq!(code[6..14], expected.to_le_bytes(), "{triple}: {name}");
                    }
                }
            }

            // Data: an absolute symbol at base address + slide.
            let data = find(&file, &raw("BAR_DATA")).expect("data symbol");
            assert_eq!(data.section(), object::SymbolSection::Absolute, "{triple}");
            assert_eq!(data.address(), base("BAR_DATA") + SLIDE, "{triple}");
            assert!(data.is_global(), "{triple}");

            // TLS: a fresh thread-local holding the base's initializer.
            let tls = find(&file, &raw("TLS_VAR")).expect("TLS symbol");
            assert_eq!(tls.kind(), SymbolKind::Tls, "{triple}");
            let read_back = ImageSymbols::parse(&stub, target, "stub").unwrap();
            assert_eq!(
                read_back.tls_init(&raw("TLS_VAR")),
                Some(&TLS_INIT[..]),
                "{triple}"
            );
            if target.format() == Format::MachO {
                let init = format!("{}{TLV_INIT_SUFFIX}", raw("TLS_VAR"));
                assert!(find(&file, &init).is_some(), "{triple}: {init}");
            }

            // Imports, symbols the inputs define, and absent weak references
            // get nothing.
            for name in ["libc_import", "patched_fn", "local_def", "weak_absent"] {
                assert!(find(&file, &raw(name)).is_none(), "{triple}: {name}");
            }

            if target.format() == Format::MachO {
                // The writer re-adds the `_` the stub stripped: one prefix,
                // never two.
                assert!(find(&file, "__foo_fn").is_none(), "{triple}");
                let macho = object::read::macho::MachOFile64::<Endianness>::parse(&*stub).unwrap();
                let version = macho.build_version().unwrap().expect("LC_BUILD_VERSION");
                let endian = macho.endian();
                assert_eq!(version.platform.get(endian), object::macho::PLATFORM_MACOS);
                assert_eq!(version.minos.get(endian), MACOS_MIN_VERSION);
            }
        }
    }

    #[test]
    fn a_symbol_absent_from_the_base_refuses_the_whole_stub() {
        for triple in TRIPLES {
            let target = Target::from_triple(triple).unwrap();
            let cache = base_cache(target);
            let undefined = UndefinedSymbols {
                strong: [
                    target.raw_symbol_name("foo_fn"),
                    target.raw_symbol_name("ghost_fn"),
                ]
                .into_iter()
                .collect(),
                weak: BTreeSet::new(),
            };
            let err = build_stub(&cache, &undefined, cache.anchor_address() + SLIDE).unwrap_err();
            assert!(
                matches!(&err, HotpatchError::BuilderUnsupported { detail } if detail.contains("ghost_fn")),
                "{triple}: {err:?}"
            );
        }
    }

    #[test]
    fn a_runtime_anchor_below_the_link_anchor_is_refused() {
        let target = Target::from_triple("aarch64-apple-darwin").unwrap();
        let cache = base_cache(target);
        let err = build_stub(
            &cache,
            &UndefinedSymbols::default(),
            cache.anchor_address() - 1,
        )
        .unwrap_err();
        assert!(
            matches!(err, HotpatchError::BuilderUnsupported { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn an_unreadable_input_is_refused() {
        let target = Target::from_triple("x86_64-unknown-linux-gnu").unwrap();
        let cache = base_cache(target);
        let dir = temp_dir("unreadable");
        let input = dir.join("garbage.o");
        std::fs::write(&input, b"not an object").unwrap();
        let err =
            create_undefined_symbol_stub(&cache, &[input], cache.anchor_address()).unwrap_err();
        assert!(
            matches!(err, HotpatchError::BuilderUnsupported { .. }),
            "{err:?}"
        );
        let missing = dir.join("missing.o");
        let err =
            create_undefined_symbol_stub(&cache, &[missing], cache.anchor_address()).unwrap_err();
        assert!(matches!(err, HotpatchError::Io { .. }), "{err:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn thunks_encode_the_absolute_address() {
        let address = 0x0123_4567_89ab_cdef_u64;
        let aarch64 = thunk_code(Arch::Aarch64, address);
        assert_eq!(aarch64.len(), 16);
        assert_eq!(aarch64[8..], address.to_le_bytes());
        let x86 = thunk_code(Arch::X86_64, address);
        assert_eq!(x86.len(), 14);
        assert_eq!(x86[6..], address.to_le_bytes());
    }
}
