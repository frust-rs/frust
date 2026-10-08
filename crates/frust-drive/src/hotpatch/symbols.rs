//! The base image's symbol table, parsed once per session and kept: the
//! stub resolves every symbol the patch leaves undefined against it, and the
//! jump table maps patch symbols onto it by name.
//!
//! The parse is dioxus-cli 0.7.10's `HotpatchModuleCache` for ELF and Mach-O
//! (`build/patch.rs`), keyed on the app-owned anchor symbol instead of
//! `main`. Names are kept raw, exactly as the symbol table spells them, so a
//! Darwin name carries its leading `_` (`___frust_hotpatch_anchor`).
//!
//! TLS initializers are resolved per symbol at parse time. On ELF a TLS
//! symbol's value is its offset into its section's initialization image and
//! its size is recorded; on Mach-O the variable is a descriptor in
//! `__thread_vars` whose initializer is the `<name>$tlv$init` symbol in
//! `__thread_data`/`__thread_bss`, sized by the next initializer's address
//! because Mach-O symbols carry no size.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use object::read::{File, Object, ObjectSection, ObjectSymbol, SectionIndex};
use object::{SectionKind, SymbolKind, SymbolSection};

use super::HotpatchError;

/// The app-owned ASLR anchor (`#[unsafe(no_mangle)] pub extern "C" fn
/// __frust_hotpatch_anchor() {}`). Must equal `frust_hotpatch::ANCHOR_SYMBOL`;
/// it is restated rather than imported because tooling crates depend on no
/// framework crate.
pub const ANCHOR_SYMBOL: &str = "__frust_hotpatch_anchor";

/// The suffix LLVM gives a Mach-O thread-local variable's initializer symbol.
pub const TLV_INIT_SUFFIX: &str = "$tlv$init";

/// The object format of a supported target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Elf,
    MachO,
}

/// The architecture of a supported target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    Aarch64,
    X86_64,
}

/// The operating system of a supported target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Linux,
    Android,
}

/// A target the builder can produce stubs and jump tables for. Anything else
/// (PE, iOS, 32-bit, other architectures) is refused when the triple is
/// parsed, never guessed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub arch: Arch,
    pub os: Os,
}

impl Target {
    /// Parse a rustc target triple such as `aarch64-apple-darwin`,
    /// `x86_64-unknown-linux-gnu` or `aarch64-linux-android`.
    pub fn from_triple(triple: &str) -> Result<Self, HotpatchError> {
        let refuse = || {
            HotpatchError::unsupported(format!(
                "hot-patch target `{triple}` is not supported (ELF or Mach-O on aarch64/x86_64 only)"
            ))
        };
        let parts: Vec<&str> = triple.split('-').collect();
        let (arch, rest) = parts.split_first().ok_or_else(refuse)?;
        let arch = match *arch {
            "aarch64" | "arm64" => Arch::Aarch64,
            "x86_64" => Arch::X86_64,
            _ => return Err(refuse()),
        };
        let os = match rest {
            ["apple", "darwin"] => Os::MacOs,
            ["linux", "android"] => Os::Android,
            ["unknown", "linux", "gnu" | "musl"] => Os::Linux,
            _ => return Err(refuse()),
        };
        Ok(Self { arch, os })
    }

    /// The target's object format.
    pub fn format(self) -> Format {
        match self.os {
            Os::MacOs => Format::MachO,
            Os::Linux | Os::Android => Format::Elf,
        }
    }

    /// The prefix the platform's symbol tables put on a C-level name: `_` on
    /// Darwin, nothing on ELF.
    pub fn symbol_prefix(self) -> &'static str {
        match self.format() {
            Format::MachO => "_",
            Format::Elf => "",
        }
    }

    /// `name` as this target's symbol tables spell it.
    pub fn raw_symbol_name(self, name: &str) -> String {
        format!("{}{name}", self.symbol_prefix())
    }

    /// The anchor symbol as this target's symbol tables spell it.
    pub fn anchor_symbol(self) -> String {
        self.raw_symbol_name(ANCHOR_SYMBOL)
    }

    /// Pointer width in bytes.
    pub fn pointer_width(self) -> u64 {
        8
    }

    pub(crate) fn object_format(self) -> object::BinaryFormat {
        match self.format() {
            Format::Elf => object::BinaryFormat::Elf,
            Format::MachO => object::BinaryFormat::MachO,
        }
    }

    pub(crate) fn object_architecture(self) -> object::Architecture {
        match self.arch {
            Arch::Aarch64 => object::Architecture::Aarch64,
            Arch::X86_64 => object::Architecture::X86_64,
        }
    }

    /// Refuse an image whose format or architecture is not this target's.
    pub(crate) fn check(self, file: &File<'_>, what: &str) -> Result<(), HotpatchError> {
        if file.format() != self.object_format()
            || file.architecture() != self.object_architecture()
        {
            return Err(HotpatchError::unsupported(format!(
                "{what} is {:?}/{:?}, expected {:?}/{:?}",
                file.format(),
                file.architecture(),
                self.object_format(),
                self.object_architecture()
            )));
        }
        Ok(())
    }
}

/// Parse `bytes` as an object file, refusing an unknown format.
pub(crate) fn parse_object<'data>(
    bytes: &'data [u8],
    what: &str,
) -> Result<File<'data>, HotpatchError> {
    File::parse(bytes).map_err(|err| {
        HotpatchError::unsupported(format!("{what} is not a readable object: {err}"))
    })
}

/// Where a symbol lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Referenced but not defined here (an import, on a linked image).
    Undefined,
    /// Defined in a section.
    Section,
    /// Defined at an absolute address.
    Absolute,
    /// A common or otherwise unplaced symbol.
    Other,
}

/// The format-specific symbol flags a stub copies onto an absolute data
/// symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawFlags {
    None,
    Elf { st_info: u8, st_other: u8 },
    MachO { n_desc: u16 },
}

/// One symbol-table entry, by its raw name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CachedSymbol {
    /// Link-time address (ELF TLS: offset into the TLS image).
    pub address: u64,
    /// Size, 0 where the format records none (every Mach-O symbol).
    pub size: u64,
    pub kind: SymbolKind,
    pub placement: Placement,
    pub is_global: bool,
    pub is_weak: bool,
    pub flags: RawFlags,
}

impl CachedSymbol {
    pub fn is_undefined(&self) -> bool {
        self.placement == Placement::Undefined
    }

    /// Defined in a section of this image (not absolute, not imported).
    pub fn is_section_defined(&self) -> bool {
        self.placement == Placement::Section
    }

    /// Lower ranks win when one name appears more than once.
    fn rank(&self) -> u8 {
        match (self.placement, self.is_global) {
            (Placement::Section, true) => 0,
            (Placement::Section, false) => 1,
            (Placement::Absolute, _) => 2,
            (Placement::Other, _) => 3,
            (Placement::Undefined, _) => 4,
        }
    }
}

/// One image's symbol table: the base executable, or a patch library, whose
/// addresses the host maps a missed seam key through.
#[derive(Clone, Debug)]
pub struct ImageSymbols {
    target: Target,
    by_name: HashMap<String, CachedSymbol>,
    /// Section-defined text symbols as `(address, size, name)`, sorted by
    /// address with one name per address.
    text_by_address: Vec<(u64, u64, String)>,
    /// Initializer bytes per TLS variable, keyed by the variable's raw name.
    tls_inits: HashMap<String, Vec<u8>>,
}

impl ImageSymbols {
    /// Read and parse the image at `path`.
    pub fn load(path: &Path, target: Target) -> Result<Self, HotpatchError> {
        let bytes = std::fs::read(path)
            .map_err(|err| HotpatchError::io(format!("reading `{}`", path.display()), err))?;
        Self::parse(&bytes, target, &format!("`{}`", path.display()))
    }

    /// Parse an image's symbol table; `what` names it in errors.
    pub fn parse(bytes: &[u8], target: Target, what: &str) -> Result<Self, HotpatchError> {
        let file = parse_object(bytes, what)?;
        target.check(&file, what)?;

        let mut by_name: HashMap<String, CachedSymbol> = HashMap::new();
        for symbol in file.symbols() {
            let Ok(name) = symbol.name() else { continue };
            if name.is_empty() {
                continue;
            }
            let cached = CachedSymbol {
                address: symbol.address(),
                size: symbol.size(),
                kind: symbol.kind(),
                placement: match symbol.section() {
                    SymbolSection::Undefined => Placement::Undefined,
                    SymbolSection::Section(_) => Placement::Section,
                    SymbolSection::Absolute => Placement::Absolute,
                    _ => Placement::Other,
                },
                is_global: symbol.is_global(),
                is_weak: symbol.is_weak(),
                flags: match symbol.flags() {
                    object::SymbolFlags::Elf { st_info, st_other } => {
                        RawFlags::Elf { st_info, st_other }
                    }
                    object::SymbolFlags::MachO { n_desc } => RawFlags::MachO { n_desc },
                    _ => RawFlags::None,
                },
            };
            match by_name.get(name) {
                Some(existing) if existing.rank() <= cached.rank() => {}
                _ => {
                    by_name.insert(name.to_string(), cached);
                }
            }
        }

        let mut text_by_address: Vec<(u64, u64, String)> = by_name
            .iter()
            .filter(|(_, s)| s.kind == SymbolKind::Text && s.is_section_defined())
            .map(|(name, s)| (s.address, s.size, name.clone()))
            .collect();
        text_by_address.sort();
        text_by_address.dedup_by_key(|(address, _, _)| *address);

        let tls_inits = match target.format() {
            Format::Elf => elf_tls_inits(&file),
            Format::MachO => macho_tls_inits(&file),
        };

        Ok(Self {
            target,
            by_name,
            text_by_address,
            tls_inits,
        })
    }

    pub fn target(&self) -> Target {
        self.target
    }

    /// The entry for a raw symbol name.
    pub fn get(&self, raw_name: &str) -> Option<&CachedSymbol> {
        self.by_name.get(raw_name)
    }

    /// Every entry, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &CachedSymbol)> {
        self.by_name.iter().map(|(name, s)| (name.as_str(), s))
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Link-time address of a symbol defined in a section of this image.
    pub fn defined_address(&self, raw_name: &str) -> Option<u64> {
        self.get(raw_name)
            .filter(|s| s.is_section_defined())
            .map(|s| s.address)
    }

    /// The text symbol containing `link_address`: the nearest one at or
    /// below it, bounded by its size where the format records one.
    pub fn symbol_at(&self, link_address: u64) -> Option<&str> {
        let index = self
            .text_by_address
            .partition_point(|(address, _, _)| *address <= link_address);
        let (address, size, name) = self.text_by_address.get(index.checked_sub(1)?)?;
        if *size != 0 && link_address - address >= *size {
            return None;
        }
        Some(name)
    }

    /// The initializer bytes of the TLS variable `raw_name`, when this image
    /// records them.
    pub fn tls_init(&self, raw_name: &str) -> Option<&[u8]> {
        self.tls_inits.get(raw_name).map(Vec::as_slice)
    }
}

/// ELF: a TLS symbol's value is its offset into its section's part of the
/// initialization image (`.tdata` in a linked image, a `.tdata.*` section in
/// a relocatable object); `.tbss` variables are zero-initialized.
fn elf_tls_inits(file: &File<'_>) -> HashMap<String, Vec<u8>> {
    let mut inits = HashMap::new();
    for symbol in file.symbols() {
        if symbol.kind() != SymbolKind::Tls || symbol.size() == 0 {
            continue;
        }
        let (Ok(name), Some(index)) = (symbol.name(), symbol.section_index()) else {
            continue;
        };
        let Ok(section) = file.section_by_index(index) else {
            continue;
        };
        let size = symbol.size();
        let bytes = match section.kind() {
            SectionKind::UninitializedTls => vec![0; size as usize],
            SectionKind::Tls => {
                let Ok(data) = section.data() else { continue };
                let start = symbol.address();
                match start
                    .checked_add(size)
                    .and_then(|end| data.get(start as usize..end as usize))
                {
                    Some(bytes) => bytes.to_vec(),
                    None => continue,
                }
            }
            _ => continue,
        };
        inits.insert(name.to_string(), bytes);
    }
    inits
}

/// Mach-O: each `<name>$tlv$init` symbol in a thread-local data or zerofill
/// section runs to the next initializer in that section, or to its end.
fn macho_tls_inits(file: &File<'_>) -> HashMap<String, Vec<u8>> {
    let mut by_section: HashMap<SectionIndex, Vec<(u64, String)>> = HashMap::new();
    for symbol in file.symbols() {
        let (Ok(name), Some(index)) = (symbol.name(), symbol.section_index()) else {
            continue;
        };
        if let Some(variable) = name.strip_suffix(TLV_INIT_SUFFIX) {
            by_section
                .entry(index)
                .or_default()
                .push((symbol.address(), variable.to_string()));
        }
    }

    let mut inits = HashMap::new();
    for (index, mut entries) in by_section {
        let Ok(section) = file.section_by_index(index) else {
            continue;
        };
        let zerofill = match section.kind() {
            SectionKind::Tls => false,
            SectionKind::UninitializedTls => true,
            _ => continue,
        };
        let data = if zerofill {
            &[][..]
        } else {
            let Ok(data) = section.data() else { continue };
            data
        };
        let section_start = section.address();
        let section_end = section_start.saturating_add(section.size());
        entries.sort();
        entries.dedup_by_key(|(address, _)| *address);
        for (i, (address, variable)) in entries.iter().enumerate() {
            let end = entries.get(i + 1).map_or(section_end, |(next, _)| *next);
            let (Some(offset), Some(size)) = (
                address.checked_sub(section_start),
                end.checked_sub(*address),
            ) else {
                continue;
            };
            let bytes = if zerofill {
                vec![0; size as usize]
            } else {
                match data.get(offset as usize..(offset + size) as usize) {
                    Some(bytes) => bytes.to_vec(),
                    None => continue,
                }
            };
            inits.insert(variable.clone(), bytes);
        }
    }
    inits
}

/// The base image's symbol table plus the anchor's link-time address — the
/// session-long patch cache.
#[derive(Clone, Debug)]
pub struct SymbolCache {
    path: PathBuf,
    symbols: ImageSymbols,
    anchor_address: u64,
}

impl SymbolCache {
    /// Parse the base image at `path` once.
    pub fn load(path: &Path, target: Target) -> Result<Self, HotpatchError> {
        let bytes = std::fs::read(path)
            .map_err(|err| HotpatchError::io(format!("reading `{}`", path.display()), err))?;
        Self::from_bytes(path, &bytes, target)
    }

    /// Parse a base image already in memory; `path` names it.
    pub fn from_bytes(
        path: impl Into<PathBuf>,
        bytes: &[u8],
        target: Target,
    ) -> Result<Self, HotpatchError> {
        let path = path.into();
        let what = format!("base image `{}`", path.display());
        let symbols = ImageSymbols::parse(bytes, target, &what)?;
        let anchor = target.anchor_symbol();
        let anchor_address = symbols
            .get(&anchor)
            .filter(|s| s.is_section_defined() && s.kind == SymbolKind::Text)
            .map(|s| s.address)
            .ok_or_else(|| {
                HotpatchError::unsupported(format!(
                    "{what} defines no `{anchor}` text symbol (is the app built with hot-patching on?)"
                ))
            })?;
        Ok(Self {
            path,
            symbols,
            anchor_address,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn target(&self) -> Target {
        self.symbols.target()
    }

    /// The base image's symbol table.
    pub fn symbols(&self) -> &ImageSymbols {
        &self.symbols
    }

    /// Link-time address of the base image's anchor.
    pub fn anchor_address(&self) -> u64 {
        self.anchor_address
    }
}

/// Object builders for the hot-patch tests: tiny relocatable objects written
/// with `object` in the test itself, in each supported format.
#[cfg(test)]
pub(crate) mod fixtures {
    use object::write::{Object, StandardSection, Symbol, SymbolSection};
    use object::{Endianness, SymbolFlags, SymbolKind, SymbolScope};

    use super::Target;

    /// One triple per supported format/architecture pair.
    pub(crate) const TRIPLES: [&str; 4] = [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "aarch64-linux-android",
        "x86_64-unknown-linux-gnu",
    ];

    /// One symbol of a fixture object, by its unprefixed name (the writer
    /// adds Darwin's `_`).
    pub(crate) enum Def<'a> {
        /// A text symbol of `len` bytes.
        Text(&'a str, usize),
        Data(&'a str, &'a [u8]),
        Tls(&'a str, &'a [u8]),
        Undefined(&'a str),
        WeakUndefined(&'a str),
    }

    pub(crate) fn object(target: Target, defs: &[Def<'_>]) -> Vec<u8> {
        let mut obj = Object::new(
            target.object_format(),
            target.object_architecture(),
            Endianness::Little,
        );
        for def in defs {
            let (name, kind, section, data, weak) = match def {
                Def::Text(name, len) => (
                    *name,
                    SymbolKind::Text,
                    Some(StandardSection::Text),
                    vec![0x90; *len],
                    false,
                ),
                Def::Data(name, data) => (
                    *name,
                    SymbolKind::Data,
                    Some(StandardSection::Data),
                    data.to_vec(),
                    false,
                ),
                Def::Tls(name, data) => (
                    *name,
                    SymbolKind::Tls,
                    Some(StandardSection::Tls),
                    data.to_vec(),
                    false,
                ),
                Def::Undefined(name) => (*name, SymbolKind::Text, None, Vec::new(), false),
                Def::WeakUndefined(name) => (*name, SymbolKind::Text, None, Vec::new(), true),
            };
            let id = obj.add_symbol(Symbol {
                name: name.as_bytes().to_vec(),
                value: 0,
                size: 0,
                kind,
                scope: SymbolScope::Linkage,
                weak,
                section: SymbolSection::Undefined,
                flags: SymbolFlags::None,
            });
            if let Some(section) = section {
                let section = obj.section_id(section);
                obj.add_symbol_data(id, section, &data, 8);
            }
        }
        obj.write().expect("fixture object writes")
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{Def, TRIPLES, object};
    use super::*;

    const TLS_INIT: [u8; 8] = [0xef, 0xbe, 0xad, 0xde, 0x01, 0x02, 0x03, 0x04];
    const DATA: [u8; 4] = [1, 2, 3, 4];

    fn base(target: Target) -> Vec<u8> {
        object(
            target,
            &[
                Def::Text("padding_fn", 32),
                Def::Text("foo_fn", 16),
                Def::Text(ANCHOR_SYMBOL, 4),
                Def::Data("BAR_DATA", &DATA),
                Def::Tls("ZERO_TLS", &[0; 4]),
                Def::Tls("TLS_VAR", &TLS_INIT),
                Def::Undefined("libc_import"),
            ],
        )
    }

    #[test]
    fn triples_parse_to_targets() {
        let cases = [
            (
                "aarch64-apple-darwin",
                Arch::Aarch64,
                Os::MacOs,
                Format::MachO,
            ),
            (
                "x86_64-apple-darwin",
                Arch::X86_64,
                Os::MacOs,
                Format::MachO,
            ),
            (
                "aarch64-linux-android",
                Arch::Aarch64,
                Os::Android,
                Format::Elf,
            ),
            (
                "x86_64-linux-android",
                Arch::X86_64,
                Os::Android,
                Format::Elf,
            ),
            (
                "x86_64-unknown-linux-gnu",
                Arch::X86_64,
                Os::Linux,
                Format::Elf,
            ),
            (
                "aarch64-unknown-linux-musl",
                Arch::Aarch64,
                Os::Linux,
                Format::Elf,
            ),
        ];
        for (triple, arch, os, format) in cases {
            let target = Target::from_triple(triple).unwrap();
            assert_eq!(
                (target.arch, target.os, target.format()),
                (arch, os, format)
            );
        }
        assert_eq!(
            Target::from_triple("aarch64-apple-darwin")
                .unwrap()
                .anchor_symbol(),
            "___frust_hotpatch_anchor"
        );
        assert_eq!(
            Target::from_triple("aarch64-linux-android")
                .unwrap()
                .anchor_symbol(),
            "__frust_hotpatch_anchor"
        );
    }

    #[test]
    fn unknown_triples_are_builder_unsupported() {
        for triple in [
            "x86_64-pc-windows-msvc",
            "aarch64-apple-ios",
            "aarch64-apple-ios-sim",
            "riscv64gc-unknown-linux-gnu",
            "i686-unknown-linux-gnu",
            "armv7-linux-androideabi",
            "wasm32-unknown-unknown",
            "",
        ] {
            let err = Target::from_triple(triple).unwrap_err();
            assert!(
                matches!(err, HotpatchError::BuilderUnsupported { .. }),
                "{triple}: {err:?}"
            );
        }
    }

    #[test]
    fn cache_finds_text_data_tls_and_the_anchor_in_each_format() {
        for triple in TRIPLES {
            let target = Target::from_triple(triple).unwrap();
            let bytes = base(target);
            let cache = SymbolCache::from_bytes("base", &bytes, target).unwrap();
            let symbols = cache.symbols();
            let raw = |name: &str| target.raw_symbol_name(name);

            // The anchor's link-time address is the anchor symbol's address,
            // read independently here with `object`.
            let file = File::parse(&*bytes).unwrap();
            let anchor = file
                .symbols()
                .find(|s| s.name() == Ok(raw(ANCHOR_SYMBOL).as_str()))
                .expect("fixture has the anchor");
            assert_eq!(cache.anchor_address(), anchor.address(), "{triple}");
            assert_ne!(
                cache.anchor_address(),
                symbols.defined_address(&raw("padding_fn")).unwrap(),
                "{triple}"
            );

            let foo = symbols.get(&raw("foo_fn")).expect("text symbol");
            assert_eq!(foo.kind, SymbolKind::Text, "{triple}");
            assert!(foo.is_section_defined(), "{triple}");

            let data = symbols.get(&raw("BAR_DATA")).expect("data symbol");
            assert_eq!(data.kind, SymbolKind::Data, "{triple}");

            let tls = symbols.get(&raw("TLS_VAR")).expect("TLS symbol");
            assert_eq!(tls.kind, SymbolKind::Tls, "{triple}");
            assert_eq!(
                symbols.tls_init(&raw("TLS_VAR")),
                Some(&TLS_INIT[..]),
                "{triple}"
            );
            let zero = symbols.tls_init(&raw("ZERO_TLS")).expect("zero TLS init");
            assert!(zero.len() >= 4 && zero.iter().all(|b| *b == 0), "{triple}");

            let import = symbols.get(&raw("libc_import")).expect("import");
            assert!(import.is_undefined(), "{triple}");
            assert_eq!(symbols.defined_address(&raw("libc_import")), None);

            // Address -> name, for the host's missed-key mapping.
            let foo_at = symbols.defined_address(&raw("foo_fn")).unwrap();
            assert_eq!(symbols.symbol_at(foo_at), Some(raw("foo_fn").as_str()));
            assert_eq!(symbols.symbol_at(foo_at + 3), Some(raw("foo_fn").as_str()));
        }
    }

    #[test]
    fn a_base_without_the_anchor_is_refused() {
        for triple in TRIPLES {
            let target = Target::from_triple(triple).unwrap();
            let bytes = object(target, &[Def::Text("main", 4)]);
            let err = SymbolCache::from_bytes("base", &bytes, target).unwrap_err();
            assert!(
                matches!(&err, HotpatchError::BuilderUnsupported { detail } if detail.contains(ANCHOR_SYMBOL)),
                "{triple}: {err:?}"
            );
        }
    }

    #[test]
    fn unknown_formats_and_mismatched_targets_are_refused() {
        let darwin = Target::from_triple("aarch64-apple-darwin").unwrap();
        let err = SymbolCache::from_bytes("base", b"definitely not an object", darwin).unwrap_err();
        assert!(
            matches!(err, HotpatchError::BuilderUnsupported { .. }),
            "{err:?}"
        );

        // A Mach-O image read for an ELF target, and an x86_64 image read for
        // an aarch64 target.
        let elf = Target::from_triple("aarch64-linux-android").unwrap();
        let err = SymbolCache::from_bytes("base", &base(darwin), elf).unwrap_err();
        assert!(
            matches!(err, HotpatchError::BuilderUnsupported { .. }),
            "{err:?}"
        );
        let x86 = Target::from_triple("x86_64-apple-darwin").unwrap();
        let err = SymbolCache::from_bytes("base", &base(x86), darwin).unwrap_err();
        assert!(
            matches!(err, HotpatchError::BuilderUnsupported { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn a_missing_base_is_an_io_error() {
        let target = Target::from_triple("aarch64-apple-darwin").unwrap();
        let err = SymbolCache::load(Path::new("/nonexistent/frust-base"), target).unwrap_err();
        assert!(matches!(err, HotpatchError::Io { .. }), "{err:?}");
    }

    const PROBE_SOURCE: &str = r#"
use std::cell::Cell;
#[unsafe(no_mangle)]
pub extern "C" fn __frust_hotpatch_anchor() {}
#[unsafe(no_mangle)]
pub static mut PROBE_DATA: u64 = 0x1122_3344_5566_7788;
#[unsafe(no_mangle)]
pub extern "C" fn probe_text() -> u64 { PROBE_TLS.with(|c| c.get()) }
thread_local! { pub static PROBE_TLS: Cell<u64> = const { Cell::new(0x0bad_cafe) }; }
"#;

    /// Compile [`PROBE_SOURCE`] to an object with the workspace toolchain.
    fn rustc_probe(target: Option<&str>) -> Vec<u8> {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-hotpatch-symbols-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("probe.rs");
        let out = dir.join("probe.o");
        std::fs::write(&source, PROBE_SOURCE).unwrap();
        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
        let mut command = std::process::Command::new(rustc);
        command.args(["--edition", "2024", "--crate-type", "rlib", "--emit", "obj"]);
        if let Some(target) = target {
            command.args(["--target", target]);
        }
        let status = command.arg("-o").arg(&out).arg(&source).status().unwrap();
        assert!(status.success(), "rustc failed for {target:?}");
        let bytes = std::fs::read(&out).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        bytes
    }

    fn assert_probe(bytes: &[u8], target: Target, expect_tls: bool) {
        let cache = SymbolCache::from_bytes("probe.o", bytes, target).unwrap();
        let symbols = cache.symbols();
        assert_eq!(
            Some(cache.anchor_address()),
            symbols.defined_address(&target.anchor_symbol())
        );
        let text = symbols.get(&target.raw_symbol_name("probe_text")).unwrap();
        assert_eq!(text.kind, SymbolKind::Text);
        let data = symbols.get(&target.raw_symbol_name("PROBE_DATA")).unwrap();
        assert_eq!(data.kind, SymbolKind::Data);
        if expect_tls {
            // rustc mangles the variable; find it by kind and its
            // initializer's value.
            let init = 0x0bad_cafe_u64.to_le_bytes();
            let found = symbols.iter().any(|(name, s)| {
                s.kind == SymbolKind::Tls
                    && name.contains("PROBE_TLS")
                    && symbols
                        .tls_init(name)
                        .is_some_and(|bytes| bytes.starts_with(&init))
            });
            assert!(found, "no TLS variable seeded with the probe's initializer");
        }
    }

    #[test]
    #[cfg(any(
        all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ),
        all(
            target_os = "linux",
            any(target_arch = "aarch64", target_arch = "x86_64")
        )
    ))]
    fn cache_reads_a_rustc_built_host_object() {
        let triple = if cfg!(target_os = "macos") {
            format!("{}-apple-darwin", std::env::consts::ARCH)
        } else {
            format!("{}-unknown-linux-gnu", std::env::consts::ARCH)
        };
        let target = Target::from_triple(&triple).unwrap();
        assert_probe(&rustc_probe(None), target, true);
    }

    #[test]
    #[ignore = "needs the aarch64-linux-android rustup target; run with `cargo test -p frust-drive hotpatch::symbols -- --ignored`"]
    fn cache_reads_a_rustc_built_android_elf_object() {
        // Android's std keeps thread-locals in pthread keys, so this object
        // carries no ELF TLS; the fixtures cover ELF TLS.
        let target = Target::from_triple("aarch64-linux-android").unwrap();
        assert_probe(&rustc_probe(Some("aarch64-linux-android")), target, false);
    }
}
