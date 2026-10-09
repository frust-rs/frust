//! PE/PDB mechanics for the Windows (MSVC) builder: the base image's symbol
//! cache and a patch DLL's symbol table, both read from the image's PDB.
//!
//! A linked PE image keeps no COFF symbol table, so the entries come from
//! the PDB's global symbol stream, as in dioxus-cli 0.7.10's
//! `HotpatchModuleCache::new` and `create_windows_jump_table`
//! (`build/patch.rs`): every public (`S_PUB32`) and data (`S_*DATA32`)
//! record, keyed by name (a record with no RVA is undefined). A public
//! flagged as a function is text, anything else data. Every address is a
//! VA: the image's preferred `ImageBase` (from its PE32+ optional header,
//! [`image_base`]) plus the record's RVA, the link-time address the runtime
//! measures a slide from (`frust_hotpatch` registers a PE image's slide as
//! its load base minus its preferred `ImageBase`, as for ELF and Mach-O).
//! The anchor's VA is therefore the jump table's `aslr_reference` for the
//! fat exe and its `new_base_address` for a patch DLL; an x64 fat exe,
//! linked at its fixed base with ASLR off, has slide 0. The fat exe and
//! every patch DLL link with `/HIGHENTROPYVA:NO`
//! ([`HIGH_ENTROPY_VA_OFF`](super::fat_link::HIGH_ENTROPY_VA_OFF)).
//!
//! Unlike dx, the PDB is opened only after checking it is the image's own:
//! `<image stem>.pdb` beside the image, whose GUID and age match the
//! CodeView record in the image's debug directory. A Windows build of a
//! scaffolded app warns of a `.pdb` filename collision (its lib and bin
//! share a crate name), so a PDB found by name alone may belong to another
//! artifact. A missing, foreign or unreadable PDB is
//! [`HotpatchError::BuilderUnsupported`]. [`open_pdb`] is the one place a
//! PDB is opened; the PDB reader is a Windows-host dependency, so off
//! Windows every reader here answers `BuilderUnsupported`.
//!
//! The record-to-cache rule ([`image_symbols_from_records`]), the header
//! readers ([`codeview`], [`image_base`], [`size_of_image`]), the stub's PE
//! arms and the jump
//! table (which matches PDB-built tables by name, anchored on
//! [`ANCHOR_SYMBOL`] exactly as for ELF and Mach-O) work on every host.

use std::path::{Path, PathBuf};

use object::SymbolKind;

use super::HotpatchError;
use super::symbols::{
    ANCHOR_SYMBOL, Arch, CachedSymbol, Format, ImageSymbols, Placement, RawFlags, SymbolCache,
    Target,
};

/// `IMAGE_FILE_MACHINE_AMD64`.
pub const MACHINE_AMD64: u16 = 0x8664;
/// `IMAGE_FILE_MACHINE_ARM64`.
pub const MACHINE_ARM64: u16 = 0xaa64;

/// `IMAGE_DEBUG_TYPE_CODEVIEW`.
const DEBUG_TYPE_CODEVIEW: u32 = 2;
/// The PE32+ optional-header magic.
const PE32_PLUS_MAGIC: u16 = 0x20b;
/// `ImageBase`'s offset in a PE32+ optional header.
const IMAGE_BASE_OFFSET: usize = 24;
/// `SizeOfImage`'s offset in a PE32+ optional header.
const SIZE_OF_IMAGE_OFFSET: usize = 56;
/// The debug directory's index among the optional header's data directories.
const DEBUG_DIRECTORY_INDEX: usize = 6;
/// One `IMAGE_DEBUG_DIRECTORY` entry.
const DEBUG_ENTRY_SIZE: usize = 28;
/// One section header.
const SECTION_HEADER_SIZE: usize = 40;

/// The PDB link.exe writes beside `image` by default: `<image stem>.pdb`.
pub fn pdb_path(image: &Path) -> PathBuf {
    image.with_extension("pdb")
}

/// The architecture a PE machine field names, for the two frust ships.
pub fn machine_arch(machine: u16) -> Option<Arch> {
    match machine {
        MACHINE_AMD64 => Some(Arch::X86_64),
        MACHINE_ARM64 => Some(Arch::Aarch64),
        _ => None,
    }
}

/// What kind of PDB global-symbol record an entry came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordKind {
    /// `S_PUB32`: a linker public; `function` is its function flag.
    Public { function: bool },
    /// `S_GDATA32`/`S_LDATA32`: a data record.
    Data,
}

/// One PDB global-symbol record the cache reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdbRecord {
    pub name: String,
    /// The record's RVA; `None` when its section offset maps to none.
    pub rva: Option<u32>,
    pub kind: RecordKind,
}

/// The records of one image's PDB, the image's machine and its preferred
/// base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdbImage {
    /// The COFF machine of the image ([`MACHINE_AMD64`], [`MACHINE_ARM64`]).
    pub machine: u16,
    /// The image's preferred `ImageBase`, added to every record's RVA.
    pub image_base: u64,
    pub records: Vec<PdbRecord>,
}

/// Link markers of the MSVC CRT: a compiler references one to make the
/// linker pull in a CRT feature, and nothing reads its value. A patch's own
/// link supplies its copy (`/defaultlib:msvcrt`), so the base's is entered
/// as undefined and the stub leaves the reference to that link, as for a
/// symbol the base imports. `_fltused` is referenced by every object that
/// uses floating point, and the base's copy sits wherever the exe loaded —
/// above 4 GiB for an exe that keeps ASLR, where the stub refuses a data
/// symbol.
pub const CRT_LINK_MARKERS: &[&str] = &["_fltused"];

/// dx's cache rule over PDB records: each named record becomes an entry at
/// its VA, `image_base` plus its RVA (undefined at 0 without an RVA), text
/// when it is a public function and data otherwise. A public wins over a
/// data record of the same name. A [`CRT_LINK_MARKERS`] name is undefined.
/// `target` must be a PE target.
pub fn image_symbols_from_records(
    target: Target,
    image_base: u64,
    records: &[PdbRecord],
) -> Result<ImageSymbols, HotpatchError> {
    if target.format() != Format::Pe {
        return Err(HotpatchError::unsupported(format!(
            "PDB records describe a PE image, not a {:?} target",
            target.format()
        )));
    }
    if image_base.checked_add(u64::from(u32::MAX)).is_none() {
        return Err(HotpatchError::unsupported(format!(
            "image base {image_base:#x} leaves no room for the image's RVAs"
        )));
    }
    let entry = |record: &PdbRecord| {
        let kind = match record.kind {
            RecordKind::Public { function: true } => SymbolKind::Text,
            RecordKind::Public { function: false } | RecordKind::Data => SymbolKind::Data,
        };
        let address = record
            .rva
            .filter(|_| !CRT_LINK_MARKERS.contains(&record.name.as_str()))
            .map(|rva| image_base + u64::from(rva));
        let cached = CachedSymbol {
            address: address.unwrap_or(0),
            size: 0,
            kind,
            placement: if address.is_some() {
                Placement::Section
            } else {
                Placement::Undefined
            },
            is_global: true,
            is_weak: false,
            flags: RawFlags::None,
        };
        (record.name.clone(), cached)
    };
    let publics = records
        .iter()
        .filter(|r| matches!(r.kind, RecordKind::Public { .. }));
    let data = records.iter().filter(|r| r.kind == RecordKind::Data);
    let entries = publics
        .chain(data)
        .filter(|r| !r.name.is_empty())
        .map(entry);
    Ok(ImageSymbols::from_entries(
        target,
        entries,
        Default::default(),
    ))
}

/// The CodeView (`RSDS`) record a PE image keeps for its PDB, with the
/// image's machine and preferred base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeView {
    /// The image's COFF machine.
    pub machine: u16,
    /// The image's preferred `ImageBase` ([`image_base`]).
    pub image_base: u64,
    /// The PDB GUID as its fields: `Data1`, `Data2`, `Data3`, `Data4`.
    pub guid: (u32, u16, u16, [u8; 8]),
    pub age: u32,
    /// The PDB path the linker recorded.
    pub pdb_name: String,
}

fn le_at<const N: usize>(bytes: &[u8], offset: usize) -> Option<[u8; N]> {
    bytes.get(offset..offset.checked_add(N)?)?.try_into().ok()
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    le_at(bytes, offset).map(u16::from_le_bytes)
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    le_at(bytes, offset).map(u32::from_le_bytes)
}

fn u64_at(bytes: &[u8], offset: usize) -> Option<u64> {
    le_at(bytes, offset).map(u64::from_le_bytes)
}

/// Where a PE32+ image's headers sit, and the two fields every reader here
/// needs.
struct Headers {
    machine: u16,
    sections: usize,
    /// File offset of the optional header.
    optional: usize,
    optional_size: usize,
    image_base: u64,
}

/// Walks the DOS header, PE signature, COFF header and PE32+ optional
/// header of `bytes` (`what` names it in errors).
fn headers(bytes: &[u8], what: &str) -> Result<Headers, HotpatchError> {
    let bad =
        |why: &str| HotpatchError::unsupported(format!("{what} is not a usable PE image: {why}"));
    if bytes.get(..2) != Some(b"MZ") {
        return Err(bad("no DOS header"));
    }
    let pe = u32_at(bytes, 0x3c).ok_or_else(|| bad("truncated DOS header"))? as usize;
    if bytes.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err(bad("no PE signature"));
    }
    let coff = pe + 4;
    let truncated_coff = || bad("truncated COFF header");
    let machine = u16_at(bytes, coff).ok_or_else(truncated_coff)?;
    let sections = u16_at(bytes, coff + 2).ok_or_else(truncated_coff)? as usize;
    let optional_size = u16_at(bytes, coff + 16).ok_or_else(truncated_coff)? as usize;
    let optional = coff + 20;
    if u16_at(bytes, optional) != Some(PE32_PLUS_MAGIC) {
        return Err(bad("not a PE32+ (64-bit) image"));
    }
    let image_base = u64_at(bytes, optional + IMAGE_BASE_OFFSET)
        .ok_or_else(|| bad("truncated optional header"))?;
    Ok(Headers {
        machine,
        sections,
        optional,
        optional_size,
        image_base,
    })
}

/// The preferred `ImageBase` of the PE32+ image `bytes` (`what` names it in
/// errors): the address its RVAs are offsets from at link time, so a VA is
/// `ImageBase + RVA`. Anything out of bounds is
/// [`HotpatchError::BuilderUnsupported`].
pub fn image_base(bytes: &[u8], what: &str) -> Result<u64, HotpatchError> {
    headers(bytes, what).map(|headers| headers.image_base)
}

/// The `SizeOfImage` of the PE32+ image `bytes` (`what` names it in
/// errors): the bytes of address space the loaded image spans from its
/// `ImageBase`. Anything out of bounds is
/// [`HotpatchError::BuilderUnsupported`].
pub fn size_of_image(bytes: &[u8], what: &str) -> Result<u32, HotpatchError> {
    let headers = headers(bytes, what)?;
    u32_at(bytes, headers.optional + SIZE_OF_IMAGE_OFFSET).ok_or_else(|| {
        HotpatchError::unsupported(format!(
            "{what} is not a usable PE image: truncated optional header"
        ))
    })
}

/// Reads the CodeView record of the PE32+ image `bytes` (`what` names it in
/// errors): the headers ([`image_base`]), the debug data directory and its
/// `IMAGE_DEBUG_TYPE_CODEVIEW` entry. Anything out of bounds or missing is
/// [`HotpatchError::BuilderUnsupported`].
pub fn codeview(bytes: &[u8], what: &str) -> Result<CodeView, HotpatchError> {
    let bad =
        |why: &str| HotpatchError::unsupported(format!("{what} is not a usable PE image: {why}"));
    let u32_at = |offset: usize| u32_at(bytes, offset);
    let Headers {
        machine,
        sections,
        optional,
        optional_size,
        image_base,
    } = headers(bytes, what)?;
    let directories = u32_at(optional + 108).ok_or_else(|| bad("truncated optional header"))?;
    if (directories as usize) <= DEBUG_DIRECTORY_INDEX
        || 112 + 8 * (DEBUG_DIRECTORY_INDEX + 1) > optional_size
    {
        return Err(bad("no debug data directory"));
    }
    let debug_dir = optional + 112 + 8 * DEBUG_DIRECTORY_INDEX;
    let debug_rva = u32_at(debug_dir).ok_or_else(|| bad("truncated data directory"))?;
    let debug_size = u32_at(debug_dir + 4).ok_or_else(|| bad("truncated data directory"))? as usize;
    if debug_rva == 0 || debug_size < DEBUG_ENTRY_SIZE {
        return Err(bad("no debug directory (was it linked with /DEBUG?)"));
    }

    let section_table = optional + optional_size;
    let file_offset = |rva: u32| {
        (0..sections).find_map(|index| {
            let header = section_table + index * SECTION_HEADER_SIZE;
            let virtual_size = u32_at(header + 8)?;
            let virtual_address = u32_at(header + 12)?;
            let raw_size = u32_at(header + 16)?;
            let raw_pointer = u32_at(header + 20)?;
            let delta = rva.checked_sub(virtual_address)?;
            (delta < virtual_size.max(raw_size) && delta < raw_size)
                .then(|| raw_pointer as usize + delta as usize)
        })
    };
    let entries =
        file_offset(debug_rva).ok_or_else(|| bad("debug directory outside any section"))?;
    for index in 0..debug_size / DEBUG_ENTRY_SIZE {
        let entry = entries + index * DEBUG_ENTRY_SIZE;
        if u32_at(entry + 12).ok_or_else(|| bad("truncated debug directory"))?
            != DEBUG_TYPE_CODEVIEW
        {
            continue;
        }
        let size = u32_at(entry + 16).ok_or_else(|| bad("truncated debug directory"))? as usize;
        let at = u32_at(entry + 24).ok_or_else(|| bad("truncated debug directory"))? as usize;
        let record = at
            .checked_add(size)
            .and_then(|end| bytes.get(at..end))
            .ok_or_else(|| bad("CodeView record out of bounds"))?;
        if record.len() < 24 || &record[..4] != b"RSDS" {
            return Err(bad("CodeView record is not RSDS"));
        }
        let field =
            |offset: usize| u32::from_le_bytes(record[offset..offset + 4].try_into().unwrap());
        let mut data4 = [0u8; 8];
        data4.copy_from_slice(&record[12..20]);
        let name = &record[24..];
        let name = &name[..name.iter().position(|b| *b == 0).unwrap_or(name.len())];
        return Ok(CodeView {
            machine,
            image_base,
            guid: (
                field(4),
                u16::from_le_bytes([record[8], record[9]]),
                u16::from_le_bytes([record[10], record[11]]),
                data4,
            ),
            age: field(20),
            pdb_name: String::from_utf8_lossy(name).into_owned(),
        });
    }
    Err(bad("no CodeView debug record"))
}

/// The PE image at `image` and its own PDB, opened: `<image stem>.pdb`,
/// refused unless its GUID and age match the image's CodeView record (the
/// DBI age where the PDB records one, else the PDB-info age must be at
/// least the image's). The one place a PDB is opened: a reader of the
/// PDB's type records starts here too.
#[cfg(windows)]
pub fn open_pdb(
    image: &Path,
) -> Result<(CodeView, pdb::PDB<'static, std::fs::File>), HotpatchError> {
    let what = format!("`{}`", image.display());
    let bytes = std::fs::read(image)
        .map_err(|err| HotpatchError::io(format!("reading PE image {what}"), err))?;
    let view = codeview(&bytes, &what)?;
    drop(bytes);

    let path = pdb_path(image);
    let pdb_what = format!("PDB `{}`", path.display());
    let file = std::fs::File::open(&path).map_err(|err| {
        HotpatchError::unsupported(format!(
            "{pdb_what} for {what} cannot be opened (was it linked with /DEBUG?): {err}"
        ))
    })?;
    let pdb_error = |err: pdb::Error| HotpatchError::unsupported(format!("{pdb_what}: {err}"));
    let mut pdb = pdb::PDB::open(file).map_err(pdb_error)?;
    let info = pdb.pdb_information().map_err(pdb_error)?;
    let (data1, data2, data3, data4) = info.guid.as_fields();
    let guid = (data1, data2, data3, *data4);
    let dbi_age = pdb.debug_information().map_err(pdb_error)?.age();
    let age_matches = match dbi_age {
        Some(age) => age == view.age,
        None => info.age >= view.age,
    };
    if guid != view.guid || !age_matches {
        return Err(HotpatchError::unsupported(format!(
            "{pdb_what} is not {what}'s own PDB (GUID or age differs from the image's CodeView \
             record, which names `{}`)",
            view.pdb_name
        )));
    }
    Ok((view, pdb))
}

/// Reads the public and data records of `image`'s own PDB ([`open_pdb`]),
/// with the image's preferred base from its headers.
#[cfg(windows)]
pub fn read_pdb(image: &Path) -> Result<PdbImage, HotpatchError> {
    use pdb::FallibleIterator;

    let (view, mut pdb) = open_pdb(image)?;
    let pdb_what = format!("PDB `{}`", pdb_path(image).display());
    let pdb_error = |err: pdb::Error| HotpatchError::unsupported(format!("{pdb_what}: {err}"));
    let globals = pdb.global_symbols().map_err(pdb_error)?;
    let address_map = pdb.address_map().map_err(pdb_error)?;
    let mut records = Vec::new();
    let mut symbols = globals.iter();
    while let Some(symbol) = symbols.next().map_err(pdb_error)? {
        let (name, offset, kind) = match symbol.parse() {
            Ok(pdb::SymbolData::Public(data)) => (
                data.name,
                data.offset,
                RecordKind::Public {
                    function: data.function,
                },
            ),
            Ok(pdb::SymbolData::Data(data)) => (data.name, data.offset, RecordKind::Data),
            // Other record kinds, and kinds this reader does not know.
            _ => continue,
        };
        records.push(PdbRecord {
            name: name.to_string().into_owned(),
            rva: offset.to_rva(&address_map).map(|rva| rva.0),
            kind,
        });
    }
    Ok(PdbImage {
        machine: view.machine,
        image_base: view.image_base,
        records,
    })
}

/// Off Windows there is no PDB reader: always
/// [`HotpatchError::BuilderUnsupported`].
#[cfg(not(windows))]
pub fn read_pdb(image: &Path) -> Result<PdbImage, HotpatchError> {
    Err(HotpatchError::unsupported(format!(
        "reading the PDB of PE image `{}` needs a Windows host",
        image.display()
    )))
}

/// `image`'s symbol table from its own PDB, refused when the image's
/// machine is not `target`'s architecture.
pub fn load_image_symbols(image: &Path, target: Target) -> Result<ImageSymbols, HotpatchError> {
    let pdb = read_pdb(image)?;
    if machine_arch(pdb.machine) != Some(target.arch) {
        return Err(HotpatchError::unsupported(format!(
            "PE image `{}` is machine {:#06x}, expected {:?}",
            image.display(),
            pdb.machine,
            target.arch
        )));
    }
    image_symbols_from_records(target, pdb.image_base, &pdb.records)
}

/// The session's patch cache over the base PE image `image`, read from its
/// own PDB; the anchor's VA is its link-time address.
pub fn load_symbol_cache(image: &Path, target: Target) -> Result<SymbolCache, HotpatchError> {
    SymbolCache::from_symbols(image, load_image_symbols(image, target)?)
}

/// The VA (preferred `ImageBase` + RVA) of the [`ANCHOR_SYMBOL`] public
/// function in `image`'s own PDB.
pub fn anchor_address(image: &Path) -> Result<u64, HotpatchError> {
    anchor_in(&read_pdb(image)?, image)
}

fn anchor_in(pdb: &PdbImage, image: &Path) -> Result<u64, HotpatchError> {
    pdb.records
        .iter()
        .find_map(|record| match (record.kind, record.rva) {
            (RecordKind::Public { function: true }, Some(rva)) if record.name == ANCHOR_SYMBOL => {
                pdb.image_base.checked_add(u64::from(rva))
            }
            _ => None,
        })
        .ok_or_else(|| {
            HotpatchError::unsupported(format!(
                "the PDB of `{}` has no `{ANCHOR_SYMBOL}` public function; the app must be built \
                 with `frust::app!` and the `hotpatch` feature",
                image.display()
            ))
        })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};

    use object::read::{File, Object, ObjectSection, ObjectSymbol};

    use super::super::jump_table::create_jump_table;
    use super::super::stub::{
        IMP_PREFIX, UndefinedSymbols, build_stub, create_undefined_symbol_stub, windows_thunk_code,
    };
    use super::super::symbols::Os;
    use super::super::symbols::fixtures::{Def, object};
    use super::*;

    const X86_64: &str = "x86_64-pc-windows-msvc";
    const AARCH64: &str = "aarch64-pc-windows-msvc";
    /// The fat exe's preferred base, and an lld-linked DLL's default one.
    const EXE_BASE: u64 = super::super::fat_link::FAT_IMAGE_BASE;
    const DLL_BASE: u64 = 0x1_8000_0000;

    fn target(triple: &str) -> Target {
        Target::from_triple(triple).unwrap()
    }

    fn public(name: &str, rva: u32, function: bool) -> PdbRecord {
        PdbRecord {
            name: name.to_string(),
            rva: Some(rva),
            kind: RecordKind::Public { function },
        }
    }

    fn data(name: &str, rva: u32) -> PdbRecord {
        PdbRecord {
            name: name.to_string(),
            rva: Some(rva),
            kind: RecordKind::Data,
        }
    }

    /// A base image's records: text, data, an import slot, a data record
    /// under its debug name, and an undefined public.
    fn base_records() -> Vec<PdbRecord> {
        vec![
            data("app::BAR_DATA", 0x9000),
            public("padding_fn", 0x1000, true),
            public("foo_fn", 0x1040, true),
            public(ANCHOR_SYMBOL, 0x1080, true),
            public("old_only_fn", 0x10c0, true),
            public("BAR_DATA", 0x8000, false),
            // A data record repeating a public's name loses to the public.
            data("foo_fn", 0x7777),
            public("__imp_GetLastError", 0xa000, false),
            PdbRecord {
                name: "unplaced".to_string(),
                rva: None,
                kind: RecordKind::Public { function: true },
            },
        ]
    }

    fn base_cache(triple: &str) -> SymbolCache {
        let symbols =
            image_symbols_from_records(target(triple), EXE_BASE, &base_records()).unwrap();
        SymbolCache::from_symbols("app.exe", symbols).unwrap()
    }

    #[test]
    fn msvc_triples_are_pe_targets_and_other_windows_triples_are_refused() {
        for (triple, arch) in [(X86_64, Arch::X86_64), (AARCH64, Arch::Aarch64)] {
            let target = target(triple);
            assert_eq!((target.arch, target.os), (arch, Os::Windows), "{triple}");
            assert_eq!(target.format(), Format::Pe);
            assert_eq!(target.symbol_prefix(), "");
            assert_eq!(target.anchor_symbol(), ANCHOR_SYMBOL);
            assert_eq!(target.object_format(), object::BinaryFormat::Coff);
        }
        for triple in [
            "x86_64-pc-windows-gnu",
            "aarch64-pc-windows-gnullvm",
            "x86_64-uwp-windows-msvc",
            "i686-pc-windows-msvc",
            "thumbv7a-pc-windows-msvc",
        ] {
            assert!(
                matches!(
                    Target::from_triple(triple),
                    Err(HotpatchError::BuilderUnsupported { .. })
                ),
                "{triple}"
            );
        }
    }

    #[test]
    fn pdb_records_build_the_cache_at_their_vas_and_find_the_anchor() {
        let cache = base_cache(X86_64);
        let symbols = cache.symbols();
        assert_eq!(cache.anchor_address(), EXE_BASE + 0x1080);
        assert_eq!(cache.path(), Path::new("app.exe"));

        let foo = symbols.get("foo_fn").unwrap();
        assert_eq!(
            (foo.kind, foo.address, foo.placement),
            (SymbolKind::Text, EXE_BASE + 0x1040, Placement::Section),
            "the public wins over the same-named data record"
        );
        let bar = symbols.get("BAR_DATA").unwrap();
        assert_eq!(
            (bar.kind, bar.address),
            (SymbolKind::Data, EXE_BASE + 0x8000)
        );
        let debug_name = symbols.get("app::BAR_DATA").unwrap();
        assert_eq!(debug_name.kind, SymbolKind::Data);
        let unplaced = symbols.get("unplaced").unwrap();
        assert!(unplaced.is_undefined());
        assert_eq!(unplaced.address, 0, "no RVA, no VA");
        assert_eq!(symbols.symbol_at(EXE_BASE + 0x1044), Some("foo_fn"));
        assert_eq!(symbols.symbol_at(0x1044), None, "an RVA is not an address");

        // The anchor helper reads the same record at the same VA.
        let pdb = PdbImage {
            machine: MACHINE_AMD64,
            image_base: EXE_BASE,
            records: base_records(),
        };
        assert_eq!(
            anchor_in(&pdb, Path::new("app.exe")).unwrap(),
            cache.anchor_address()
        );

        // A DLL's records sit above its own base, past 4 GiB.
        let dll = image_symbols_from_records(target(X86_64), DLL_BASE, &base_records()).unwrap();
        assert_eq!(dll.defined_address("foo_fn"), Some(DLL_BASE + 0x1040));

        // A base with no room for a 32-bit RVA is refused, never wrapped.
        assert!(matches!(
            image_symbols_from_records(target(X86_64), u64::MAX - 0x1000, &base_records()),
            Err(HotpatchError::BuilderUnsupported { .. })
        ));
    }

    #[test]
    fn a_pdb_without_the_anchor_or_for_another_format_is_refused() {
        let records = vec![public("main", 0x1000, true)];
        let symbols = image_symbols_from_records(target(X86_64), EXE_BASE, &records).unwrap();
        let err = SymbolCache::from_symbols("app.exe", symbols).unwrap_err();
        assert!(
            matches!(&err, HotpatchError::BuilderUnsupported { detail } if detail.contains(ANCHOR_SYMBOL)),
            "{err:?}"
        );
        let pdb = PdbImage {
            machine: MACHINE_AMD64,
            image_base: EXE_BASE,
            records: vec![
                // A data public or an unplaced function is not the anchor.
                public(ANCHOR_SYMBOL, 0x1000, false),
                PdbRecord {
                    name: ANCHOR_SYMBOL.to_string(),
                    rva: None,
                    kind: RecordKind::Public { function: true },
                },
            ],
        };
        assert!(matches!(
            anchor_in(&pdb, Path::new("app.exe")),
            Err(HotpatchError::BuilderUnsupported { .. })
        ));

        let elf = Target::from_triple("x86_64-unknown-linux-gnu").unwrap();
        assert!(matches!(
            image_symbols_from_records(elf, EXE_BASE, &base_records()),
            Err(HotpatchError::BuilderUnsupported { .. })
        ));
    }

    #[test]
    fn the_pe_jump_table_carries_anchor_vas_whose_offsets_are_the_runtime_slides() {
        let cache = base_cache(X86_64);
        let patch_records = vec![
            public("bar_fn", 0x2000, true),
            public("new_only_fn", 0x2040, true),
            public(ANCHOR_SYMBOL, 0x2080, true),
            public("foo_fn", 0x20c0, true),
            public("BAR_DATA", 0x3000, false),
        ];
        let patch = image_symbols_from_records(target(X86_64), DLL_BASE, &patch_records).unwrap();
        let table = create_jump_table(&cache, &patch).unwrap();

        assert_eq!(table.aslr_reference, EXE_BASE + 0x1080);
        assert_eq!(table.new_base_address, DLL_BASE + 0x2080);
        let expected: HashMap<u64, u64> = [
            (EXE_BASE + 0x1040, DLL_BASE + 0x20c0),
            (EXE_BASE + 0x1080, DLL_BASE + 0x2080),
            (EXE_BASE + 0x8000, DLL_BASE + 0x3000),
        ]
        .into();
        assert_eq!(table.map, expected);

        // The runtime's check: each image's implied offset (its runtime
        // anchor minus the table's) must be its slide, load base minus
        // preferred `ImageBase`. The fat exe loads at its fixed base (slide
        // 0); the DLL wherever the loader put it.
        let dll_load = 0x7ffa_1230_0000_u64;
        let exe_anchor_runtime = EXE_BASE + 0x1080;
        let dll_anchor_runtime = dll_load + 0x2080;
        let old_offset = exe_anchor_runtime.wrapping_sub(table.aslr_reference);
        let new_offset = dll_anchor_runtime.wrapping_sub(table.new_base_address);
        assert_eq!((old_offset, new_offset), (0, dll_load - DLL_BASE));
        let rebased: HashMap<u64, u64> = table
            .map
            .iter()
            .map(|(old, new)| (old + old_offset, new + new_offset))
            .collect();
        assert_eq!(
            rebased.get(&(EXE_BASE + 0x1040)),
            Some(&(dll_load + 0x20c0))
        );
    }

    #[test]
    fn windows_thunks_are_dx_bytes_for_both_architectures() {
        let address = 0x0123_4567_89ab_cdef_u64;
        let x86 = windows_thunk_code(Arch::X86_64, address);
        let mut expected = vec![0x48, 0xb8];
        expected.extend_from_slice(&address.to_le_bytes());
        expected.extend_from_slice(&[0xff, 0xe0]);
        assert_eq!(x86, expected, "movabs rax, imm64; jmp rax");

        let arm = windows_thunk_code(Arch::Aarch64, address);
        let words: Vec<u32> = arm
            .chunks(4)
            .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
            .collect();
        assert_eq!(
            words,
            [
                0xd280_0010 | (0xcdef << 5), // movz x16, #0xcdef
                0xf2a0_0010 | (0x89ab << 5), // movk x16, #0x89ab, lsl #16
                0xf2c0_0010 | (0x4567 << 5), // movk x16, #0x4567, lsl #32
                0xf2e0_0010 | (0x0123 << 5), // movk x16, #0x0123, lsl #48
                0xd61f_0200,                 // br x16
            ]
        );
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-hotpatch-pe-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn find<'a>(file: &'a File<'a>, name: &str) -> Option<object::read::Symbol<'a, 'a>> {
        file.symbols()
            .find(|s| s.name() == Ok(name) && !s.is_undefined())
    }

    /// The 8 bytes a defined data symbol points at.
    fn slot_value(file: &File<'_>, name: &str) -> u64 {
        let symbol = find(file, name).unwrap_or_else(|| panic!("no `{name}`"));
        assert_eq!(symbol.kind(), SymbolKind::Data, "{name}");
        let section = file
            .section_by_index(symbol.section_index().unwrap())
            .unwrap();
        let data = section.data().unwrap();
        let start = (symbol.address() - section.address()) as usize;
        u64::from_le_bytes(data[start..start + 8].try_into().unwrap())
    }

    #[test]
    fn imp_references_become_data_slots_and_text_becomes_windows_thunks() {
        const SLIDE: u64 = 0x1_4000_0000;
        for triple in [X86_64, AARCH64] {
            let target = target(triple);
            let cache = base_cache(triple);
            let dir = temp_dir("stub");
            let input = dir.join("tip.rcgu.o");
            std::fs::write(
                &input,
                object(
                    target,
                    &[
                        Def::Undefined("__imp_BAR_DATA"),
                        Def::Undefined("__imp_foo_fn"),
                        Def::Undefined("__imp_GetLastError"),
                        Def::Undefined("foo_fn"),
                        Def::Text("patched_fn", 8),
                    ],
                ),
            )
            .unwrap();
            let stub =
                create_undefined_symbol_stub(&cache, &[input], cache.anchor_address() + SLIDE)
                    .unwrap();
            let _ = std::fs::remove_dir_all(&dir);

            let file = File::parse(&*stub).unwrap();
            assert_eq!(file.format(), object::BinaryFormat::Coff, "{triple}");
            assert_eq!(file.architecture(), target.object_architecture());

            // `__imp_X` is an 8-byte data slot holding X's runtime address.
            assert_eq!(
                slot_value(&file, "__imp_BAR_DATA"),
                EXE_BASE + 0x8000 + SLIDE
            );
            assert_eq!(slot_value(&file, "__imp_foo_fn"), EXE_BASE + 0x1040 + SLIDE);
            // The base imports GetLastError itself: the patch's own link
            // resolves it through the import libraries.
            assert!(find(&file, "__imp_GetLastError").is_none(), "{triple}");
            assert!(find(&file, "GetLastError").is_none(), "{triple}");
            assert!(find(&file, "patched_fn").is_none(), "{triple}");

            // A direct text reference is a Windows thunk.
            let thunk = find(&file, "foo_fn").expect("thunk");
            assert_eq!(thunk.kind(), SymbolKind::Text);
            let section = file
                .section_by_index(thunk.section_index().unwrap())
                .unwrap();
            let start = (thunk.address() - section.address()) as usize;
            let expected = windows_thunk_code(target.arch, EXE_BASE + 0x1040 + SLIDE);
            assert_eq!(
                &section.data().unwrap()[start..start + expected.len()],
                &expected[..],
                "{triple}"
            );
        }
    }

    #[test]
    fn unresolvable_imports_and_wide_data_addresses_refuse_the_stub() {
        let cache = base_cache(X86_64);
        let anchor = cache.anchor_address();
        let refused = |undefined: &[&str], runtime: u64| {
            let undefined = UndefinedSymbols {
                strong: undefined.iter().map(|s| s.to_string()).collect(),
                weak: BTreeSet::new(),
            };
            match build_stub(&cache, &undefined, runtime) {
                Err(HotpatchError::BuilderUnsupported { detail }) => detail,
                other => panic!("expected a refusal, got {other:?}"),
            }
        };

        let detail = refused(&[&format!("{IMP_PREFIX}ghost")], anchor + 0x1000);
        assert!(detail.contains("__imp_ghost"), "{detail}");

        // A directly referenced data symbol is a COFF absolute symbol, which
        // holds 32 bits: fine below 4 GiB, refused above rather than cut.
        let low = UndefinedSymbols {
            strong: ["BAR_DATA".to_string()].into(),
            weak: BTreeSet::new(),
        };
        // `object` reports no address for a COFF absolute symbol: read the
        // raw value.
        use object::read::coff::ImageSymbol as _;
        let absolute = |stub: &[u8]| {
            let coff = object::read::coff::CoffFile::<&[u8]>::parse(stub).unwrap();
            let bar = coff.symbols().find(|s| s.name() == Ok("BAR_DATA")).unwrap();
            assert_eq!(bar.section(), object::SymbolSection::Absolute);
            bar.coff_symbol().value()
        };
        // At the fixed base (slide 0) the runtime address is the VA itself.
        let stub = build_stub(&cache, &low, anchor).unwrap();
        assert_eq!(u64::from(absolute(&stub)), EXE_BASE + 0x8000);
        // Relocated a little, still below 4 GiB.
        let stub = build_stub(&cache, &low, anchor + 0x10_0000).unwrap();
        assert_eq!(u64::from(absolute(&stub)), EXE_BASE + 0x8000 + 0x10_0000);
        let detail = refused(&["BAR_DATA"], anchor + 0x1_4000_0000);
        assert!(detail.contains("32 bits"), "{detail}");

        // A weak `__imp_` reference the base cannot resolve is left out.
        let weak = UndefinedSymbols {
            strong: BTreeSet::new(),
            weak: ["__imp_ghost".to_string()].into(),
        };
        let stub = build_stub(&cache, &weak, anchor).unwrap();
        assert!(find(&File::parse(&*stub).unwrap(), "__imp_ghost").is_none());
    }

    #[test]
    fn a_crt_link_marker_is_left_to_the_patch_link_wherever_the_base_loaded() {
        let mut records = base_records();
        records.push(public("_fltused", 0x9100, false));
        let symbols = image_symbols_from_records(target(X86_64), EXE_BASE, &records).unwrap();
        let cache = SymbolCache::from_symbols("app.exe", symbols).unwrap();
        assert!(cache.symbols().get("_fltused").unwrap().is_undefined());

        // A base loaded above 4 GiB: the marker does not refuse the stub,
        // and the stub does not define it; real data still refuses.
        let runtime = cache.anchor_address() + 0x7ff7_0000_0000;
        let marker = UndefinedSymbols {
            strong: ["_fltused".to_string()].into(),
            weak: BTreeSet::new(),
        };
        let stub = build_stub(&cache, &marker, runtime).unwrap();
        assert!(find(&File::parse(&*stub).unwrap(), "_fltused").is_none());
        let data = UndefinedSymbols {
            strong: ["BAR_DATA".to_string(), "_fltused".to_string()].into(),
            weak: BTreeSet::new(),
        };
        assert!(matches!(
            build_stub(&cache, &data, runtime),
            Err(HotpatchError::BuilderUnsupported { detail }) if detail.contains("32 bits")
        ));
    }

    /// The preferred base [`pe_image`] records.
    const FIXTURE_BASE: u64 = 0x1_4000_0000;
    /// The `SizeOfImage` [`pe_image`] records.
    const FIXTURE_SIZE: u32 = 0x2000;

    /// A minimal PE32+ image at [`FIXTURE_BASE`]: one `.rdata` section
    /// holding a debug directory whose CodeView record names `app.pdb`.
    fn pe_image(machine: u16, guid: [u8; 16], age: u32) -> Vec<u8> {
        let mut image = vec![0u8; 0x400];
        let put16 = |image: &mut Vec<u8>, at: usize, v: u16| {
            image[at..at + 2].copy_from_slice(&v.to_le_bytes())
        };
        let put32 = |image: &mut Vec<u8>, at: usize, v: u32| {
            image[at..at + 4].copy_from_slice(&v.to_le_bytes())
        };
        image[..2].copy_from_slice(b"MZ");
        put32(&mut image, 0x3c, 0x40);
        image[0x40..0x44].copy_from_slice(b"PE\0\0");
        put16(&mut image, 0x44, machine);
        put16(&mut image, 0x46, 1); // one section
        put16(&mut image, 0x54, 240); // PE32+ optional header, 16 directories
        let optional = 0x58;
        put16(&mut image, optional, PE32_PLUS_MAGIC);
        image[optional + 24..optional + 32].copy_from_slice(&FIXTURE_BASE.to_le_bytes());
        put32(&mut image, optional + 56, FIXTURE_SIZE);
        put32(&mut image, optional + 108, 16);
        put32(&mut image, optional + 112 + 8 * 6, 0x1000); // debug directory RVA
        put32(&mut image, optional + 112 + 8 * 6 + 4, 28);
        let section = optional + 240;
        image[section..section + 6].copy_from_slice(b".rdata");
        put32(&mut image, section + 8, 0x200); // virtual size
        put32(&mut image, section + 12, 0x1000); // virtual address
        put32(&mut image, section + 16, 0x200); // raw size
        put32(&mut image, section + 20, 0x200); // raw pointer
        let entry = 0x200;
        put32(&mut image, entry + 12, DEBUG_TYPE_CODEVIEW);
        put32(&mut image, entry + 16, 24 + 8);
        put32(&mut image, entry + 20, 0x1020);
        put32(&mut image, entry + 24, 0x220);
        image[0x220..0x224].copy_from_slice(b"RSDS");
        image[0x224..0x234].copy_from_slice(&guid);
        put32(&mut image, 0x234, age);
        image[0x238..0x240].copy_from_slice(b"app.pdb\0");
        image
    }

    #[test]
    fn codeview_reads_the_pdb_identity_and_refuses_what_is_not_a_pe_image() {
        let guid = [
            0x78, 0x56, 0x34, 0x12, 0xbc, 0x9a, 0xf0, 0xde, 1, 2, 3, 4, 5, 6, 7, 8,
        ];
        let image = pe_image(MACHINE_ARM64, guid, 3);
        let view = codeview(&image, "app.exe").unwrap();
        assert_eq!(
            view,
            CodeView {
                machine: MACHINE_ARM64,
                image_base: FIXTURE_BASE,
                guid: (0x1234_5678, 0x9abc, 0xdef0, [1, 2, 3, 4, 5, 6, 7, 8]),
                age: 3,
                pdb_name: "app.pdb".to_string(),
            }
        );
        assert_eq!(machine_arch(view.machine), Some(Arch::Aarch64));
        assert_eq!(machine_arch(MACHINE_AMD64), Some(Arch::X86_64));
        assert_eq!(machine_arch(0x14c), None);
        assert_eq!(pdb_path(Path::new("fat/app.exe")), Path::new("fat/app.pdb"));

        let mut no_debug = pe_image(MACHINE_AMD64, guid, 1);
        no_debug[0x58 + 112 + 48..0x58 + 112 + 56].fill(0);
        let mut truncated = pe_image(MACHINE_AMD64, guid, 1);
        truncated.truncate(0x230);
        let mut pe32 = pe_image(MACHINE_AMD64, guid, 1);
        pe32[0x58] = 0x0b;
        pe32[0x59] = 0x01;
        // The base needs only the headers, not a debug directory.
        assert_eq!(image_base(&image, "app.exe").unwrap(), FIXTURE_BASE);
        assert_eq!(image_base(&no_debug, "app.exe").unwrap(), FIXTURE_BASE);
        assert_eq!(size_of_image(&image, "app.exe").unwrap(), FIXTURE_SIZE);
        assert_eq!(size_of_image(&no_debug, "app.exe").unwrap(), FIXTURE_SIZE);
        let mut short_size = image.clone();
        short_size.truncate(0x58 + 58);
        assert_eq!(image_base(&short_size, "app.exe").unwrap(), FIXTURE_BASE);
        let mut short_header = image.clone();
        short_header.truncate(0x58 + 28);
        for (case, bytes) in [
            ("garbage", b"definitely not a PE image".to_vec()),
            ("no debug directory", no_debug),
            ("truncated", truncated),
            ("PE32", pe32.clone()),
            ("short optional header", short_header.clone()),
        ] {
            assert!(
                matches!(
                    codeview(&bytes, "app.exe"),
                    Err(HotpatchError::BuilderUnsupported { .. })
                ),
                "{case}"
            );
        }
        for (case, bytes) in [
            ("garbage", b"definitely not a PE image".to_vec()),
            ("PE32", pe32),
            ("short optional header", short_header),
        ] {
            assert!(
                matches!(
                    image_base(&bytes, "app.exe"),
                    Err(HotpatchError::BuilderUnsupported { .. })
                ),
                "image_base: {case}"
            );
            assert!(
                matches!(
                    size_of_image(&bytes, "app.exe"),
                    Err(HotpatchError::BuilderUnsupported { .. })
                ),
                "size_of_image: {case}"
            );
        }
        assert!(matches!(
            size_of_image(&short_size, "app.exe"),
            Err(HotpatchError::BuilderUnsupported { .. })
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn off_windows_every_pdb_reader_is_builder_unsupported() {
        let image = Path::new("C:/app/app.exe");
        for err in [
            read_pdb(image).unwrap_err(),
            anchor_address(image).unwrap_err(),
            load_symbol_cache(image, target(X86_64)).unwrap_err(),
            super::super::fat_link::anchor_address(
                super::super::fat_link::LinkerFlavor::Msvc,
                image,
            )
            .unwrap_err(),
        ] {
            assert!(
                matches!(&err, HotpatchError::BuilderUnsupported { detail } if detail.contains("Windows host")),
                "{err:?}"
            );
        }
    }

    /// The generic loaders hand a PE target's image to the PDB reader rather
    /// than `object`, which reads no linked PE image here.
    #[cfg(not(windows))]
    #[test]
    fn off_windows_the_generic_loaders_route_a_pe_image_to_the_pdb_reader() {
        let image = Path::new("C:/app/app.exe");
        for err in [
            SymbolCache::load(image, target(X86_64)).unwrap_err(),
            ImageSymbols::load(image, target(AARCH64)).unwrap_err(),
        ] {
            assert!(
                matches!(&err, HotpatchError::BuilderUnsupported { detail } if detail.contains("Windows host")),
                "{err:?}"
            );
        }
    }

    /// Fixture builds with the pinned toolchain, under the test binary's own
    /// target dir (built binaries run in place there).
    #[cfg(windows)]
    mod windows {
        use super::*;

        const BIN_SOURCE: &str = r#"
#[unsafe(no_mangle)]
pub extern "C" fn __frust_hotpatch_anchor() {}
#[unsafe(no_mangle)]
pub static PROBE_DATA: u64 = 0x1122_3344_5566_7788;
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn probe_text() -> u64 { std::hint::black_box(PROBE_DATA) }
fn main() {
    let anchor = __frust_hotpatch_anchor as usize;
    let text = probe_text as usize;
    let data = &raw const PROBE_DATA as usize;
    println!("{anchor} {text} {data} {}", probe_text());
}
"#;

        const DLL_SOURCE: &str = r#"
#[unsafe(no_mangle)]
pub extern "C" fn __frust_hotpatch_anchor() {}
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn probe_text() -> u64 { 42 }
"#;

        fn fixture_dir(tag: &str) -> PathBuf {
            static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let exe = std::env::current_exe().unwrap();
            let dir = exe.parent().unwrap().parent().unwrap().join(format!(
                "frust-hotpatch-pe-{tag}-{}-{n}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        /// Builds `source` as `crate_type` named `name` into `dir` with full
        /// debug info, so link.exe writes `<name>.pdb` beside it.
        fn rustc(dir: &Path, name: &str, crate_type: &str, source: &str) -> PathBuf {
            let src = dir.join(format!("{name}.rs"));
            std::fs::write(&src, source).unwrap();
            let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
            let output = std::process::Command::new(rustc)
                .args([
                    "--edition",
                    "2024",
                    "--crate-type",
                    crate_type,
                    "--crate-name",
                    name,
                ])
                .args(["-Cdebuginfo=2", "-Copt-level=0", "--out-dir"])
                .arg(dir)
                .arg(&src)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "rustc {name}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let ext = if crate_type == "bin" { "exe" } else { "dll" };
            let image = dir.join(format!("{name}.{ext}"));
            assert!(image.is_file() && pdb_path(&image).is_file(), "{image:?}");
            image
        }

        fn host() -> Target {
            target(if cfg!(target_arch = "aarch64") {
                AARCH64
            } else {
                X86_64
            })
        }

        /// The image's preferred base read from its headers, and its PDB's
        /// anchor RVA: the two halves of the anchor's VA.
        fn base_and_anchor_rva(image: &Path) -> (u64, u64) {
            let base = image_base(&std::fs::read(image).unwrap(), "fixture").unwrap();
            let pdb = read_pdb(image).unwrap();
            assert_eq!(pdb.image_base, base, "{image:?}");
            let rva = pdb
                .records
                .iter()
                .find(|r| {
                    r.name == ANCHOR_SYMBOL && r.kind == RecordKind::Public { function: true }
                })
                .and_then(|r| r.rva)
                .expect("the anchor's RVA");
            (base, u64::from(rva))
        }

        #[test]
        fn a_rustc_built_exe_pdb_feeds_the_cache_at_the_image_vas() {
            let dir = fixture_dir("cache");
            let exe = rustc(&dir, "pe_probe", "bin", BIN_SOURCE);
            let cache = load_symbol_cache(&exe, host()).unwrap();
            let (base, anchor_rva) = base_and_anchor_rva(&exe);
            assert_eq!(cache.anchor_address(), base + anchor_rva, "{base:#x}");
            // The generic loader reads the same PDB.
            let generic = SymbolCache::load(&exe, host()).unwrap();
            assert_eq!(generic.anchor_address(), cache.anchor_address());
            assert_eq!(generic.symbols().len(), cache.symbols().len());
            let symbols = cache.symbols();
            let text = symbols.get("probe_text").expect("probe_text public");
            assert_eq!(text.kind, SymbolKind::Text);
            // The static's linkage-name public, or failing that its data
            // record under the debug name.
            let (data_name, data) = symbols
                .iter()
                .filter(|(name, s)| name.ends_with("PROBE_DATA") && s.is_section_defined())
                .min_by_key(|(name, _)| name.len())
                .unwrap_or_else(|| {
                    let names: Vec<&str> = symbols
                        .iter()
                        .map(|(name, _)| name)
                        .filter(|name| name.contains("PROBE") || name.contains("probe"))
                        .collect();
                    panic!("no PROBE_DATA record; probe names: {names:?}")
                });
            assert_eq!(data.kind, SymbolKind::Data, "{data_name}");
            assert_eq!(anchor_address(&exe).unwrap(), cache.anchor_address());
            assert_eq!(
                super::super::super::fat_link::anchor_address(
                    super::super::super::fat_link::LinkerFlavor::Msvc,
                    &exe
                )
                .unwrap(),
                cache.anchor_address()
            );

            // The running image puts the two functions as far apart as their
            // PDB VAs: the cache's addresses are the image's own.
            let run = std::process::Command::new(&exe).output().unwrap();
            assert!(run.status.success(), "{run:?}");
            let stdout = String::from_utf8(run.stdout).unwrap();
            let fields: Vec<u64> = stdout
                .split_whitespace()
                .map(|f| f.parse().unwrap())
                .collect();
            let (anchor_rt, text_rt, data_rt) = (fields[0], fields[1], fields[2]);
            assert_eq!(fields[3], 0x1122_3344_5566_7788);
            assert_eq!(
                text_rt.wrapping_sub(anchor_rt),
                text.address.wrapping_sub(cache.anchor_address()),
                "runtime distance equals PDB VA distance"
            );
            assert_eq!(
                data_rt.wrapping_sub(anchor_rt),
                data.address.wrapping_sub(cache.anchor_address()),
                "{data_name}: runtime distance equals PDB VA distance"
            );
            // The slide is load base minus preferred base, the runtime's:
            // both 64 KiB-aligned.
            let slide = anchor_rt.wrapping_sub(cache.anchor_address());
            assert_eq!(slide & 0xffff, 0, "a 64 KiB-aligned slide: {slide:#x}");
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_patch_dll_pdb_maps_onto_the_base_by_name() {
            let dir = fixture_dir("dll");
            let exe = rustc(&dir, "pe_base", "bin", BIN_SOURCE);
            let dll = rustc(&dir, "pe_patch", "cdylib", DLL_SOURCE);
            let cache = load_symbol_cache(&exe, host()).unwrap();
            let patch = load_image_symbols(&dll, host()).unwrap();
            let generic = ImageSymbols::load(&dll, host()).unwrap();
            assert_eq!(
                generic.defined_address("probe_text"),
                patch.defined_address("probe_text")
            );
            let table = create_jump_table(&cache, &patch).unwrap();
            let old = cache.symbols().defined_address("probe_text").unwrap();
            let new = patch.defined_address("probe_text").unwrap();
            assert_eq!(table.map.get(&old), Some(&new));
            assert_eq!(table.aslr_reference, cache.anchor_address());
            assert_eq!(table.new_base_address, anchor_address(&dll).unwrap());
            // Both anchors are VAs: preferred base plus RVA.
            let (exe_base, exe_rva) = base_and_anchor_rva(&exe);
            let (dll_base, dll_rva) = base_and_anchor_rva(&dll);
            assert_eq!(table.aslr_reference, exe_base + exe_rva);
            assert_eq!(table.new_base_address, dll_base + dll_rva);
            assert!(dll_base > u64::from(u32::MAX), "a DLL base: {dll_base:#x}");
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_missing_or_foreign_pdb_or_another_machine_is_refused() {
            let dir = fixture_dir("foreign");
            let first = rustc(&dir, "pe_first", "bin", BIN_SOURCE);
            let second = rustc(&dir, "pe_second", "bin", BIN_SOURCE);

            // An image of the other architecture is refused by machine.
            let other = target(if cfg!(target_arch = "aarch64") {
                X86_64
            } else {
                AARCH64
            });
            assert!(matches!(
                load_symbol_cache(&first, other),
                Err(HotpatchError::BuilderUnsupported { .. })
            ));

            // `second`'s PDB replaced by `first`'s: right name, wrong GUID.
            std::fs::copy(pdb_path(&first), pdb_path(&second)).unwrap();
            let err = load_symbol_cache(&second, host()).unwrap_err();
            assert!(
                matches!(&err, HotpatchError::BuilderUnsupported { detail } if detail.contains("own PDB")),
                "{err:?}"
            );

            std::fs::remove_file(pdb_path(&first)).unwrap();
            let err = anchor_address(&first).unwrap_err();
            assert!(
                matches!(&err, HotpatchError::BuilderUnsupported { detail } if detail.contains("/DEBUG")),
                "{err:?}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
