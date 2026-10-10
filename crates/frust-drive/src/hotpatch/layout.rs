//! L3, the host-side structural gate: DWARF layout fingerprints of the types
//! a replayed crate defines or instantiates, compared with the session's
//! accepted-layout set before a patch is offered.
//!
//! **Table.** [`extract`] reads the DWARF of replayed objects (an rlib's
//! `.o` members, or loose objects) and records one [`LayoutEntry`] per
//! struct, enum or union whose path lies in a replayable crate, and per
//! instantiation of an external generic over such a type. Closure
//! environments are DWARF structs named `{closure_env#N}` under their
//! function's path, so they are covered too. An entry holds the byte size,
//! the alignment and an FNV-1a 64 hash over the members `(name, offset,
//! type)`, the variant parts and the enumerators, recursing into every
//! by-value member type: a change anywhere inside a value changes the hash
//! of every type that holds it. The fat build's table seeds the accepted set
//! and is saved as [`LAYOUT_BASE_FILE`].
//!
//! **Gate.** [`diff`] reports each type present in both a candidate and the
//! accepted set with a different hash as a [`LayoutChanged`]; a type absent
//! from the set is new and passes. [`merge`] adds a candidate's new entries
//! and never removes or replaces one, so a type dropped by one patch and
//! re-added with another layout by a later one is still refused. Comparison
//! is by type path, deliberately a superset of what the seam reaches (a
//! trait-object child is invisible in DWARF): editing a type with no live
//! values also restarts, a false positive that is accepted.
//!
//! **Units.** An object is read as a whole: a type reference may name a DIE
//! in another compile unit of the same object (`DW_FORM_ref_addr`), which
//! is what an object compiled at `opt-level >= 1` holds once rustc's
//! crate-local ThinLTO has imported functions across codegen units — the
//! case of a dependency built under `[profile.dev.package."*"]`. DIEs are
//! keyed by their `.debug_info` offset ([`DieRef`]).
//!
//! **Fail closed.** Only Mach-O and ELF objects are read. An input with no
//! object, an object without `.debug_info`, or debug info without types
//! (`debug = "line-tables-only"` or `"limited"`) is
//! [`HotpatchError::BuilderUnsupported`]: a gate with nothing to compare
//! would pass every edit. Under cargo's default dev profile the DWARF sits in
//! each rlib member (on macOS the default `split-debuginfo = "unpacked"`
//! leaves it in the `.o` files rather than a `.dSYM`). See
//! `docs/CLI_ARCHITECTURE.md`.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};

use gimli::{
    AttributeValue, DebugInfoOffset, DebuggingInformationEntry, Reader, RunTimeEndian, Unit,
    UnitOffset,
};
use object::read::RelocationMap;
use object::read::archive::ArchiveFile;
use object::{
    BinaryFormat, Object as _, ObjectSection as _, ObjectSymbol as _, RelocationTarget, SymbolKind,
};
use serde::{Deserialize, Serialize};

use super::HotpatchError;

/// The fat build's table, written to the session directory.
pub const LAYOUT_BASE_FILE: &str = "layout-base.json";

/// One type's layout fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutEntry {
    /// `DW_AT_byte_size`.
    pub size: u64,
    /// `DW_AT_alignment`, 0 when the DWARF states none.
    pub align: u64,
    /// 16 lowercase hex digits over the size, alignment, members, variant
    /// parts and enumerators, recursing into by-value member types.
    pub hash: String,
}

/// Layout fingerprints keyed by type path, e.g. `my_app::HomeState` or
/// `frust_core::component::ComponentWidget<my_app::HomePage>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutTable {
    pub types: BTreeMap<String, LayoutEntry>,
}

impl LayoutTable {
    pub fn len(&self) -> usize {
        self.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    pub fn get(&self, type_path: &str) -> Option<&LayoutEntry> {
        self.types.get(type_path)
    }

    /// Pretty JSON with a trailing newline; the same table always renders
    /// the same bytes (the map is ordered).
    pub fn to_json(&self) -> String {
        let mut json = serde_json::to_string_pretty(self).unwrap_or_default();
        json.push('\n');
        json
    }

    /// Writes [`to_json`](Self::to_json) to `path`.
    pub fn write(&self, path: &Path) -> Result<(), HotpatchError> {
        std::fs::write(path, self.to_json())
            .map_err(|err| HotpatchError::io(format!("writing {}", path.display()), err))
    }

    /// Writes the table as `<session_dir>/`[`LAYOUT_BASE_FILE`] and returns
    /// that path.
    pub fn write_base(&self, session_dir: &Path) -> Result<PathBuf, HotpatchError> {
        std::fs::create_dir_all(session_dir)
            .map_err(|err| HotpatchError::io(format!("creating {}", session_dir.display()), err))?;
        let path = session_dir.join(LAYOUT_BASE_FILE);
        self.write(&path)?;
        Ok(path)
    }

    /// Reads a table written by [`write`](Self::write).
    pub fn read(path: &Path) -> Result<Self, HotpatchError> {
        let text = std::fs::read_to_string(path)
            .map_err(|err| HotpatchError::io(format!("reading {}", path.display()), err))?;
        serde_json::from_str(&text).map_err(|err| {
            HotpatchError::unsupported(format!("{} is not a layout table: {err}", path.display()))
        })
    }
}

/// A type whose layout differs between a candidate and the accepted set:
/// the session's `RestartRequired { LayoutChanged { .. } }` reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutChanged {
    pub type_path: String,
    pub old_size: u64,
    pub new_size: u64,
}

impl fmt::Display for LayoutChanged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.old_size == self.new_size {
            write!(
                f,
                "{} changed layout ({} bytes, members moved)",
                self.type_path, self.old_size
            )
        } else {
            write!(
                f,
                "{} changed layout ({} → {} bytes)",
                self.type_path, self.old_size, self.new_size
            )
        }
    }
}

/// Where an object's DWARF was read: the evidence that the build carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DwarfSource {
    /// The input path, with the archive member in parentheses for an rlib.
    pub object: String,
    /// `Mach-O` or `ELF`.
    pub format: &'static str,
    /// The section `.debug_info` was found in, e.g. `__DWARF,__debug_info`.
    pub section: String,
    /// That section's size in bytes.
    pub bytes: u64,
}

/// [`extract`]'s result: the table and where its DWARF came from.
#[derive(Debug, Clone, Default)]
pub struct Extraction {
    pub table: LayoutTable,
    pub sources: Vec<DwarfSource>,
}

/// Builds the layout table of `inputs` (rlibs or objects) for the types of
/// `crates` (crate names; `-` is read as `_`).
pub fn extract(inputs: &[PathBuf], crates: &[String]) -> Result<Extraction, HotpatchError> {
    let crates: Vec<String> = crates.iter().map(|name| name.replace('-', "_")).collect();
    let mut found: BTreeMap<String, BTreeSet<Composite>> = BTreeMap::new();
    let mut sources = Vec::new();
    for input in inputs {
        let mut type_dies = 0usize;
        for_each_object(input, |label, file| {
            let (source, dies) = read_object(label, file, &crates, &mut found)?;
            sources.push(source);
            type_dies += dies;
            Ok(())
        })?;
        if type_dies == 0 {
            return Err(HotpatchError::unsupported(format!(
                "{} carries DWARF without type information; the layout gate needs \
                 `debug = true` (full debug info)",
                input.display()
            )));
        }
    }
    let types = found
        .into_iter()
        .map(|(path, layouts)| (path, combine(&layouts)))
        .collect();
    Ok(Extraction {
        table: LayoutTable { types },
        sources,
    })
}

/// Whether any object in `path` (an rlib's `.o` members, or one object)
/// holds code or data. A crate compiled to nothing on this target (a
/// platform shell whose every item is `cfg`'d out) holds neither, so it
/// has no value of any type, and carries no DWARF to read either. An input
/// that yields no object, or an object that is not Mach-O or ELF, is
/// [`HotpatchError::BuilderUnsupported`].
pub fn holds_code_or_data(path: &Path) -> Result<bool, HotpatchError> {
    use object::SectionKind;
    let mut found = false;
    for_each_object(path, |_, file| {
        found |= file.sections().any(|section| {
            section.size() > 0
                && matches!(
                    section.kind(),
                    SectionKind::Text
                        | SectionKind::Data
                        | SectionKind::ReadOnlyData
                        | SectionKind::ReadOnlyDataWithRel
                        | SectionKind::ReadOnlyString
                        | SectionKind::UninitializedData
                        | SectionKind::Common
                        | SectionKind::Tls
                        | SectionKind::UninitializedTls
                        | SectionKind::TlsVariables
                )
        });
        Ok(())
    })?;
    Ok(found)
}

/// Every type present in both tables whose hash differs, ordered by path.
pub fn diff(candidate: &LayoutTable, accepted: &LayoutTable) -> Vec<LayoutChanged> {
    candidate
        .types
        .iter()
        .filter_map(|(path, new)| {
            let old = accepted.types.get(path)?;
            (old.hash != new.hash).then(|| LayoutChanged {
                type_path: path.clone(),
                old_size: old.size,
                new_size: new.size,
            })
        })
        .collect()
}

/// Adds the candidate's new types to the accepted set. An existing entry is
/// never removed or replaced: values of every accepted layout may be live.
pub fn merge(accepted: &mut LayoutTable, candidate: &LayoutTable) {
    for (path, entry) in &candidate.types {
        accepted
            .types
            .entry(path.clone())
            .or_insert_with(|| entry.clone());
    }
}

/// True when `path` names a type of one of `crates`, or an instantiation
/// that mentions one (`alloc::vec::Vec<my_app::Item, alloc::alloc::Global>`).
pub fn in_scope(path: &str, crates: &[String]) -> bool {
    crates.iter().any(|name| {
        path.match_indices(name.as_str()).any(|(at, _)| {
            let starts = at == 0
                || !path[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':');
            starts && path[at + name.len()..].starts_with("::")
        })
    })
}

/// Calls `visit` with every object in `path`: the file itself, or each `.o`
/// member of an archive (an rlib's other members are metadata). An input
/// that yields no object, or an object that is not Mach-O or ELF, is
/// [`HotpatchError::BuilderUnsupported`].
pub(super) fn for_each_object(
    path: &Path,
    mut visit: impl FnMut(&str, &object::File<'_>) -> Result<(), HotpatchError>,
) -> Result<(), HotpatchError> {
    let data = std::fs::read(path)
        .map_err(|err| HotpatchError::io(format!("reading {}", path.display()), err))?;
    let mut count = 0usize;
    if data.starts_with(b"!<arch>\n") || data.starts_with(b"!<thin>\n") {
        let archive = ArchiveFile::parse(&*data).map_err(|err| {
            HotpatchError::unsupported(format!(
                "{} is not a readable archive: {err}",
                path.display()
            ))
        })?;
        for member in archive.members() {
            let member = member.map_err(|err| {
                HotpatchError::unsupported(format!("{}: bad archive member: {err}", path.display()))
            })?;
            let name = String::from_utf8_lossy(member.name()).into_owned();
            if !name.ends_with(".o") {
                continue;
            }
            let label = format!("{}({name})", path.display());
            let bytes = member.data(&*data).map_err(|err| {
                HotpatchError::unsupported(format!("{label}: unreadable member: {err}"))
            })?;
            let file = parse_object(&label, bytes)?;
            visit(&label, &file)?;
            count += 1;
        }
    } else {
        let label = path.display().to_string();
        let file = parse_object(&label, &data)?;
        visit(&label, &file)?;
        count += 1;
    }
    if count == 0 {
        return Err(HotpatchError::unsupported(format!(
            "{} holds no object file",
            path.display()
        )));
    }
    Ok(())
}

fn parse_object<'data>(
    label: &str,
    data: &'data [u8],
) -> Result<object::File<'data>, HotpatchError> {
    let file = object::File::parse(data).map_err(|err| {
        HotpatchError::unsupported(format!("{label} is not an object file: {err}"))
    })?;
    match file.format() {
        BinaryFormat::MachO | BinaryFormat::Elf => Ok(file),
        other => Err(HotpatchError::unsupported(format!(
            "{label} is {other:?}; the DWARF gates read Mach-O and ELF only"
        ))),
    }
}

/// One fingerprinted type as found in one object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Composite {
    hash: u64,
    size: u64,
    align: u64,
}

/// The entry for a path found once or more. Every object normally agrees;
/// if two disagree, the entry hashes all of them, so it still compares
/// equal only to a build that shows the same set.
fn combine(layouts: &BTreeSet<Composite>) -> LayoutEntry {
    let mut iter = layouts.iter();
    let first = iter.next().copied().unwrap_or(Composite {
        hash: 0,
        size: 0,
        align: 0,
    });
    if layouts.len() == 1 {
        return LayoutEntry {
            size: first.size,
            align: first.align,
            hash: format!("{:016x}", first.hash),
        };
    }
    let mut hasher = Fnv1a64::new();
    hasher.write(b"multi");
    for layout in layouts {
        hasher.write(&layout.hash.to_le_bytes());
    }
    LayoutEntry {
        size: layouts.iter().map(|l| l.size).max().unwrap_or(0),
        align: layouts.iter().map(|l| l.align).max().unwrap_or(0),
        hash: format!("{:016x}", hasher.finish()),
    }
}

/// FNV-1a 64: stable across processes and toolchains, unlike std's hasher.
struct Fnv1a64(u64);

impl Fnv1a64 {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// A DWARF read failure: gimli's own, or a shape the gate does not handle.
enum ReadError {
    Gimli(gimli::Error),
    Shape(String),
}

impl From<gimli::Error> for ReadError {
    fn from(err: gimli::Error) -> Self {
        Self::Gimli(err)
    }
}

impl ReadError {
    fn into_hotpatch(self, label: &str) -> HotpatchError {
        match self {
            Self::Gimli(err) => HotpatchError::unsupported(format!("{label}: bad DWARF: {err}")),
            Self::Shape(detail) => HotpatchError::unsupported(format!("{label}: {detail}")),
        }
    }
}

/// A section's bytes and, for ELF, the relocations its DWARF offsets need
/// (`.debug_str` references are relocated in a relocatable ELF object; a
/// Mach-O object stores them resolved).
struct LoadedSection<'data> {
    data: Cow<'data, [u8]>,
    relocations: RelocationMap,
}

#[derive(Debug, Clone, Copy)]
struct SectionRelocations<'a>(&'a RelocationMap);

impl gimli::read::Relocate for SectionRelocations<'_> {
    fn relocate_address(&self, offset: usize, value: u64) -> gimli::Result<u64> {
        Ok(self.0.relocate(offset as u64, value))
    }

    fn relocate_offset(&self, offset: usize, value: usize) -> gimli::Result<usize> {
        <usize as gimli::ReaderOffset>::from_u64(self.0.relocate(offset as u64, value as u64))
    }
}

/// The relocation map of one ELF section, built entry by entry. A
/// relocation the map cannot apply is skipped only when it targets a
/// thread-local symbol: that can only feed a TLS `DW_AT_location`
/// expression (`R_X86_64_DTPOFF64`, `R_AARCH64_TLS_DTPREL*`), which the
/// layout gate never reads. Any other failure refuses the section.
fn elf_relocation_map<'data>(
    file: &object::File<'data>,
    section: &object::Section<'data, '_>,
) -> object::read::Result<RelocationMap> {
    let mut map = RelocationMap::default();
    for (offset, relocation) in section.relocations() {
        let target = relocation.target();
        if let Err(err) = map.add(file, offset, relocation) {
            let thread_local = match target {
                RelocationTarget::Symbol(index) => file
                    .symbol_by_index(index)
                    .is_ok_and(|symbol| symbol.kind() == SymbolKind::Tls),
                _ => false,
            };
            if !thread_local {
                return Err(err);
            }
        }
    }
    Ok(map)
}

/// Reads one object's DWARF into `found`; returns where it was read and how
/// many type DIEs it holds.
fn read_object(
    label: &str,
    file: &object::File<'_>,
    crates: &[String],
    found: &mut BTreeMap<String, BTreeSet<Composite>>,
) -> Result<(DwarfSource, usize), HotpatchError> {
    let (format, is_elf) = match file.format() {
        BinaryFormat::Elf => ("ELF", true),
        _ => ("Mach-O", false),
    };
    let info = file
        .section_by_name(".debug_info")
        .filter(|section| section.size() > 0)
        .ok_or_else(|| {
            HotpatchError::unsupported(format!(
                "{label} carries no DWARF (no `.debug_info`); the layout gate needs \
                 `debug = true` and the DWARF left in the objects"
            ))
        })?;
    let section = match info.segment_name() {
        Ok(Some(segment)) => format!("{segment},{}", info.name().unwrap_or("?")),
        _ => info.name().unwrap_or("?").to_string(),
    };
    let source = DwarfSource {
        object: label.to_string(),
        format,
        section,
        bytes: info.size(),
    };
    let sections = gimli::DwarfSections::load(|id| -> Result<LoadedSection<'_>, HotpatchError> {
        let Some(section) = file.section_by_name(id.name()) else {
            return Ok(LoadedSection {
                data: Cow::Borrowed(&[]),
                relocations: RelocationMap::default(),
            });
        };
        let data = section.uncompressed_data().map_err(|err| {
            HotpatchError::unsupported(format!("{label}: unreadable {}: {err}", id.name()))
        })?;
        let relocations = if is_elf {
            elf_relocation_map(file, &section).map_err(|err| {
                HotpatchError::unsupported(format!(
                    "{label}: unsupported relocation in {}: {err}",
                    id.name()
                ))
            })?
        } else {
            RelocationMap::default()
        };
        Ok(LoadedSection { data, relocations })
    })?;
    let endian = if file.is_little_endian() {
        RunTimeEndian::Little
    } else {
        RunTimeEndian::Big
    };
    let dwarf = sections.borrow(|section| {
        gimli::RelocateReader::new(
            gimli::EndianSlice::new(&section.data, endian),
            SectionRelocations(&section.relocations),
        )
    });
    let type_dies = read_units(&dwarf, crates, found).map_err(|err| err.into_hotpatch(label))?;
    Ok((source, type_dies))
}

/// A DIE's offset in the object's `.debug_info`: unique across its units,
/// so a reference into another unit resolves like a unit-local one.
type DieRef = DebugInfoOffset<usize>;

fn read_units<R: Reader<Offset = usize>>(
    dwarf: &gimli::Dwarf<R>,
    crates: &[String],
    found: &mut BTreeMap<String, BTreeSet<Composite>>,
) -> Result<usize, ReadError> {
    let mut units = Vec::new();
    let mut headers = dwarf.units();
    while let Some(header) = headers.next()? {
        units.push(dwarf.unit(header)?);
    }
    let mut walk = Walk::default();
    for unit in &units {
        let mut tree = unit.entries_tree(None)?;
        walk_node(dwarf, unit, tree.root()?, &mut Vec::new(), &mut walk)?;
    }
    let mut fingerprints = Fingerprints {
        dwarf,
        units: &units,
        paths: &walk.paths,
        composites: HashMap::new(),
        refs: HashMap::new(),
        active: HashSet::new(),
    };
    for (offset, path) in &walk.roots {
        if !in_scope(path, crates) {
            continue;
        }
        let composite = fingerprints.composite(*offset)?;
        found.entry(path.clone()).or_default().insert(composite);
    }
    Ok(walk.type_dies)
}

/// The first pass over an object's units: every composite type's path, and
/// the defined (sized, non-declaration) ones to fingerprint.
#[derive(Default)]
struct Walk {
    paths: HashMap<DieRef, String>,
    roots: Vec<(DieRef, String)>,
    type_dies: usize,
}

/// `offset` of `unit` as a [`DieRef`]; a unit outside `.debug_info` (a
/// DWARF 4 type unit) has none, a shape the gate does not read.
fn die_ref<R: Reader<Offset = usize>>(
    unit: &Unit<R>,
    offset: UnitOffset,
) -> Result<DieRef, ReadError> {
    offset
        .to_debug_info_offset(&unit.header)
        .ok_or_else(|| ReadError::Shape("a type DIE outside `.debug_info`".to_string()))
}

fn walk_node<R: Reader<Offset = usize>>(
    dwarf: &gimli::Dwarf<R>,
    unit: &Unit<R>,
    node: gimli::EntriesTreeNode<'_, '_, '_, R>,
    stack: &mut Vec<String>,
    walk: &mut Walk,
) -> Result<(), ReadError> {
    let entry = node.entry();
    let tag = entry.tag();
    let mut pushed = false;
    match tag {
        gimli::DW_TAG_namespace => {
            stack.push(name_of(dwarf, unit, entry)?.unwrap_or_else(|| "{anon}".to_string()));
            pushed = true;
        }
        gimli::DW_TAG_structure_type
        | gimli::DW_TAG_union_type
        | gimli::DW_TAG_enumeration_type => {
            walk.type_dies += 1;
            let name = name_of(dwarf, unit, entry)?.unwrap_or_else(|| "{anon}".to_string());
            let path = qualify(stack, &name);
            let declaration = flag(entry, gimli::DW_AT_declaration)?;
            let sized = udata(entry, gimli::DW_AT_byte_size)?.is_some();
            let at = die_ref(unit, entry.offset())?;
            walk.paths.insert(at, path.clone());
            if sized && !declaration {
                walk.roots.push((at, path));
            }
            stack.push(name);
            pushed = true;
        }
        gimli::DW_TAG_base_type => walk.type_dies += 1,
        _ => {}
    }
    let mut children = node.children();
    while let Some(child) = children.next()? {
        walk_node(dwarf, unit, child, stack, walk)?;
    }
    if pushed {
        stack.pop();
    }
    Ok(())
}

fn qualify(stack: &[String], name: &str) -> String {
    if stack.is_empty() {
        name.to_string()
    } else {
        format!("{}::{name}", stack.join("::"))
    }
}

/// A composite's direct content, collected before recursing into types.
enum Item {
    Member {
        name: String,
        offset: Option<u64>,
        ty: Option<DieRef>,
    },
    Discriminant {
        offset: Option<u64>,
        ty: Option<DieRef>,
    },
    Variant {
        value: Option<String>,
    },
    Enumerator {
        name: String,
        value: String,
    },
}

/// A composite DIE's own attributes and direct content.
struct Collected {
    tag: gimli::DwTag,
    size: u64,
    align: u64,
    underlying: Option<DieRef>,
    items: Vec<Item>,
}

/// The second pass over an object: memoised fingerprints of composites and
/// descriptors of referenced types, in whichever of its units they are.
struct Fingerprints<'a, R: Reader<Offset = usize>> {
    dwarf: &'a gimli::Dwarf<R>,
    /// The object's units, in `.debug_info` order.
    units: &'a [Unit<R>],
    paths: &'a HashMap<DieRef, String>,
    composites: HashMap<DieRef, Composite>,
    refs: HashMap<DieRef, String>,
    active: HashSet<DieRef>,
}

impl<'a, R: Reader<Offset = usize>> Fingerprints<'a, R> {
    fn path(&self, offset: DieRef) -> &str {
        self.paths.get(&offset).map_or("{anon}", String::as_str)
    }

    /// The unit holding `at`, and `at` within it. A reference outside every
    /// unit is a shape the gate does not read.
    fn locate(&self, at: DieRef) -> Result<(&'a Unit<R>, UnitOffset), ReadError> {
        let units: &'a [Unit<R>] = self.units;
        units
            .iter()
            .find_map(|unit| at.to_unit_offset(&unit.header).map(|offset| (unit, offset)))
            .ok_or_else(|| {
                ReadError::Shape(format!(
                    "type reference {:#x} lies in no compile unit of the object",
                    at.0
                ))
            })
    }

    fn composite(&mut self, offset: DieRef) -> Result<Composite, ReadError> {
        if let Some(composite) = self.composites.get(&offset) {
            return Ok(*composite);
        }
        if !self.active.insert(offset) {
            return Err(ReadError::Shape(format!(
                "type `{}` contains itself by value",
                self.path(offset)
            )));
        }
        let Collected {
            tag,
            size,
            align,
            underlying,
            items,
        } = self.collect(offset)?;
        let mut canon = format!("{}|{size}|{align}|", tag.0);
        if let Some(underlying) = underlying {
            let desc = self.type_ref(Some(underlying))?;
            let _ = write!(canon, "u:{desc}|");
        }
        for item in items {
            match item {
                Item::Member { name, offset, ty } => {
                    let desc = self.type_ref(ty)?;
                    let _ = write!(canon, "m:{name}@{}:{desc}|", opt(offset));
                }
                Item::Discriminant { offset, ty } => {
                    let desc = self.type_ref(ty)?;
                    let _ = write!(canon, "d@{}:{desc}|", opt(offset));
                }
                Item::Variant { value } => {
                    let _ = write!(canon, "v:{}|", value.as_deref().unwrap_or("default"));
                }
                Item::Enumerator { name, value } => {
                    let _ = write!(canon, "e:{name}={value}|");
                }
            }
        }
        let mut hasher = Fnv1a64::new();
        hasher.write(canon.as_bytes());
        let composite = Composite {
            hash: hasher.finish(),
            size,
            align,
        };
        self.active.remove(&offset);
        self.composites.insert(offset, composite);
        Ok(composite)
    }

    fn collect(&self, offset: DieRef) -> Result<Collected, ReadError> {
        let (unit, local) = self.locate(offset)?;
        let mut tree = unit.entries_tree(Some(local))?;
        let root = tree.root()?;
        let entry = root.entry();
        let tag = entry.tag();
        let size = udata(entry, gimli::DW_AT_byte_size)?.unwrap_or(0);
        let align = udata(entry, gimli::DW_AT_alignment)?.unwrap_or(0);
        let underlying = if tag == gimli::DW_TAG_enumeration_type {
            type_of(unit, entry)?
        } else {
            None
        };
        let mut items = Vec::new();
        let mut children = root.children();
        while let Some(child) = children.next()? {
            let entry = child.entry();
            match entry.tag() {
                gimli::DW_TAG_member => items.push(self.member(unit, entry)?),
                gimli::DW_TAG_enumerator => items.push(Item::Enumerator {
                    name: self.name(unit, entry)?,
                    value: constant(entry, gimli::DW_AT_const_value)?
                        .unwrap_or_else(|| "?".to_string()),
                }),
                gimli::DW_TAG_variant_part => {
                    let mut parts = child.children();
                    while let Some(part) = parts.next()? {
                        let entry = part.entry();
                        match entry.tag() {
                            gimli::DW_TAG_member => items.push(Item::Discriminant {
                                offset: member_offset(entry)?,
                                ty: type_of(unit, entry)?,
                            }),
                            gimli::DW_TAG_variant => {
                                items.push(Item::Variant {
                                    value: constant(entry, gimli::DW_AT_discr_value)?,
                                });
                                let mut members = part.children();
                                while let Some(member) = members.next()? {
                                    if member.entry().tag() == gimli::DW_TAG_member {
                                        items.push(self.member(unit, member.entry())?);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(Collected {
            tag,
            size,
            align,
            underlying,
            items,
        })
    }

    fn member(
        &self,
        unit: &Unit<R>,
        entry: &DebuggingInformationEntry<'_, '_, R>,
    ) -> Result<Item, ReadError> {
        Ok(Item::Member {
            name: self.name(unit, entry)?,
            offset: member_offset(entry)?,
            ty: type_of(unit, entry)?,
        })
    }

    fn name(
        &self,
        unit: &Unit<R>,
        entry: &DebuggingInformationEntry<'_, '_, R>,
    ) -> Result<String, ReadError> {
        Ok(name_of(self.dwarf, unit, entry)?.unwrap_or_else(|| "{anon}".to_string()))
    }

    /// A referenced type: a composite by path and fingerprint, a pointer by
    /// its own name and size (its pointee has its own entry), anything else
    /// by its shape.
    fn type_ref(&mut self, offset: Option<DieRef>) -> Result<String, ReadError> {
        let Some(offset) = offset else {
            return Ok("void".to_string());
        };
        if let Some(desc) = self.refs.get(&offset) {
            return Ok(desc.clone());
        }
        let (unit, local) = self.locate(offset)?;
        let entry = unit.entry(local)?;
        let tag = entry.tag();
        let size = udata(&entry, gimli::DW_AT_byte_size)?;
        let desc = match tag {
            gimli::DW_TAG_structure_type
            | gimli::DW_TAG_union_type
            | gimli::DW_TAG_enumeration_type => {
                let composite = self.composite(offset)?;
                format!("{}#{:016x}", self.path(offset), composite.hash)
            }
            gimli::DW_TAG_base_type => {
                let encoding = match entry.attr_value(gimli::DW_AT_encoding)? {
                    Some(AttributeValue::Encoding(encoding)) => encoding.0,
                    _ => 0,
                };
                format!("b:{}:{}:{encoding}", self.name(unit, &entry)?, opt(size))
            }
            gimli::DW_TAG_pointer_type
            | gimli::DW_TAG_reference_type
            | gimli::DW_TAG_rvalue_reference_type
            | gimli::DW_TAG_ptr_to_member_type => {
                format!("p:{}:{}", self.name(unit, &entry)?, opt(size))
            }
            gimli::DW_TAG_array_type => {
                let element = type_of(unit, &entry)?;
                let counts = self.array_counts(unit, local)?;
                let element = self.type_ref(element)?;
                format!("a:[{element};{counts}]")
            }
            gimli::DW_TAG_typedef
            | gimli::DW_TAG_const_type
            | gimli::DW_TAG_volatile_type
            | gimli::DW_TAG_atomic_type
            | gimli::DW_TAG_restrict_type => {
                let inner = type_of(unit, &entry)?;
                format!("q{}:{}", tag.0, self.type_ref(inner)?)
            }
            _ => format!("t{}:{}:{}", tag.0, self.name(unit, &entry)?, opt(size)),
        };
        self.refs.insert(offset, desc.clone());
        Ok(desc)
    }

    fn array_counts(&self, unit: &Unit<R>, offset: UnitOffset) -> Result<String, ReadError> {
        let mut tree = unit.entries_tree(Some(offset))?;
        let root = tree.root()?;
        let mut counts = Vec::new();
        let mut children = root.children();
        while let Some(child) = children.next()? {
            let entry = child.entry();
            if entry.tag() == gimli::DW_TAG_subrange_type {
                let count = match constant(entry, gimli::DW_AT_count)? {
                    Some(count) => count,
                    None => match constant(entry, gimli::DW_AT_upper_bound)? {
                        Some(upper) => format!("..={upper}"),
                        None => "?".to_string(),
                    },
                };
                counts.push(count);
            }
        }
        Ok(counts.join(","))
    }
}

fn opt(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_string(), |v| v.to_string())
}

fn name_of<R: Reader<Offset = usize>>(
    dwarf: &gimli::Dwarf<R>,
    unit: &Unit<R>,
    entry: &DebuggingInformationEntry<'_, '_, R>,
) -> Result<Option<String>, ReadError> {
    match entry.attr_value(gimli::DW_AT_name)? {
        None => Ok(None),
        Some(value) => {
            let name = dwarf.attr_string(unit, value)?;
            Ok(Some(name.to_string_lossy()?.into_owned()))
        }
    }
}

fn udata<R: Reader<Offset = usize>>(
    entry: &DebuggingInformationEntry<'_, '_, R>,
    attr: gimli::DwAt,
) -> Result<Option<u64>, ReadError> {
    Ok(entry
        .attr_value(attr)?
        .and_then(|value| value.udata_value()))
}

fn flag<R: Reader<Offset = usize>>(
    entry: &DebuggingInformationEntry<'_, '_, R>,
    attr: gimli::DwAt,
) -> Result<bool, ReadError> {
    Ok(matches!(
        entry.attr_value(attr)?,
        Some(AttributeValue::Flag(true))
    ))
}

/// A constant attribute as text: unsigned, signed, or a block in hex.
fn constant<R: Reader<Offset = usize>>(
    entry: &DebuggingInformationEntry<'_, '_, R>,
    attr: gimli::DwAt,
) -> Result<Option<String>, ReadError> {
    let Some(value) = entry.attr_value(attr)? else {
        return Ok(None);
    };
    if let Some(unsigned) = value.udata_value() {
        return Ok(Some(unsigned.to_string()));
    }
    if let Some(signed) = value.sdata_value() {
        return Ok(Some(signed.to_string()));
    }
    match value {
        AttributeValue::Block(block) => Ok(Some(hex(&block.to_slice()?).replace(' ', ""))),
        other => Err(ReadError::Shape(format!(
            "constant attribute {attr} has an unexpected form {}",
            form(&other)
        ))),
    }
}

/// A member's offset: a constant, or the `DW_OP_plus_uconst N` expression
/// some targets emit for it (the Android ELF objects do).
fn member_offset<R: Reader<Offset = usize>>(
    entry: &DebuggingInformationEntry<'_, '_, R>,
) -> Result<Option<u64>, ReadError> {
    let Some(value) = entry.attr_value(gimli::DW_AT_data_member_location)? else {
        return Ok(None);
    };
    if let Some(offset) = value.udata_value() {
        return Ok(Some(offset));
    }
    if let AttributeValue::Exprloc(expression) = &value {
        let mut ops = expression.0.clone();
        if ops.read_u8()? == gimli::DW_OP_plus_uconst.0 {
            let offset = ops.read_uleb128()?;
            if ops.is_empty() {
                return Ok(Some(offset));
            }
        }
        let bytes = expression.0.to_slice()?;
        return Err(ReadError::Shape(format!(
            "member location expression [{}] is not a constant offset",
            hex(&bytes)
        )));
    }
    Err(ReadError::Shape(format!(
        "member location in form {} is not a constant offset",
        form(&value)
    )))
}

/// An attribute value's form, without the reader it borrows.
fn form<R: Reader<Offset = usize>>(value: &AttributeValue<R>) -> String {
    let debug = format!("{value:?}");
    debug.split('(').next().unwrap_or("?").to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `entry`'s `DW_AT_type` (of `unit`) as a [`DieRef`]: a unit-local
/// reference, or a `DW_FORM_ref_addr` one into any unit of the object. Any
/// other form (a type unit's signature, a supplementary file) is a shape
/// the gate does not read.
fn type_of<R: Reader<Offset = usize>>(
    unit: &Unit<R>,
    entry: &DebuggingInformationEntry<'_, '_, R>,
) -> Result<Option<DieRef>, ReadError> {
    match entry.attr_value(gimli::DW_AT_type)? {
        None => Ok(None),
        Some(AttributeValue::UnitRef(offset)) => die_ref(unit, offset).map(Some),
        Some(AttributeValue::DebugInfoRef(offset)) => Ok(Some(offset)),
        Some(other) => Err(ReadError::Shape(format!(
            "type reference in form {} is not within the object's `.debug_info`",
            form(&other)
        ))),
    }
}

#[cfg(all(test, not(windows)))]
pub(super) mod fixture {
    //! Builds the fixture workspace in `tests/fixtures/hotpatch` with the
    //! toolchain building these tests (the workspace pin), once per edit
    //! per process, through cargo's default dev profile.

    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use crate::process::{ProcessRunner, RealProcessRunner};

    /// The replayable crate of the fixture.
    pub const APP_CRATE: &str = "hotpatch_fixture_app";

    /// One fixture build: an edit (a cargo feature of the app, `None` for
    /// the fat build), a target triple (`None` for the host) and an
    /// override of the dev profile's `debug` setting.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Spec {
        pub edit: Option<&'static str>,
        pub target: Option<&'static str>,
        pub debug: Option<&'static str>,
    }

    impl Spec {
        pub fn edit(edit: &'static str) -> Self {
            Self {
                edit: Some(edit),
                ..Self::default()
            }
        }
    }

    static BUILT: Mutex<Option<HashMap<String, PathBuf>>> = Mutex::new(None);

    /// The app rlib of the host's fat build.
    pub fn base() -> PathBuf {
        build(Spec::default())
    }

    /// The app rlib of `edit`'s host build.
    pub fn edited(edit: &'static str) -> PathBuf {
        build(Spec::edit(edit))
    }

    /// The crates the gate reads, as the session passes them.
    pub fn crates() -> Vec<String> {
        vec![APP_CRATE.to_string()]
    }

    /// The app rlib `spec` builds. Panics with cargo's output on failure.
    pub fn build(spec: Spec) -> PathBuf {
        let key = format!("{spec:?}");
        let mut built = BUILT.lock().unwrap_or_else(|poison| poison.into_inner());
        let built = built.get_or_insert_with(HashMap::new);
        if let Some(path) = built.get(&key) {
            return path.clone();
        }
        let root = std::env::temp_dir().join("frust-drive-hotpatch-fixtures");
        sync_sources(&fixture_dir(), &root);
        let path = cargo_build(&root, spec);
        built.insert(key, path.clone());
        path
    }

    /// The pinned toolchain's rustc beside the cargo building these tests.
    pub fn toolchain() -> (String, Option<PathBuf>) {
        let cargo = option_env!("CARGO").unwrap_or("cargo").to_string();
        let rustc =
            Path::new(&cargo).with_file_name(if cfg!(windows) { "rustc.exe" } else { "rustc" });
        (cargo, rustc.is_file().then_some(rustc))
    }

    /// True when the toolchain has the standard library for `target`.
    pub fn has_target(target: &str) -> bool {
        let (_, rustc) = toolchain();
        let rustc = rustc.map_or_else(|| "rustc".to_string(), |p| p.display().to_string());
        let Ok(out) =
            RealProcessRunner.run(&rustc, &["--print", "target-libdir", "--target", target])
        else {
            return false;
        };
        out.success
            && std::fs::read_dir(out.stdout.trim()).is_ok_and(|entries| {
                entries
                    .flatten()
                    .any(|e| e.file_name().to_string_lossy().starts_with("libcore-"))
            })
    }

    fn fixture_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hotpatch")
    }

    /// Mirrors the fixture sources into `root`, rewriting only files whose
    /// content changed (atomically), so cargo's fingerprints stay fresh
    /// across runs and a concurrent build never reads a half-written file.
    fn sync_sources(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            if name == "target" || name == "Cargo.lock" {
                continue;
            }
            let (src, dst) = (entry.path(), to.join(&name));
            if entry.file_type().unwrap().is_dir() {
                sync_sources(&src, &dst);
                continue;
            }
            let content = std::fs::read(&src).unwrap();
            if std::fs::read(&dst).ok().as_deref() == Some(content.as_slice()) {
                continue;
            }
            let tmp = to.join(format!(
                ".{}.{}",
                name.to_string_lossy(),
                std::process::id()
            ));
            std::fs::write(&tmp, &content).unwrap();
            std::fs::rename(&tmp, &dst).unwrap();
        }
    }

    fn cargo_build(root: &Path, spec: Spec) -> PathBuf {
        let (cargo, rustc) = toolchain();
        let target_dir = root.join(format!(
            "target-{}-{}",
            spec.target.unwrap_or("host"),
            spec.debug.unwrap_or("default")
        ));
        let target_dir = target_dir.display().to_string();
        let mut args = vec![
            "build",
            "--offline",
            "--lib",
            "-p",
            "hotpatch-fixture-app",
            "--message-format=json",
        ];
        let features;
        if let Some(edit) = spec.edit {
            features = format!("--features={edit}");
            args.push(&features);
        }
        if let Some(target) = spec.target {
            args.extend(["--target", target]);
        }
        let rustc = rustc.map(|p| p.display().to_string());
        let mut env = vec![("CARGO_TARGET_DIR", target_dir.as_str())];
        if let Some(rustc) = &rustc {
            env.push(("RUSTC", rustc));
        }
        let mut remove = vec![
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_BUILD_RUSTFLAGS",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_BUILD_TARGET",
            "CARGO_INCREMENTAL",
            "CARGO_PROFILE_DEV_SPLIT_DEBUGINFO",
            super::super::capture::CAPTURE_ENV,
        ];
        match spec.debug {
            Some(debug) => env.push(("CARGO_PROFILE_DEV_DEBUG", debug)),
            None => remove.push("CARGO_PROFILE_DEV_DEBUG"),
        }
        let mut lines = Vec::new();
        let out = RealProcessRunner
            .run_streaming_scrubbed(&cargo, &args, Some(root), &env, &remove, &mut |line| {
                lines.push(line.to_string())
            })
            .expect("spawning cargo for the hot-patch fixture");
        assert!(
            out.success,
            "fixture build {spec:?} failed:\n{}\n{}",
            out.stderr,
            lines.join("\n")
        );
        rlib_of(&lines).unwrap_or_else(|| panic!("no app rlib in cargo's output for {spec:?}"))
    }

    /// The app's rlib in `deps/`, named with its metadata hash, so another
    /// edit's build (which rewrites the unhashed copy) never replaces it.
    fn rlib_of(lines: &[String]) -> Option<PathBuf> {
        lines.iter().find_map(|line| {
            let message: serde_json::Value = serde_json::from_str(line).ok()?;
            if message["reason"] != "compiler-artifact" || message["target"]["name"] != APP_CRATE {
                return None;
            }
            message["filenames"].as_array()?.iter().find_map(|file| {
                let file = Path::new(file.as_str()?);
                let name = file.file_name()?.to_str()?;
                let stem = name.strip_suffix(".rmeta").or(name.strip_suffix(".rlib"))?;
                if !stem.contains('-') {
                    return None;
                }
                let deps = if file.parent()?.ends_with("deps") {
                    file.parent()?.to_path_buf()
                } else {
                    file.parent()?.join("deps")
                };
                let rlib = deps.join(format!("{stem}.rlib"));
                rlib.is_file().then_some(rlib)
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(size: u64, hash: &str) -> LayoutEntry {
        LayoutEntry {
            size,
            align: 4,
            hash: hash.to_string(),
        }
    }

    fn table(entries: &[(&str, u64, &str)]) -> LayoutTable {
        LayoutTable {
            types: entries
                .iter()
                .map(|(path, size, hash)| (path.to_string(), entry(*size, hash)))
                .collect(),
        }
    }

    fn unsupported<T: fmt::Debug>(result: Result<T, HotpatchError>) -> String {
        match result {
            Err(HotpatchError::BuilderUnsupported { detail }) => detail,
            other => panic!("expected BuilderUnsupported, got {other:?}"),
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("frust-drive-layout-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A crate whose module `a` uses a struct only `a::make` names, each
    /// module its own codegen unit. At `opt-level = 1` crate-local ThinLTO
    /// imports `make` into `b`'s unit with its own compile unit, which then
    /// defines `Scratch` while the `u64` its members name was already
    /// emitted by `b`'s: the members reference it by `DW_FORM_ref_addr`.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    const CROSS_UNIT_SOURCE: &str = "
pub mod a {
    pub struct Scratch { pub v: u64, pub w: u64 }
    pub fn make(x: u64) -> u64 {
        let scratch = Scratch { v: x * 3, w: x ^ 5 };
        std::hint::black_box(&scratch);
        scratch.v + scratch.w
    }
}
pub mod b {
    pub struct Outer { pub value: u64, pub count: u64 }
    pub fn build(x: u64) -> Outer { Outer { value: crate::a::make(x), count: x } }
}
pub fn total(x: u64) -> u64 { let o = b::build(x); o.value + o.count }
";

    /// [`CROSS_UNIT_SOURCE`] compiled by the toolchain building these tests
    /// as the rlib `xcu` at `opt_level`, full debug info, 16 codegen units.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn cross_unit_rlib(dir: &Path, opt_level: &str) -> PathBuf {
        let out = dir.join(format!("opt-{opt_level}"));
        std::fs::create_dir_all(&out).unwrap();
        let source = dir.join("lib.rs");
        std::fs::write(&source, CROSS_UNIT_SOURCE).unwrap();
        use crate::process::{ProcessRunner as _, RealProcessRunner};
        let (_, rustc) = super::fixture::toolchain();
        let rustc = rustc.map_or_else(|| "rustc".to_string(), |p| p.display().to_string());
        let opt = format!("-Copt-level={opt_level}");
        let (out_text, source_text) = (out.to_string_lossy(), source.to_string_lossy());
        let output = RealProcessRunner
            .run(
                &rustc,
                &[
                    "--edition=2024",
                    "--crate-name=xcu",
                    "--crate-type=rlib",
                    &opt,
                    "-Cdebuginfo=2",
                    "-Ccodegen-units=16",
                    "--out-dir",
                    &out_text,
                    &source_text,
                ],
            )
            .expect("spawning rustc");
        assert!(output.success, "rustc failed at {opt}: {}", output.stderr);
        out.join("libxcu.rlib")
    }

    /// How many members in `rlib`'s objects reference their type in another
    /// compile unit (`DW_FORM_ref_addr`).
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn cross_unit_type_refs(rlib: &Path) -> usize {
        let mut count = 0;
        for_each_object(rlib, |_, file| {
            let sections =
                gimli::DwarfSections::load(|id| -> Result<Cow<'_, [u8]>, gimli::Error> {
                    Ok(file
                        .section_by_name(id.name())
                        .and_then(|section| section.uncompressed_data().ok())
                        .unwrap_or(Cow::Borrowed(&[])))
                })
                .unwrap();
            let dwarf =
                sections.borrow(|data| gimli::EndianSlice::new(data, RunTimeEndian::Little));
            let mut headers = dwarf.units();
            while let Some(header) = headers.next().unwrap() {
                let unit = dwarf.unit(header).unwrap();
                let mut entries = unit.entries();
                while let Some((_, entry)) = entries.next_dfs().unwrap() {
                    if entry.tag() == gimli::DW_TAG_member
                        && let Some(AttributeValue::DebugInfoRef(_)) =
                            entry.attr_value(gimli::DW_AT_type).unwrap()
                    {
                        count += 1;
                    }
                }
            }
            Ok(())
        })
        .unwrap();
        count
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_type_reference_into_another_compile_unit_of_the_object_is_followed() {
        let dir = temp_dir("cross-unit");
        let plain = cross_unit_rlib(&dir, "0");
        let optimized = cross_unit_rlib(&dir, "1");
        assert_eq!(cross_unit_type_refs(&plain), 0);
        assert!(
            cross_unit_type_refs(&optimized) > 0,
            "the optimized rlib must hold a member typed across compile units, or this \
             test proves nothing"
        );
        let crates = vec!["xcu".to_string()];
        let plain = extract(&[plain], &crates).unwrap().table;
        let optimized = extract(&[optimized], &crates).unwrap().table;
        for ty in ["xcu::a::Scratch", "xcu::b::Outer"] {
            let entry = optimized.get(ty).unwrap_or_else(|| panic!("no `{ty}`"));
            assert_eq!(
                Some(entry),
                plain.get(ty),
                "`{ty}` fingerprints alike through a cross-unit reference"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Pure comparison.

    #[test]
    fn diff_reports_only_types_in_both_with_a_different_hash() {
        let accepted = table(&[
            ("a::Same", 4, "1"),
            ("a::Changed", 4, "2"),
            ("a::Gone", 8, "3"),
        ]);
        let candidate = table(&[
            ("a::Same", 4, "1"),
            ("a::Changed", 8, "9"),
            ("a::New", 4, "4"),
        ]);
        assert_eq!(
            diff(&candidate, &accepted),
            vec![LayoutChanged {
                type_path: "a::Changed".into(),
                old_size: 4,
                new_size: 8,
            }]
        );
    }

    #[test]
    fn merge_adds_new_types_and_never_removes_or_replaces_one() {
        let mut accepted = table(&[("a::Kept", 4, "1"), ("a::Dropped", 4, "2")]);
        merge(
            &mut accepted,
            &table(&[("a::Kept", 8, "x"), ("a::New", 2, "3")]),
        );
        assert_eq!(
            accepted,
            table(&[
                ("a::Kept", 4, "1"),
                ("a::Dropped", 4, "2"),
                ("a::New", 2, "3")
            ])
        );
    }

    #[test]
    fn layout_changed_renders_the_restart_reason() {
        let grew = LayoutChanged {
            type_path: "app::HomeState".into(),
            old_size: 4,
            new_size: 8,
        };
        assert_eq!(
            grew.to_string(),
            "app::HomeState changed layout (4 → 8 bytes)"
        );
        let moved = LayoutChanged {
            new_size: 4,
            ..grew
        };
        assert_eq!(
            moved.to_string(),
            "app::HomeState changed layout (4 bytes, members moved)"
        );
    }

    #[test]
    fn scope_covers_crate_paths_and_instantiations_over_them() {
        let crates = vec!["my_app".to_string()];
        assert!(in_scope("my_app::HomeState", &crates));
        assert!(in_scope(
            "my_app::{impl#0}::build::{closure_env#0}",
            &crates
        ));
        assert!(in_scope(
            "alloc::vec::Vec<my_app::Item, alloc::alloc::Global>",
            &crates
        ));
        assert!(in_scope(
            "frust_core::ComponentWidget<my_app::HomePage>",
            &crates
        ));
        assert!(in_scope("(&my_app::HomeState, u32)", &crates));
        assert!(!in_scope(
            "alloc::vec::Vec<u8, alloc::alloc::Global>",
            &crates
        ));
        assert!(!in_scope("not_my_app::Thing", &crates));
        assert!(!in_scope("other::my_app::Thing", &crates));
        assert!(!in_scope("my_app_extra::Thing", &crates));
    }

    #[test]
    fn a_table_round_trips_through_its_json() {
        let dir = temp_dir("round-trip");
        let original = table(&[("a::B", 4, "00ff")]);
        let path = dir.join("t.json");
        original.write(&path).unwrap();
        assert_eq!(LayoutTable::read(&path).unwrap(), original);
    }

    // Fail closed.

    #[test]
    fn an_object_without_dwarf_is_builder_unsupported() {
        let dir = temp_dir("no-dwarf");
        let object = dir.join("empty.o");
        let empty = super::super::link_intercept::empty_object(
            object::BinaryFormat::Elf,
            object::Architecture::Aarch64,
        )
        .unwrap();
        std::fs::write(&object, empty).unwrap();
        let detail = unsupported(extract(&[object], &["my_app".to_string()]));
        assert!(detail.contains("carries no DWARF"), "{detail}");
    }

    /// An x86_64 ELF object whose `.debug_info` holds one empty unit and two
    /// relocations: `R_X86_64_32` against `.debug_abbrev` at the unit's
    /// abbrev offset, and `kind` against a thread-local (or, when
    /// `tls_target` is false, a plain data) symbol in the trailing padding.
    fn elf_with_debug_relocation(kind: u32, tls_target: bool) -> Vec<u8> {
        use object::write::{Object, Relocation, Symbol, SymbolSection};
        use object::{RelocationFlags, SectionKind, SymbolFlags, SymbolScope};
        let mut obj = Object::new(
            BinaryFormat::Elf,
            object::Architecture::X86_64,
            object::Endianness::Little,
        );
        let abbrev = obj.add_section(Vec::new(), b".debug_abbrev".to_vec(), SectionKind::Debug);
        // 1: compile unit, 2: namespace (both with children), 3: a
        // 4-byte struct; all named by an inline string.
        let abbrevs = [
            &[1, 0x11, 1, 0, 0][..],
            &[2, 0x39, 1, 0x03, 0x08, 0, 0],
            &[3, 0x13, 0, 0x03, 0x08, 0x0b, 0x0b, 0, 0],
            &[0],
        ];
        obj.set_section_data(abbrev, abbrevs.concat(), 1);
        let info = obj.add_section(Vec::new(), b".debug_info".to_vec(), SectionKind::Debug);
        // DWARF 4, abbrev offset 0, 8-byte addresses: `my_app::S`, then the
        // scope closes and 8 padding bytes carry the second relocation.
        let mut unit = vec![0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 8];
        unit.extend([1, 2]);
        unit.extend(b"my_app\0");
        unit.extend([3]);
        unit.extend(b"S\0");
        unit.extend([4, 0, 0]);
        let padding = unit.len() as u64;
        unit.extend([0u8; 8]);
        let length = (unit.len() - 4) as u32;
        unit[..4].copy_from_slice(&length.to_le_bytes());
        obj.set_section_data(info, unit, 1);
        let (section, symbol_kind) = if tls_target {
            let tdata = obj.add_section(Vec::new(), b".tdata".to_vec(), SectionKind::Tls);
            obj.set_section_data(tdata, vec![0; 8], 8);
            (tdata, SymbolKind::Tls)
        } else {
            let data = obj.add_section(Vec::new(), b".data".to_vec(), SectionKind::Data);
            obj.set_section_data(data, vec![0; 8], 8);
            (data, SymbolKind::Data)
        };
        let target = obj.add_symbol(Symbol {
            name: b"VAR".to_vec(),
            value: 0,
            size: 8,
            kind: symbol_kind,
            scope: SymbolScope::Compilation,
            weak: false,
            section: SymbolSection::Section(section),
            flags: SymbolFlags::None,
        });
        let abbrev_symbol = obj.section_symbol(abbrev);
        let flags = |r_type| RelocationFlags::Elf { r_type };
        obj.add_relocation(
            info,
            Relocation {
                offset: 6,
                symbol: abbrev_symbol,
                addend: 0,
                flags: flags(object::elf::R_X86_64_32),
            },
        )
        .unwrap();
        obj.add_relocation(
            info,
            Relocation {
                offset: padding,
                symbol: target,
                addend: 0,
                flags: flags(kind),
            },
        )
        .unwrap();
        obj.write().unwrap()
    }

    #[test]
    fn a_tls_offset_relocation_in_debug_info_is_skipped_not_refused() {
        let dir = temp_dir("tls-reloc");
        let object = dir.join("tls.o");
        std::fs::write(
            &object,
            elf_with_debug_relocation(object::elf::R_X86_64_DTPOFF64, true),
        )
        .unwrap();
        let extraction = extract(&[object], &["my_app".to_string()]).unwrap();
        assert!(extraction.table.get("my_app::S").is_some());
    }

    #[test]
    fn an_unsupported_relocation_against_a_plain_symbol_is_builder_unsupported() {
        let dir = temp_dir("plain-reloc");
        let object = dir.join("plain.o");
        std::fs::write(
            &object,
            elf_with_debug_relocation(object::elf::R_X86_64_PC32, false),
        )
        .unwrap();
        let detail = unsupported(extract(&[object], &["my_app".to_string()]));
        assert!(
            detail.contains("unsupported relocation in .debug_info"),
            "{detail}"
        );
    }

    #[test]
    fn a_four_byte_tls_offset_relocation_is_skipped_too() {
        let dir = temp_dir("tls-dtpoff32");
        let object = dir.join("tls32.o");
        std::fs::write(
            &object,
            elf_with_debug_relocation(object::elf::R_X86_64_DTPOFF32, true),
        )
        .unwrap();
        let extraction = extract(&[object], &["my_app".to_string()]).unwrap();
        assert!(extraction.table.get("my_app::S").is_some());
    }

    #[test]
    fn an_input_without_objects_is_builder_unsupported() {
        let dir = temp_dir("no-objects");
        let archive = dir.join("libempty.rlib");
        std::fs::write(&archive, b"!<arch>\n").unwrap();
        let detail = unsupported(extract(&[archive], &["my_app".to_string()]));
        assert!(detail.contains("holds no object file"), "{detail}");
    }

    /// Tests over the fixture workspace, compiled with the pinned toolchain.
    /// Not on Windows: MSVC objects keep types in PDB records, and the gates
    /// read Mach-O and ELF only.
    #[cfg(not(windows))]
    mod fixtures {
        use super::super::fixture::{self, Spec};
        use super::super::*;
        use super::{temp_dir, unsupported};
        use std::time::Instant;

        fn extract_rlib(rlib: &Path) -> Extraction {
            extract(&[rlib.to_path_buf()], &fixture::crates()).expect("extracting the fixture")
        }

        fn table_of(rlib: &Path) -> LayoutTable {
            extract_rlib(rlib).table
        }

        /// The gate's verdict on `edit` against the fat build's table alone.
        fn changes_against_base(edit: &'static str) -> Vec<String> {
            let base = table_of(&fixture::base());
            let candidate = table_of(&fixture::edited(edit));
            diff(&candidate, &base)
                .into_iter()
                .map(|change| change.type_path)
                .collect()
        }

        fn app(path: &str) -> String {
            format!("{}::{path}", fixture::APP_CRATE)
        }

        #[test]
        fn hyphenated_crate_names_are_read_as_underscored() {
            let extraction =
                extract(&[fixture::base()], &["hotpatch-fixture-app".to_string()]).unwrap();
            assert!(extraction.table.get(&app("HomeState")).is_some());
        }

        #[test]
        fn a_build_with_debug_info_off_is_builder_unsupported() {
            let rlib = fixture::build(Spec {
                debug: Some("false"),
                ..Spec::default()
            });
            let detail = unsupported(extract(&[rlib], &fixture::crates()));
            assert!(detail.contains("carries no DWARF"), "{detail}");
        }

        #[test]
        fn a_build_with_line_tables_only_is_builder_unsupported() {
            let rlib = fixture::build(Spec {
                debug: Some("line-tables-only"),
                ..Spec::default()
            });
            let detail = unsupported(extract(&[rlib], &fixture::crates()));
            assert!(detail.contains("without type information"), "{detail}");
        }

        // DWARF presence under the default dev profile.

        fn assert_dwarf_in_every_member(extraction: &Extraction, format: &str, section: &str) {
            assert!(!extraction.sources.is_empty());
            for source in &extraction.sources {
                eprintln!(
                    "dwarf: {} [{}] {} ({} bytes)",
                    source.object, source.format, source.section, source.bytes
                );
                assert!(source.object.ends_with(".o)"), "{}", source.object);
                assert_eq!(source.format, format);
                assert_eq!(source.section, section);
                assert!(source.bytes > 0);
            }
            assert!(extraction.table.get(&app("HomeState")).is_some());
        }

        #[test]
        fn the_default_dev_profile_leaves_dwarf_in_every_rlib_member() {
            let extraction = extract_rlib(&fixture::base());
            if cfg!(target_vendor = "apple") {
                assert_dwarf_in_every_member(&extraction, "Mach-O", "__DWARF,__debug_info");
            } else {
                assert_dwarf_in_every_member(&extraction, "ELF", ".debug_info");
            }
        }

        /// The ELF leg on a non-ELF host: an Android rlib needs no linker, so
        /// it stands in for Linux when that target's std is installed.
        #[test]
        fn an_elf_rlib_carries_dwarf_in_every_member() {
            const TARGET: &str = "aarch64-linux-android";
            if !cfg!(target_vendor = "apple") {
                let extraction = extract_rlib(&fixture::base());
                assert_dwarf_in_every_member(&extraction, "ELF", ".debug_info");
                return;
            }
            if !fixture::has_target(TARGET) {
                eprintln!(
                    "skipped: no {TARGET} std in this toolchain; the ELF leg needs it on macOS"
                );
                return;
            }
            let base = |edit| {
                table_of(&fixture::build(Spec {
                    edit,
                    target: Some(TARGET),
                    debug: None,
                }))
            };
            let extraction = extract_rlib(&fixture::build(Spec {
                target: Some(TARGET),
                ..Spec::default()
            }));
            assert_dwarf_in_every_member(&extraction, "ELF", ".debug_info");
            // Relocated `.debug_str` offsets resolve to the same names and the
            // gate sees the same edit as on the host.
            let changes = diff(&base(Some("d2-field-add")), &base(None));
            let paths: Vec<_> = changes.iter().map(|c| c.type_path.clone()).collect();
            assert!(paths.contains(&app("HomeState")), "{paths:?}");
        }

        // Determinism and the base file.

        #[test]
        fn the_base_table_is_written_and_deterministic_for_the_same_objects() {
            let rlib = fixture::base();
            let start = Instant::now();
            let first = extract_rlib(&rlib);
            let took = start.elapsed();
            let second = extract_rlib(&rlib);
            assert_eq!(first.table.to_json(), second.table.to_json());
            eprintln!(
                "layout gate cost: {} types from {} objects in {took:?}",
                first.table.len(),
                first.sources.len()
            );
            let dir = temp_dir("base");
            let path = first.table.write_base(&dir).unwrap();
            assert_eq!(path, dir.join(LAYOUT_BASE_FILE));
            let written = std::fs::read_to_string(&path).unwrap();
            assert_eq!(written, second.table.to_json());
            assert_eq!(LayoutTable::read(&path).unwrap(), first.table);
        }

        #[test]
        fn the_table_covers_components_closures_and_instantiations() {
            let table = table_of(&fixture::base());
            for path in [
                app("HomeState"),
                app("CounterState"),
                app("on_press::{closure_env#0}"),
                format!("frust_core::ComponentWidget<{}>", app("HomePage")),
                format!("frust_core::FlexView<{}>", app("HomeState")),
            ] {
                assert!(table.get(&path).is_some(), "{path} missing");
            }
            assert!(
                table
                    .types
                    .keys()
                    .all(|path| in_scope(path, &fixture::crates()))
            );
            assert_eq!(table.get(&app("HomeState")).unwrap().size, 4);
        }

        // The hazard rows.

        #[test]
        fn d2_a_field_added_to_state_is_refused() {
            let base = table_of(&fixture::base());
            let candidate = table_of(&fixture::edited("d2-field-add"));
            let changes = diff(&candidate, &base);
            let state = changes
                .iter()
                .find(|c| c.type_path == app("HomeState"))
                .expect("HomeState refused");
            assert_eq!((state.old_size, state.new_size), (4, 8));
            // RESULTS.md §Milestone 1 row D2: the only record. The state is
            // boxed in the widget, so its layout is unchanged.
            assert_eq!(
                changes.iter().map(ToString::to_string).collect::<Vec<_>>(),
                vec![format!("{} changed layout (4 → 8 bytes)", app("HomeState"))]
            );
        }

        #[test]
        fn a_same_size_field_reorder_is_refused() {
            let base = table_of(&fixture::base());
            let candidate = table_of(&fixture::edited("reorder"));
            let changes = diff(&candidate, &base);
            let state = changes
                .iter()
                .find(|c| c.type_path == app("CounterState"))
                .expect("CounterState refused");
            assert_eq!((state.old_size, state.new_size), (8, 8));
            assert_eq!(
                changes
                    .iter()
                    .map(|c| c.type_path.clone())
                    .collect::<Vec<_>>(),
                vec![app("CounterState")]
            );
        }

        #[test]
        fn a_closure_capture_change_is_refused() {
            assert_eq!(
                changes_against_base("closure-capture"),
                vec![app("on_press::{closure_env#0}")]
            );
        }

        #[test]
        fn a_return_type_change_passes_where_the_view_is_erased() {
            assert_eq!(changes_against_base("return-type"), Vec::<String>::new());
        }

        /// The observed row D3 edit (root `column()` wrapped in `stack()`) on
        /// the real-shaped widget: `prev` and `child` are erased, so no record
        /// is produced and `StackView<HomeState>` reaches the gate only as a
        /// new type. At runtime that is a `TypeId` mismatch and a rebuild.
        #[test]
        fn d3_the_stack_wrap_passes_as_new_types() {
            let base = table_of(&fixture::base());
            let candidate = table_of(&fixture::edited("d3-stack-wrap"));
            let changes = diff(&candidate, &base);
            let paths: Vec<_> = changes.iter().map(|c| c.type_path.clone()).collect();
            assert!(
                !paths
                    .iter()
                    .any(|p| p.starts_with("frust_core::ComponentWidget<")),
                "the erased widget must not be refused: {paths:?}"
            );
            let stack = format!("frust_core::StackView<{}>", app("HomeState"));
            assert!(base.get(&stack).is_none());
            assert!(candidate.get(&stack).is_some());
        }

        /// The contrast the real crate no longer has: a widget holding the
        /// view by value moves its layout when the view type changes, so the
        /// same edit would be refused there.
        #[test]
        fn a_by_value_holder_would_be_refused() {
            let base = table_of(&fixture::base());
            let candidate = table_of(&fixture::edited("d3-stack-wrap"));
            let changes = diff(&candidate, &base);
            let by_value = format!("frust_core::ByValueWidget<{}>", app("HomePage"));
            let record = changes
                .iter()
                .find(|c| c.type_path == by_value)
                .expect("the by-value holder is refused");
            // Both views are 32 bytes: the layout moves without growing.
            assert_eq!(record.old_size, record.new_size, "{record}");
            assert!(
                record.to_string().ends_with("bytes, members moved)"),
                "{record}"
            );
            assert_eq!(
                changes
                    .iter()
                    .map(|c| c.type_path.clone())
                    .collect::<Vec<_>>(),
                vec![by_value]
            );
        }

        // The false-positive baseline: layout-preserving edits pass.

        #[test]
        fn layout_preserving_edits_pass() {
            for edit in ["sentinel-bump", "new-helper", "d3-column-wrap"] {
                assert_eq!(changes_against_base(edit), Vec::<String>::new(), "{edit}");
            }
        }

        // Values of an accepted patch stay live: the accepted set, not the base.

        #[test]
        fn a_type_added_in_patch_1_and_changed_in_patch_2_is_refused() {
            let mut accepted = table_of(&fixture::base());
            let patch1 = table_of(&fixture::edited("badge-p1"));
            assert_eq!(diff(&patch1, &accepted), vec![]);
            merge(&mut accepted, &patch1);
            let patch2 = table_of(&fixture::edited("badge-p2"));
            assert_eq!(
                diff(&patch2, &table_of(&fixture::base())),
                vec![],
                "the base alone has no Badge, so it cannot catch this"
            );
            assert_eq!(
                diff(&patch2, &accepted),
                vec![LayoutChanged {
                    type_path: app("Badge"),
                    old_size: 4,
                    new_size: 8,
                }]
            );
        }

        #[test]
        fn a_type_dropped_in_patch_2_and_re_added_in_patch_3_is_refused() {
            let mut accepted = table_of(&fixture::base());
            for patch in ["token-p1", "token-p2"] {
                let candidate = table_of(&fixture::edited(patch));
                assert_eq!(diff(&candidate, &accepted), vec![], "{patch}");
                merge(&mut accepted, &candidate);
            }
            assert!(
                table_of(&fixture::edited("token-p2"))
                    .get(&app("Token"))
                    .is_none()
            );
            assert!(accepted.get(&app("Token")).is_some(), "never removed");
            let patch3 = table_of(&fixture::edited("token-p3"));
            assert_eq!(
                diff(&patch3, &accepted),
                vec![LayoutChanged {
                    type_path: app("Token"),
                    old_size: 4,
                    new_size: 8,
                }]
            );
        }
    }
}
