//! L3 on Windows: the [`layout`](super::layout) gate read from PDB type
//! records (the TPI stream) instead of DWARF.
//!
//! **Table.** [`extract`] opens each PE image's own PDB through
//! [`pe::open_pdb`](super::pe::open_pdb) and records one
//! [`LayoutEntry`](super::layout::LayoutEntry) per struct, class, union or
//! enumeration whose path lies in a replayable crate, and per instantiation
//! of an external generic over such a type. The result is a
//! [`LayoutTable`], so the accepted-layout set,
//! [`layout::diff`](super::layout::diff) and
//! [`layout::merge`](super::layout::merge) apply unchanged: a type present in
//! both a candidate and the accepted set with another hash is a
//! [`LayoutChanged`](super::layout::LayoutChanged), a new type passes, and an
//! entry is never removed within a hot run.
//!
//! An entry holds the byte size, an alignment of 0 (CodeView type records
//! state none), and an FNV-1a 64 hash, as in `layout`, over the record kind,
//! the size and the field list: members `(name, offset, type)`, base classes,
//! enumerators, nested-type names and static members with their constant
//! values, recursing into every by-value member type. A pointer or reference
//! member contributes its mode, size and pointee *name*; its pointee has its
//! own entry. Forward references (CodeView's member types usually point at
//! one) are resolved to their definition by unique name. MSVC-style debug
//! names differ from DWARF's (`closure_env$0` for `{closure_env#0}`,
//! `enum2$<E>` unions for data-carrying enums, whose discriminants are the
//! `DISCR_*` static constants), so a PDB table is only ever compared with
//! another PDB table, never with a DWARF one.
//!
//! **Ordering.** The pinned `pdb` crate reads PDB streams; it exposes no
//! parser for the raw `.debug$T` records inside COFF objects. So the
//! candidate table is read from the thin-linked patch DLL's own PDB (the thin
//! line keeps `/DEBUG` for this), after the link and before the upload,
//! where the DWARF gate reads the objects before the link. The base table is
//! the fat exe's PDB, which `/WHOLEARCHIVE` makes cover every replayable
//! object. The session (`session::link_base` for the base, the builder's
//! post-link table for each candidate) runs the accepted-set check on that
//! table right after the thin link and before anything is sent, so on
//! Windows a refused patch costs one link.
//!
//! **Fail closed.** A missing, foreign or unreadable PDB is
//! [`HotpatchError::BuilderUnsupported`] (from `open_pdb`), and so is a PDB
//! with no type record for any of the replayable crates (`debug = false`,
//! `"line-tables-only"` or `"limited"`): a gate with nothing to compare would
//! pass every edit. A member type that names a record absent from the
//! stream, a malformed record, or a type that contains itself by value is
//! refused too. The PDB reader is a Windows-host dependency, so off Windows
//! [`extract`] answers `BuilderUnsupported`. See `docs/CLI_ARCHITECTURE.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::HotpatchError;
use super::layout::LayoutTable;

/// Where an image's type records were read: the evidence that the build
/// carries them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdbSource {
    /// The PE image.
    pub image: String,
    /// Its own PDB.
    pub pdb: String,
    /// Records in the PDB's type stream.
    pub type_records: usize,
    /// Type definitions of the replayable crates found among them.
    pub types: usize,
}

/// [`extract`]'s result: the table and where its type records came from.
#[derive(Debug, Clone, Default)]
pub struct Extraction {
    pub table: LayoutTable,
    pub sources: Vec<PdbSource>,
}

/// Builds the layout table of `images` (PE images, each beside its own PDB)
/// for the types of `crates` (crate names; `-` is read as `_`).
pub fn extract(images: &[PathBuf], crates: &[String]) -> Result<Extraction, HotpatchError> {
    let crates: Vec<String> = crates.iter().map(|name| name.replace('-', "_")).collect();
    let mut found: BTreeMap<String, BTreeSet<records::Composite>> = BTreeMap::new();
    let mut sources = Vec::new();
    for image in images {
        let source = read_image(image, &crates, &mut found)?;
        if source.types == 0 {
            return Err(HotpatchError::unsupported(format!(
                "PDB `{}` carries no type records for {}; the layout gate needs \
                 `debug = true` (full debug info)",
                source.pdb,
                crates.join(", ")
            )));
        }
        sources.push(source);
    }
    let types = found
        .into_iter()
        .map(|(path, layouts)| (path, records::combine(&layouts)))
        .collect();
    Ok(Extraction {
        table: LayoutTable { types },
        sources,
    })
}

/// Reads `image`'s own PDB into `found`.
#[cfg(windows)]
fn read_image(
    image: &std::path::Path,
    crates: &[String],
    found: &mut BTreeMap<String, BTreeSet<records::Composite>>,
) -> Result<PdbSource, HotpatchError> {
    let (_, mut pdb) = super::pe::open_pdb(image)?;
    let pdb_label = super::pe::pdb_path(image).display().to_string();
    let types = windows::read_types(&mut pdb, &pdb_label)?;
    let type_records = types.len();
    let count = types
        .fingerprint_roots(crates, found)
        .map_err(|err| err.into_hotpatch(&pdb_label))?;
    Ok(PdbSource {
        image: image.display().to_string(),
        pdb: pdb_label,
        type_records,
        types: count,
    })
}

/// Off Windows there is no PDB reader: always
/// [`HotpatchError::BuilderUnsupported`].
#[cfg(not(windows))]
fn read_image(
    image: &std::path::Path,
    _crates: &[String],
    _found: &mut BTreeMap<String, BTreeSet<records::Composite>>,
) -> Result<PdbSource, HotpatchError> {
    Err(HotpatchError::unsupported(format!(
        "reading the PDB type records of PE image `{}` needs a Windows host",
        image.display()
    )))
}

/// The `pdb` crate's records, converted to the owned [`records`] model.
#[cfg(windows)]
mod windows {
    use std::collections::HashMap;

    use pdb::FallibleIterator;

    use super::super::HotpatchError;
    use super::records::{Field, Index, Record, TypeRecords};

    /// Every record of the TPI stream, and the global `S_CONSTANT` values
    /// (static members' constants, e.g. an enum variant's `DISCR_EXACT`).
    pub(super) fn read_types(
        pdb: &mut pdb::PDB<'static, std::fs::File>,
        label: &str,
    ) -> Result<TypeRecords, HotpatchError> {
        let pdb_error = |err: pdb::Error| HotpatchError::unsupported(format!("{label}: {err}"));
        let info = pdb.type_information().map_err(pdb_error)?;
        let mut records = Vec::with_capacity(info.len());
        let mut iter = info.iter();
        while let Some(item) = iter.next().map_err(pdb_error)? {
            let record = match item.parse() {
                Ok(data) => convert(data, item.raw_kind()),
                // A leaf kind the reader does not model: kept by kind, so a
                // member of that type still hashes as something.
                Err(pdb::Error::UnimplementedTypeKind(kind)) => Record::Other(kind),
                Err(err) => {
                    return Err(HotpatchError::unsupported(format!(
                        "{label}: type record {:#x} is malformed: {err}",
                        item.index().0
                    )));
                }
            };
            records.push((item.index().0, record));
        }
        let mut constants = HashMap::new();
        let globals = pdb.global_symbols().map_err(pdb_error)?;
        let mut symbols = globals.iter();
        while let Some(symbol) = symbols.next().map_err(pdb_error)? {
            if let Ok(pdb::SymbolData::Constant(constant)) = symbol.parse() {
                constants.insert(
                    constant.name.to_string().into_owned(),
                    constant.value.to_string(),
                );
            }
        }
        Ok(TypeRecords::new(records, constants))
    }

    fn string(raw: pdb::RawString<'_>) -> String {
        raw.to_string().into_owned()
    }

    fn index(index: pdb::TypeIndex) -> Index {
        index.0
    }

    /// A field-list index: 0 is "none".
    fn list(index: pdb::TypeIndex) -> Option<Index> {
        (index.0 != 0).then_some(index.0)
    }

    fn convert(data: pdb::TypeData<'_>, raw_kind: u16) -> Record {
        use pdb::TypeData;
        match data {
            TypeData::Class(class) => Record::Composite {
                kind: match class.kind {
                    pdb::ClassKind::Class => "class",
                    pdb::ClassKind::Struct => "struct",
                    pdb::ClassKind::Interface => "interface",
                },
                size: class.size,
                fields: class.fields.and_then(list),
                forward: class.properties.forward_reference(),
                name: string(class.name),
                unique: class.unique_name.map(string),
            },
            TypeData::Union(union) => Record::Composite {
                kind: "union",
                size: union.size,
                fields: list(union.fields),
                forward: union.properties.forward_reference(),
                name: string(union.name),
                unique: union.unique_name.map(string),
            },
            TypeData::Enumeration(enumeration) => Record::Enumeration {
                underlying: index(enumeration.underlying_type),
                fields: list(enumeration.fields),
                forward: enumeration.properties.forward_reference(),
                name: string(enumeration.name),
                unique: enumeration.unique_name.map(string),
            },
            TypeData::FieldList(list) => Record::FieldList {
                fields: list.fields.into_iter().filter_map(field).collect(),
                continuation: list.continuation.map(index),
            },
            TypeData::Pointer(pointer) => Record::Pointer {
                pointee: index(pointer.underlying_type),
                size: pointer.attributes.size(),
                mode: match pointer.attributes.pointer_mode() {
                    pdb::PointerMode::Pointer => "ptr",
                    pdb::PointerMode::LValueReference => "ref",
                    pdb::PointerMode::RValueReference => "rref",
                    pdb::PointerMode::Member => "pm",
                    pdb::PointerMode::MemberFunction => "pmf",
                },
            },
            TypeData::Modifier(modifier) => Record::Modifier {
                inner: index(modifier.underlying_type),
                flags: format!(
                    "{}{}{}",
                    u8::from(modifier.constant),
                    u8::from(modifier.volatile),
                    u8::from(modifier.unaligned)
                ),
            },
            TypeData::Array(array) => Record::Array {
                element: index(array.element_type),
                dimensions: array.dimensions,
            },
            TypeData::Bitfield(bitfield) => Record::Bitfield {
                inner: index(bitfield.underlying_type),
                length: bitfield.length,
                position: bitfield.position,
            },
            TypeData::Procedure(_) | TypeData::MemberFunction(_) => Record::Procedure,
            _ => Record::Other(raw_kind),
        }
    }

    /// A field-list entry; methods carry no layout and are dropped.
    fn field(data: pdb::TypeData<'_>) -> Option<Field> {
        use pdb::TypeData;
        Some(match data {
            TypeData::Member(member) => Field::Member {
                name: string(member.name),
                offset: member.offset,
                ty: index(member.field_type),
            },
            TypeData::BaseClass(base) => Field::Base {
                offset: u64::from(base.offset),
                ty: index(base.base_class),
            },
            TypeData::VirtualBaseClass(base) => Field::VirtualBase {
                ty: index(base.base_class),
                pointer_offset: base.base_pointer_offset,
                table_offset: base.virtual_base_offset,
            },
            TypeData::StaticMember(member) => Field::Static {
                name: string(member.name),
                ty: index(member.field_type),
            },
            TypeData::Nested(nested) => Field::Nested {
                name: string(nested.name),
                ty: index(nested.nested_type),
            },
            TypeData::Enumerate(enumerate) => Field::Enumerate {
                name: string(enumerate.name),
                value: enumerate.value.to_string(),
            },
            TypeData::VirtualFunctionTablePointer(table) => Field::VfPtr {
                ty: index(table.table),
            },
            _ => return None,
        })
    }
}

/// The owned type-record model and the fingerprint over it, independent of
/// the PDB reader so its rules are tested on every host.
#[cfg_attr(not(windows), allow(dead_code))]
mod records {
    use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
    use std::fmt::Write as _;

    use super::super::HotpatchError;
    use super::super::layout::{LayoutEntry, in_scope};

    /// A CodeView type index. Below [`FIRST_RECORD`] it names a primitive.
    pub(super) type Index = u32;

    /// The first index of a TPI record; lower indices are primitives.
    pub(super) const FIRST_RECORD: Index = 0x1000;

    /// The deepest chain of pointer/modifier/array names a reference may
    /// spell out before it is refused as malformed.
    const MAX_NAME_DEPTH: usize = 64;

    /// One type record: only what the fingerprint reads.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) enum Record {
        /// `LF_CLASS`/`LF_STRUCTURE`/`LF_INTERFACE`/`LF_UNION`.
        Composite {
            kind: &'static str,
            size: u64,
            fields: Option<Index>,
            forward: bool,
            name: String,
            unique: Option<String>,
        },
        /// `LF_ENUM`: C-like enums.
        Enumeration {
            underlying: Index,
            fields: Option<Index>,
            forward: bool,
            name: String,
            unique: Option<String>,
        },
        FieldList {
            fields: Vec<Field>,
            continuation: Option<Index>,
        },
        Pointer {
            pointee: Index,
            size: u8,
            mode: &'static str,
        },
        Modifier {
            inner: Index,
            /// `const`, `volatile`, `unaligned` as `0`/`1` digits.
            flags: String,
        },
        Array {
            element: Index,
            /// Byte size per dimension.
            dimensions: Vec<u32>,
        },
        Bitfield {
            inner: Index,
            length: u8,
            position: u8,
        },
        /// A function type: a member of this type is a code pointer's target.
        Procedure,
        /// Any other leaf, by kind.
        Other(u16),
    }

    /// One field-list entry.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) enum Field {
        Member {
            name: String,
            offset: u64,
            ty: Index,
        },
        Base {
            offset: u64,
            ty: Index,
        },
        VirtualBase {
            ty: Index,
            pointer_offset: u32,
            table_offset: u32,
        },
        Static {
            name: String,
            ty: Index,
        },
        Nested {
            name: String,
            ty: Index,
        },
        Enumerate {
            name: String,
            value: String,
        },
        VfPtr {
            ty: Index,
        },
    }

    /// One fingerprinted type as found in one PDB.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub(super) struct Composite {
        pub(super) hash: u64,
        pub(super) size: u64,
    }

    /// A type-record read failure.
    #[derive(Debug)]
    pub(super) enum ReadError {
        /// A record references an index the stream does not hold.
        Missing(Index),
        /// A shape the gate does not handle.
        Shape(String),
    }

    impl ReadError {
        pub(super) fn into_hotpatch(self, label: &str) -> HotpatchError {
            match self {
                Self::Missing(index) => HotpatchError::unsupported(format!(
                    "{label}: type record {index:#x} is referenced but missing"
                )),
                Self::Shape(detail) => HotpatchError::unsupported(format!("{label}: {detail}")),
            }
        }
    }

    /// A PDB's type records, indexed, with forward references resolvable.
    pub(super) struct TypeRecords {
        records: HashMap<Index, Record>,
        by_unique: HashMap<String, Index>,
        by_name: HashMap<String, Index>,
        constants: HashMap<String, String>,
    }

    impl TypeRecords {
        /// `records` in stream order; `constants` maps a qualified static
        /// constant's name to its value.
        pub(super) fn new(
            records: Vec<(Index, Record)>,
            constants: HashMap<String, String>,
        ) -> Self {
            let mut by_unique = HashMap::new();
            let mut by_name = HashMap::new();
            for (index, record) in &records {
                let (forward, name, unique) = match record {
                    Record::Composite {
                        forward,
                        name,
                        unique,
                        ..
                    }
                    | Record::Enumeration {
                        forward,
                        name,
                        unique,
                        ..
                    } => (*forward, name, unique),
                    _ => continue,
                };
                if forward {
                    continue;
                }
                if let Some(unique) = unique {
                    by_unique.entry(unique.clone()).or_insert(*index);
                }
                by_name.entry(name.clone()).or_insert(*index);
            }
            Self {
                records: records.into_iter().collect(),
                by_unique,
                by_name,
                constants,
            }
        }

        pub(super) fn len(&self) -> usize {
            self.records.len()
        }

        /// Fingerprints every defined type of `crates` into `found`; returns
        /// how many definitions that was.
        pub(super) fn fingerprint_roots(
            &self,
            crates: &[String],
            found: &mut BTreeMap<String, BTreeSet<Composite>>,
        ) -> Result<usize, ReadError> {
            let mut roots: Vec<(Index, &str)> = self
                .records
                .iter()
                .filter_map(|(index, record)| match record {
                    Record::Composite {
                        forward: false,
                        name,
                        ..
                    }
                    | Record::Enumeration {
                        forward: false,
                        name,
                        ..
                    } if in_scope(name, crates) => Some((*index, name.as_str())),
                    _ => None,
                })
                .collect();
            roots.sort_unstable();
            let mut fingerprints = Fingerprints {
                types: self,
                composites: HashMap::new(),
                refs: HashMap::new(),
                active: HashSet::new(),
            };
            for (index, path) in &roots {
                let composite = fingerprints.composite(*index)?;
                found.entry(path.to_string()).or_default().insert(composite);
            }
            Ok(roots.len())
        }

        fn record(&self, index: Index) -> Result<&Record, ReadError> {
            self.records.get(&index).ok_or(ReadError::Missing(index))
        }

        /// The defining record of a composite or enumeration: itself unless
        /// it is a forward reference, else the definition with its unique
        /// name (or, lacking one, its name). `None` when nothing defines it.
        fn definition(&self, index: Index) -> Result<Option<Index>, ReadError> {
            let (forward, name, unique) = match self.record(index)? {
                Record::Composite {
                    forward,
                    name,
                    unique,
                    ..
                }
                | Record::Enumeration {
                    forward,
                    name,
                    unique,
                    ..
                } => (*forward, name, unique),
                _ => return Ok(Some(index)),
            };
            if !forward {
                return Ok(Some(index));
            }
            Ok(match unique {
                Some(unique) => self.by_unique.get(unique).copied(),
                None => self.by_name.get(name).copied(),
            })
        }

        /// The entries of a field list and its continuations.
        fn fields(&self, list: Option<Index>) -> Result<Vec<&Field>, ReadError> {
            let mut out = Vec::new();
            let mut seen = HashSet::new();
            let mut next = list;
            while let Some(index) = next {
                if !seen.insert(index) {
                    return Err(ReadError::Shape(format!(
                        "field list {index:#x} continues into itself"
                    )));
                }
                match self.record(index)? {
                    Record::FieldList {
                        fields,
                        continuation,
                    } => {
                        out.extend(fields);
                        next = *continuation;
                    }
                    other => {
                        return Err(ReadError::Shape(format!(
                            "type record {index:#x} should be a field list, found {other:?}"
                        )));
                    }
                }
            }
            Ok(out)
        }
    }

    /// Memoised fingerprints of composites and descriptors of referenced
    /// types within one PDB.
    struct Fingerprints<'a> {
        types: &'a TypeRecords,
        composites: HashMap<Index, Composite>,
        refs: HashMap<Index, String>,
        active: HashSet<Index>,
    }

    impl Fingerprints<'_> {
        /// The fingerprint of the composite or enumeration *defined* at
        /// `index`.
        fn composite(&mut self, index: Index) -> Result<Composite, ReadError> {
            if let Some(composite) = self.composites.get(&index) {
                return Ok(*composite);
            }
            let types = self.types;
            let (canon_head, size, fields, owner) = match types.record(index)? {
                Record::Composite {
                    kind,
                    size,
                    fields,
                    name,
                    ..
                } => (format!("{kind}|{size}|"), *size, *fields, name.as_str()),
                Record::Enumeration {
                    underlying,
                    fields,
                    name,
                    ..
                } => {
                    let size = primitive_size(*underlying).unwrap_or(0);
                    let head = format!("enum|{size}|u:{}|", self.type_ref(*underlying)?);
                    (head, size, *fields, name.as_str())
                }
                other => {
                    return Err(ReadError::Shape(format!(
                        "type record {index:#x} is not a type definition: {other:?}"
                    )));
                }
            };
            if !self.active.insert(index) {
                return Err(ReadError::Shape(format!(
                    "type `{owner}` contains itself by value"
                )));
            }
            let mut canon = canon_head;
            for field in types.fields(fields)? {
                match field {
                    Field::Member { name, offset, ty } => {
                        let desc = self.type_ref(*ty)?;
                        let _ = write!(canon, "m:{name}@{offset}:{desc}|");
                    }
                    Field::Base { offset, ty } => {
                        let desc = self.type_ref(*ty)?;
                        let _ = write!(canon, "b@{offset}:{desc}|");
                    }
                    Field::VirtualBase {
                        ty,
                        pointer_offset,
                        table_offset,
                    } => {
                        let name = self.name_of(*ty, 0)?;
                        let _ = write!(canon, "vb:{name}@{pointer_offset}/{table_offset}|");
                    }
                    Field::Static { name, ty } => {
                        let value = types
                            .constants
                            .get(&format!("{owner}::{name}"))
                            .map_or("-", String::as_str);
                        let ty = self.name_of(*ty, 0)?;
                        let _ = write!(canon, "s:{name}={value}:{ty}|");
                    }
                    Field::Nested { name, ty } => {
                        let ty = self.name_of(*ty, 0)?;
                        let _ = write!(canon, "n:{name}:{ty}|");
                    }
                    Field::Enumerate { name, value } => {
                        let _ = write!(canon, "e:{name}={value}|");
                    }
                    Field::VfPtr { ty } => {
                        let ty = self.name_of(*ty, 0)?;
                        let _ = write!(canon, "vf:{ty}|");
                    }
                }
            }
            let mut hasher = Fnv1a64::new();
            hasher.write(canon.as_bytes());
            let composite = Composite {
                hash: hasher.finish(),
                size,
            };
            self.active.remove(&index);
            self.composites.insert(index, composite);
            Ok(composite)
        }

        /// A by-value reference: a composite by path and fingerprint, a
        /// pointer by its mode, size and pointee name, anything else by its
        /// shape.
        fn type_ref(&mut self, index: Index) -> Result<String, ReadError> {
            if index < FIRST_RECORD {
                return Ok(format!("b:{index:#x}"));
            }
            if let Some(desc) = self.refs.get(&index) {
                return Ok(desc.clone());
            }
            let types = self.types;
            let desc = match types.record(index)? {
                Record::Composite { name, .. } | Record::Enumeration { name, .. } => {
                    match types.definition(index)? {
                        Some(definition) => {
                            let composite = self.composite(definition)?;
                            format!("{name}#{:016x}", composite.hash)
                        }
                        None => format!("{name}#undefined"),
                    }
                }
                Record::Pointer {
                    pointee,
                    size,
                    mode,
                } => format!("p:{mode}:{size}:{}", self.name_of(*pointee, 0)?),
                Record::Modifier { inner, flags } => {
                    format!("q{flags}:{}", self.type_ref(*inner)?)
                }
                Record::Array {
                    element,
                    dimensions,
                } => format!("a:[{};{}]", self.type_ref(*element)?, dims(dimensions)),
                Record::Bitfield {
                    inner,
                    length,
                    position,
                } => format!("f:{length}@{position}:{}", self.type_ref(*inner)?),
                Record::Procedure => "fn".to_string(),
                Record::Other(kind) => format!("t{kind:#x}"),
                Record::FieldList { .. } => {
                    return Err(ReadError::Shape(format!(
                        "type record {index:#x} is a field list used as a type"
                    )));
                }
            };
            self.refs.insert(index, desc.clone());
            Ok(desc)
        }

        /// A by-reference name: what a pointer to the type spells, without
        /// its contents.
        fn name_of(&self, index: Index, depth: usize) -> Result<String, ReadError> {
            if index < FIRST_RECORD {
                return Ok(format!("b:{index:#x}"));
            }
            if depth > MAX_NAME_DEPTH {
                return Err(ReadError::Shape(format!(
                    "type record {index:#x} names a chain deeper than {MAX_NAME_DEPTH}"
                )));
            }
            let next = depth + 1;
            Ok(match self.types.record(index)? {
                Record::Composite { name, .. } | Record::Enumeration { name, .. } => name.clone(),
                Record::Pointer {
                    pointee,
                    size,
                    mode,
                } => format!("p:{mode}:{size}:{}", self.name_of(*pointee, next)?),
                Record::Modifier { inner, flags } => {
                    format!("q{flags}:{}", self.name_of(*inner, next)?)
                }
                Record::Array {
                    element,
                    dimensions,
                } => format!("a:[{};{}]", self.name_of(*element, next)?, dims(dimensions)),
                Record::Bitfield {
                    inner,
                    length,
                    position,
                } => format!("f:{length}@{position}:{}", self.name_of(*inner, next)?),
                Record::Procedure => "fn".to_string(),
                Record::Other(kind) => format!("t{kind:#x}"),
                Record::FieldList { .. } => {
                    return Err(ReadError::Shape(format!(
                        "type record {index:#x} is a field list used as a type"
                    )));
                }
            })
        }
    }

    fn dims(dimensions: &[u32]) -> String {
        dimensions
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The byte size of a direct CodeView primitive (`cvinfo.h`'s basic
    /// types), or a near pointer's by its mode.
    pub(super) fn primitive_size(index: Index) -> Option<u64> {
        if index >= FIRST_RECORD {
            return None;
        }
        match (index >> 8) & 0xf {
            0 => {}
            0x4 | 0x5 => return Some(4),
            0x6 => return Some(8),
            0x7 => return Some(16),
            _ => return None,
        }
        Some(match index & 0xff {
            0x10 | 0x20 | 0x68 | 0x69 | 0x70 | 0x7c | 0x30 => 1,
            0x11 | 0x21 | 0x71 | 0x72 | 0x73 | 0x7a | 0x31 | 0x46 => 2,
            0x12 | 0x22 | 0x74 | 0x75 | 0x7b | 0x32 | 0x40 => 4,
            0x13 | 0x23 | 0x76 | 0x77 | 0x33 | 0x41 => 8,
            0x42 => 10,
            0x14 | 0x24 | 0x78 | 0x79 | 0x43 => 16,
            _ => return None,
        })
    }

    /// The entry for a path found once or more, as `layout` combines one:
    /// if two PDBs disagree, the entry hashes all of them, so it still
    /// compares equal only to a build that shows the same set.
    pub(super) fn combine(layouts: &BTreeSet<Composite>) -> LayoutEntry {
        let mut iter = layouts.iter();
        let first = iter
            .next()
            .copied()
            .unwrap_or(Composite { hash: 0, size: 0 });
        if layouts.len() == 1 {
            return LayoutEntry {
                size: first.size,
                align: 0,
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
            align: 0,
            hash: format!("{:016x}", hasher.finish()),
        }
    }

    /// FNV-1a 64, as `layout`'s: stable across processes and toolchains.
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
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::records::{Composite, Field, Index, Record, TypeRecords, combine, primitive_size};
    use super::*;
    use crate::hotpatch::layout::{LayoutChanged, diff, merge};

    const U32: Index = 0x75;
    const U16: Index = 0x73;
    const U64: Index = 0x77;
    const U8: Index = 0x69;

    fn crates() -> Vec<String> {
        vec!["app".to_string()]
    }

    fn class(name: &str, size: u64, fields: Option<Index>, forward: bool) -> Record {
        Record::Composite {
            kind: "struct",
            size,
            fields,
            forward,
            name: name.to_string(),
            unique: Some(format!("u:{name}")),
        }
    }

    fn members(fields: &[(&str, u64, Index)]) -> Record {
        Record::FieldList {
            fields: fields
                .iter()
                .map(|(name, offset, ty)| Field::Member {
                    name: name.to_string(),
                    offset: *offset,
                    ty: *ty,
                })
                .collect(),
            continuation: None,
        }
    }

    fn table(records: Vec<(Index, Record)>) -> Result<LayoutTable, String> {
        table_with(records, HashMap::new())
    }

    fn table_with(
        records: Vec<(Index, Record)>,
        constants: HashMap<String, String>,
    ) -> Result<LayoutTable, String> {
        let types = TypeRecords::new(records, constants);
        let mut found = BTreeMap::new();
        match types.fingerprint_roots(&crates(), &mut found) {
            Ok(_) => Ok(LayoutTable {
                types: found
                    .into_iter()
                    .map(|(path, layouts)| (path, combine(&layouts)))
                    .collect(),
            }),
            Err(err) => match err.into_hotpatch("t.pdb") {
                HotpatchError::BuilderUnsupported { detail } => Err(detail),
                other => panic!("{other:?}"),
            },
        }
    }

    /// `app::Inner` (fields at 0x1001) held by value in `app::Outer`, whose
    /// member type is Inner's forward reference, as CodeView emits it.
    fn outer_over_inner(inner: &[(&str, u64, Index)], inner_size: u64) -> Vec<(Index, Record)> {
        vec![
            (0x1000, class("app::Inner", 0, None, true)),
            (0x1001, members(inner)),
            (0x1002, class("app::Inner", inner_size, Some(0x1001), false)),
            (
                0x1003,
                members(&[("inner", 0, 0x1000), ("tail", inner_size, U32)]),
            ),
            (
                0x1004,
                class("app::Outer", inner_size + 4, Some(0x1003), false),
            ),
        ]
    }

    #[test]
    fn a_forward_referenced_member_hashes_its_definition() {
        let base = table(outer_over_inner(&[("a", 0, U32)], 4)).unwrap();
        let edited = table(outer_over_inner(&[("a", 0, U32), ("b", 4, U32)], 8)).unwrap();
        assert_eq!(base.len(), 2, "{base:?}");
        assert_eq!(base.get("app::Inner").unwrap().align, 0);
        let mut paths: Vec<_> = diff(&edited, &base)
            .into_iter()
            .map(|c| (c.type_path, c.old_size, c.new_size))
            .collect();
        paths.sort();
        assert_eq!(
            paths,
            vec![
                ("app::Inner".to_string(), 4, 8),
                ("app::Outer".to_string(), 8, 12)
            ]
        );
    }

    #[test]
    fn a_same_size_reorder_changes_the_hash() {
        let base = table(outer_over_inner(&[("a", 0, U16), ("b", 2, U16)], 4)).unwrap();
        let reordered = table(outer_over_inner(&[("b", 0, U16), ("a", 2, U16)], 4)).unwrap();
        let changes = diff(&reordered, &base);
        assert!(
            changes
                .iter()
                .any(|c| c.type_path == "app::Inner" && c.old_size == c.new_size),
            "{changes:?}"
        );
    }

    #[test]
    fn a_pointer_member_hashes_the_pointee_name_not_its_layout() {
        let records = |inner_size: u64, extra: bool| {
            let inner: Vec<(&str, u64, Index)> = if extra {
                vec![("a", 0, U32), ("b", 4, U32)]
            } else {
                vec![("a", 0, U32)]
            };
            vec![
                (0x1000, class("app::Inner", 0, None, true)),
                (0x1001, members(&inner)),
                (0x1002, class("app::Inner", inner_size, Some(0x1001), false)),
                (
                    0x1003,
                    Record::Pointer {
                        pointee: 0x1000,
                        size: 8,
                        mode: "ptr",
                    },
                ),
                (0x1004, members(&[("inner", 0, 0x1003)])),
                (0x1005, class("app::Holder", 8, Some(0x1004), false)),
            ]
        };
        let base = table(records(4, false)).unwrap();
        let edited = table(records(8, true)).unwrap();
        let changes: Vec<_> = diff(&edited, &base)
            .into_iter()
            .map(|c| c.type_path)
            .collect();
        assert_eq!(changes, vec!["app::Inner".to_string()]);
    }

    #[test]
    fn only_defined_types_of_the_replayable_crates_are_entries() {
        let records = vec![
            (0x1000, members(&[("a", 0, U32)])),
            (0x1001, class("app::Defined", 4, Some(0x1000), false)),
            (0x1002, class("app::OnlyDeclared", 0, None, true)),
            (0x1003, class("other::Thing", 4, Some(0x1000), false)),
            (
                0x1004,
                class("core::option::Option<app::Defined>", 8, Some(0x1000), false),
            ),
            (
                0x1005,
                class("app::on_press::closure_env$0", 4, Some(0x1000), false),
            ),
        ];
        let table = table(records).unwrap();
        assert_eq!(
            table.types.keys().cloned().collect::<Vec<_>>(),
            vec![
                "app::Defined".to_string(),
                "app::on_press::closure_env$0".to_string(),
                "core::option::Option<app::Defined>".to_string(),
            ]
        );
    }

    #[test]
    fn enumerators_and_static_constants_feed_the_hash() {
        let records = |b: &str, discr: &str| {
            (
                vec![
                    (
                        0x1000,
                        Record::FieldList {
                            fields: vec![
                                Field::Enumerate {
                                    name: "A".into(),
                                    value: "0".into(),
                                },
                                Field::Enumerate {
                                    name: b.into(),
                                    value: "1".into(),
                                },
                            ],
                            continuation: None,
                        },
                    ),
                    (
                        0x1001,
                        Record::Enumeration {
                            underlying: U8,
                            fields: Some(0x1000),
                            forward: false,
                            name: "app::Kind".into(),
                            unique: None,
                        },
                    ),
                    (
                        0x1002,
                        Record::FieldList {
                            fields: vec![Field::Static {
                                name: "DISCR_EXACT".into(),
                                ty: U64,
                            }],
                            continuation: None,
                        },
                    ),
                    (0x1003, class("app::Variant0", 0, Some(0x1002), false)),
                ],
                HashMap::from([("app::Variant0::DISCR_EXACT".to_string(), discr.to_string())]),
            )
        };
        let (r, c) = records("B", "7");
        let base = table_with(r, c).unwrap();
        assert_eq!(base.get("app::Kind").unwrap().size, 1);
        let (r, c) = records("C", "7");
        let renamed = table_with(r, c).unwrap();
        let (r, c) = records("B", "8");
        let rediscriminated = table_with(r, c).unwrap();
        assert_eq!(
            diff(&renamed, &base)
                .into_iter()
                .map(|c| c.type_path)
                .collect::<Vec<_>>(),
            vec!["app::Kind".to_string()]
        );
        assert_eq!(
            diff(&rediscriminated, &base)
                .into_iter()
                .map(|c| c.type_path)
                .collect::<Vec<_>>(),
            vec!["app::Variant0".to_string()]
        );
    }

    #[test]
    fn a_continued_field_list_is_read_whole() {
        let records = |tail: Index| {
            vec![
                (0x1000, members(&[("b", 4, tail)])),
                (
                    0x1001,
                    Record::FieldList {
                        fields: vec![Field::Member {
                            name: "a".into(),
                            offset: 0,
                            ty: U32,
                        }],
                        continuation: Some(0x1000),
                    },
                ),
                (0x1002, class("app::Long", 8, Some(0x1001), false)),
            ]
        };
        let base = table(records(U32)).unwrap();
        let edited = table(records(0x74)).unwrap();
        assert_eq!(diff(&edited, &base).len(), 1);
    }

    // Fail closed.

    #[test]
    fn a_missing_member_type_record_is_refused() {
        let detail = table(vec![
            (0x1000, members(&[("a", 0, 0x1fff)])),
            (0x1001, class("app::Broken", 4, Some(0x1000), false)),
        ])
        .unwrap_err();
        assert!(
            detail.contains("0x1fff") && detail.contains("missing"),
            "{detail}"
        );
    }

    #[test]
    fn a_type_containing_itself_by_value_is_refused() {
        let detail = table(vec![
            (0x1000, class("app::Loop", 0, None, true)),
            (0x1001, members(&[("me", 0, 0x1000)])),
            (0x1002, class("app::Loop", 4, Some(0x1001), false)),
        ])
        .unwrap_err();
        assert!(detail.contains("contains itself by value"), "{detail}");
    }

    #[test]
    fn a_field_list_that_is_not_one_is_refused() {
        let detail = table(vec![
            (0x1000, Record::Procedure),
            (0x1001, class("app::Odd", 4, Some(0x1000), false)),
        ])
        .unwrap_err();
        assert!(detail.contains("should be a field list"), "{detail}");
    }

    #[test]
    fn primitive_sizes_follow_cvinfo() {
        assert_eq!(primitive_size(U8), Some(1));
        assert_eq!(primitive_size(U16), Some(2));
        assert_eq!(primitive_size(U32), Some(4));
        assert_eq!(primitive_size(U64), Some(8));
        assert_eq!(primitive_size(0x0603), Some(8), "a near64 void*");
        assert_eq!(primitive_size(0x1000), None);
    }

    #[test]
    fn a_path_found_with_two_layouts_combines_to_neither() {
        let one = Composite { hash: 1, size: 4 };
        let two = Composite { hash: 2, size: 8 };
        let both = combine(&BTreeSet::from([one, two]));
        assert_eq!(both.size, 8);
        assert_ne!(both.hash, combine(&BTreeSet::from([one])).hash);
        assert_ne!(both.hash, combine(&BTreeSet::from([two])).hash);
    }

    #[test]
    fn the_accepted_set_rules_apply_to_pdb_tables_unchanged() {
        let mut accepted = table(outer_over_inner(&[("a", 0, U32)], 4)).unwrap();
        let grown = table(outer_over_inner(&[("a", 0, U32), ("b", 4, U32)], 8)).unwrap();
        assert_eq!(
            diff(&grown, &accepted).first(),
            Some(&LayoutChanged {
                type_path: "app::Inner".into(),
                old_size: 4,
                new_size: 8,
            })
        );
        merge(&mut accepted, &grown);
        assert_eq!(
            accepted.get("app::Inner").unwrap().size,
            4,
            "never replaced"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn off_windows_extract_is_builder_unsupported() {
        let err = extract(&[PathBuf::from("C:/app/app.exe")], &crates()).unwrap_err();
        assert!(
            matches!(&err, HotpatchError::BuilderUnsupported { detail } if detail.contains("Windows host")),
            "{err:?}"
        );
    }

    /// The H1-05 fixture set, linked into a DLL per edit on a Windows host so
    /// each build carries the PDB the gate reads.
    #[cfg(windows)]
    mod fixtures {
        use std::path::Path;
        use std::time::Instant;

        use super::super::*;
        use super::fixture::{self, Spec};
        use crate::hotpatch::layout::{LAYOUT_BASE_FILE, LayoutChanged, diff, in_scope, merge};
        use crate::hotpatch::pe;

        fn extract_dll(dll: &Path) -> Extraction {
            extract(&[dll.to_path_buf()], &fixture::crates()).expect("extracting the fixture")
        }

        fn table_of(dll: &Path) -> LayoutTable {
            extract_dll(dll).table
        }

        fn unsupported(result: Result<Extraction, HotpatchError>) -> String {
            match result {
                Err(HotpatchError::BuilderUnsupported { detail }) => detail,
                other => panic!(
                    "expected BuilderUnsupported, got {:?}",
                    other.map(|e| e.table)
                ),
            }
        }

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
            let dll = fixture::build(Spec {
                debug: Some("false"),
                ..Spec::default()
            });
            let detail = unsupported(extract(&[dll], &fixture::crates()));
            eprintln!("debug = false: {detail}");
        }

        #[test]
        fn a_build_with_line_tables_only_is_builder_unsupported() {
            let dll = fixture::build(Spec {
                debug: Some("line-tables-only"),
                ..Spec::default()
            });
            let detail = unsupported(extract(&[dll], &fixture::crates()));
            assert!(detail.contains("no type records"), "{detail}");
        }

        #[test]
        fn the_default_dev_profile_puts_type_records_in_the_dll_pdb() {
            let dll = fixture::base();
            let extraction = extract_dll(&dll);
            let [source] = extraction.sources.as_slice() else {
                panic!("{:?}", extraction.sources);
            };
            eprintln!(
                "pdb: {} ({} type records, {} app types)",
                source.pdb, source.type_records, source.types
            );
            assert_eq!(source.pdb, pe::pdb_path(&dll).display().to_string());
            assert!(source.type_records > 0 && source.types > 0);
        }

        #[test]
        fn the_base_table_is_written_and_deterministic_for_the_same_image() {
            let dll = fixture::base();
            let start = Instant::now();
            let first = extract_dll(&dll);
            let took = start.elapsed();
            let second = extract_dll(&dll);
            assert_eq!(first.table.to_json(), second.table.to_json());
            eprintln!(
                "pdb layout gate cost: {} types from {} type records in {took:?}",
                first.table.len(),
                first.sources[0].type_records
            );
            let dir = std::env::temp_dir().join(format!(
                "frust-drive-pdb-layout-base-{}",
                std::process::id()
            ));
            let path = first.table.write_base(&dir).unwrap();
            assert_eq!(path, dir.join(LAYOUT_BASE_FILE));
            assert_eq!(LayoutTable::read(&path).unwrap(), first.table);
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn the_table_covers_components_closures_and_instantiations() {
            let table = table_of(&fixture::base());
            let keys: Vec<&String> = table.types.keys().collect();
            eprintln!("{keys:#?}");
            for path in [
                app("HomeState"),
                app("CounterState"),
                app("on_press::closure_env$0"),
                format!("frust_core::ComponentWidget<{}>", app("HomePage")),
                format!("frust_core::FlexView<{}>", app("HomeState")),
            ] {
                assert!(table.get(&path).is_some(), "{path} missing from {keys:#?}");
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
            assert_eq!(
                changes
                    .iter()
                    .map(|c| (c.type_path.clone(), c.old_size, c.new_size))
                    .collect::<Vec<_>>(),
                vec![(app("CounterState"), 8, 8)]
            );
        }

        #[test]
        fn a_closure_capture_change_is_refused() {
            assert_eq!(
                changes_against_base("closure-capture"),
                vec![app("on_press::closure_env$0")]
            );
        }

        #[test]
        fn a_return_type_change_passes_where_the_view_is_erased() {
            assert_eq!(changes_against_base("return-type"), Vec::<String>::new());
        }

        #[test]
        fn d3_the_stack_wrap_passes_as_new_types() {
            let base = table_of(&fixture::base());
            let candidate = table_of(&fixture::edited("d3-stack-wrap"));
            let paths: Vec<_> = diff(&candidate, &base)
                .into_iter()
                .map(|c| c.type_path)
                .collect();
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
            assert_eq!(record.old_size, record.new_size, "{record}");
            assert_eq!(
                changes
                    .iter()
                    .map(|c| c.type_path.clone())
                    .collect::<Vec<_>>(),
                vec![by_value]
            );
        }

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

        /// The enum shapes the fixture set lacks, from a rustc-built probe:
        /// a C-like enum's enumerators and a data-carrying enum's
        /// discriminant constants reach the hash.
        #[test]
        fn an_enum_discriminant_change_is_refused() {
            let build = |tag: &str, source: &str| fixture::probe(tag, source);
            let base = build(
                "enum-base",
                "pub enum Mode { Off = 1, On = 2 }\n\
                 #[repr(u32)]\n\
                 pub enum Shape { Dot(u32) = 3, Line(u32, u32) = 4 }",
            );
            let moved = build(
                "enum-moved",
                "pub enum Mode { Off = 1, On = 5 }\n\
                 #[repr(u32)]\n\
                 pub enum Shape { Dot(u32) = 3, Line(u32, u32) = 9 }",
            );
            let crates = vec!["pdb_probe".to_string()];
            let base = extract(&[base], &crates).unwrap().table;
            let moved = extract(&[moved], &crates).unwrap().table;
            eprintln!("{:#?}", base.types.keys().collect::<Vec<_>>());
            let paths: Vec<String> = diff(&moved, &base)
                .into_iter()
                .map(|c| c.type_path)
                .collect();
            eprintln!("enum edits refused: {paths:#?}");
            assert!(paths.contains(&"pdb_probe::Mode".to_string()), "{paths:?}");
            // `Line`'s discriminant lives only in its variant's `DISCR_EXACT`
            // constant (`VariantNames` numbers variants by index): the global
            // `S_CONSTANT` records reach the hash.
            for shape in [
                "enum2$<pdb_probe::Shape>::Variant1",
                "enum2$<pdb_probe::Shape>",
            ] {
                assert!(paths.contains(&shape.to_string()), "{shape}: {paths:?}");
            }
            assert!(
                !paths.contains(&"enum2$<pdb_probe::Shape>::Variant0".to_string()),
                "`Dot` kept its discriminant: {paths:?}"
            );
        }
    }

    #[cfg(windows)]
    mod fixture {
        //! Builds the fixture workspace in `tests/fixtures/hotpatch` with the
        //! pinned toolchain as a DLL per edit (`cargo rustc --crate-type
        //! cdylib -- -Clink-dead-code`: every item of the crate is codegened
        //! and its own objects are linked whole, as in a thin link), each
        //! into its own target dir under the test binary's (the DLL's name
        //! does not change with the edit).

        use std::collections::HashMap;
        use std::path::{Path, PathBuf};
        use std::sync::Mutex;

        use crate::hotpatch::pe;
        use crate::process::{ProcessRunner, RealProcessRunner};

        /// The replayable crate of the fixture.
        pub const APP_CRATE: &str = "hotpatch_fixture_app";

        /// One fixture build: an edit (a cargo feature of the app, `None` for
        /// the fat build) and an override of the dev profile's `debug`.
        #[derive(Clone, Copy, Debug, Default)]
        pub struct Spec {
            pub edit: Option<&'static str>,
            pub debug: Option<&'static str>,
        }

        static BUILT: Mutex<Option<HashMap<String, PathBuf>>> = Mutex::new(None);

        pub fn base() -> PathBuf {
            build(Spec::default())
        }

        pub fn edited(edit: &'static str) -> PathBuf {
            build(Spec {
                edit: Some(edit),
                debug: None,
            })
        }

        pub fn crates() -> Vec<String> {
            vec![APP_CRATE.to_string()]
        }

        fn root() -> PathBuf {
            let exe = std::env::current_exe().unwrap();
            exe.parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("frust-hotpatch-pdb-fixtures")
        }

        /// The app DLL `spec` builds. Panics with cargo's output on failure.
        pub fn build(spec: Spec) -> PathBuf {
            let key = format!("{spec:?}");
            let mut built = BUILT.lock().unwrap_or_else(|poison| poison.into_inner());
            let built = built.get_or_insert_with(HashMap::new);
            if let Some(path) = built.get(&key) {
                return path.clone();
            }
            let root = root().join("workspace");
            sync_sources(
                &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hotpatch"),
                &root,
            );
            let path = cargo_rustc(&root, spec);
            built.insert(key, path.clone());
            path
        }

        /// A one-file `pdb_probe` cdylib with full debug info, built by rustc
        /// into its own directory.
        pub fn probe(tag: &str, source: &str) -> PathBuf {
            let dir = root().join(format!("probe-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let src = dir.join("pdb_probe.rs");
            std::fs::write(
                &src,
                format!(
                    "{source}\n\
                     #[unsafe(no_mangle)]\n\
                     pub extern \"C\" fn pdb_probe_keep() -> usize {{\n\
                         std::mem::size_of::<Mode>() + std::mem::size_of::<Shape>()\n\
                         + std::hint::black_box(&[Mode::Off, Mode::On]).len()\n\
                         + std::hint::black_box(&[Shape::Dot(1), Shape::Line(2, 3)]).len()\n\
                     }}\n"
                ),
            )
            .unwrap();
            let (_, rustc) = toolchain();
            let rustc = rustc.map_or_else(|| "rustc".to_string(), |p| p.display().to_string());
            let output = std::process::Command::new(rustc)
                .args(["--edition", "2024", "--crate-type", "cdylib"])
                .args([
                    "--crate-name",
                    "pdb_probe",
                    "-Cdebuginfo=2",
                    "-Copt-level=0",
                ])
                .arg("--out-dir")
                .arg(&dir)
                .arg(&src)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "rustc probe: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let dll = dir.join("pdb_probe.dll");
            assert!(dll.is_file() && pe::pdb_path(&dll).is_file(), "{dll:?}");
            dll
        }

        /// The pinned toolchain's cargo and rustc.
        fn toolchain() -> (String, Option<PathBuf>) {
            let cargo = option_env!("CARGO").unwrap_or("cargo").to_string();
            let rustc = Path::new(&cargo).with_file_name("rustc.exe");
            (cargo, rustc.is_file().then_some(rustc))
        }

        /// Mirrors the fixture sources into `to`, rewriting only changed files
        /// (atomically), as `layout`'s fixture does.
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

        fn cargo_rustc(root: &Path, spec: Spec) -> PathBuf {
            let (cargo, rustc) = toolchain();
            let target_dir = root.parent().unwrap().join(format!(
                "target-{}-{}",
                spec.edit.unwrap_or("base"),
                spec.debug.unwrap_or("default")
            ));
            let target_dir = target_dir.display().to_string();
            let mut args = vec![
                "rustc",
                "--offline",
                "--lib",
                "-p",
                "hotpatch-fixture-app",
                "--crate-type",
                "cdylib",
                "--message-format=json",
            ];
            let features;
            if let Some(edit) = spec.edit {
                features = format!("--features={edit}");
                args.push(&features);
            }
            // A cdylib roots codegen at its C exports only, and the fixture
            // has none: collect every item, as for the rlib a real build
            // links, and keep the linker from dropping it.
            args.extend(["--", "-Clink-dead-code"]);
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
                crate::hotpatch::capture::CAPTURE_ENV,
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
                .expect("spawning cargo for the hot-patch PDB fixture");
            assert!(
                out.success,
                "fixture build {spec:?} failed:\n{}\n{}",
                out.stderr,
                lines.join("\n")
            );
            dll_of(&lines).unwrap_or_else(|| panic!("no app DLL in cargo's output for {spec:?}"))
        }

        fn dll_of(lines: &[String]) -> Option<PathBuf> {
            lines.iter().find_map(|line| {
                let message: serde_json::Value = serde_json::from_str(line).ok()?;
                if message["reason"] != "compiler-artifact"
                    || message["target"]["name"] != APP_CRATE
                {
                    return None;
                }
                message["filenames"].as_array()?.iter().find_map(|file| {
                    let file = PathBuf::from(file.as_str()?);
                    let is_dll = file
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"));
                    is_dll.then_some(file)
                })
            })
        }
    }
}
