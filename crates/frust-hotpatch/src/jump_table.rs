// Ported from subsecond-types 0.7.10 (DioxusLabs, MIT OR Apache-2.0),
// https://github.com/DioxusLabs/dioxus/tree/main/packages/subsecond.
//
//! The jump table a patch builder sends: the wire type dx 0.7.10 serializes inside its devserver
//! `HotReloadMsg`. Field names and types match `subsecond_types::JumpTable` exactly, so the JSON
//! dx emits deserializes into [`JumpTable`] unchanged (the `wire_compat` test below pins that).

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One patch: the library holding the new code and the old -> new address map into it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct JumpTable {
    /// The patch dylib, a path the platform loader can open directly.
    pub lib: PathBuf,

    /// Old function address -> new function address, both as link-time addresses: neither the
    /// running binary's ASLR slide nor the patch's load address is applied yet
    /// ([`apply_patch`](crate::apply_patch) rebases both sides).
    pub map: AddressMap,

    /// Link-time address of the running binary's `main`, the anchor that recovers its ASLR slide.
    pub aslr_reference: u64,

    /// Link-time address of the patch library's `main`, the anchor that recovers its load address.
    pub new_base_address: u64,

    /// Indirect-function count; only a wasm loader reads it, kept so the wire shape is identical.
    pub ifunc_count: u64,
}

/// An address -> address map that does not hash its keys: addresses are already unique.
pub type AddressMap = HashMap<u64, u64, BuildAddressHasher>;

/// The [`AddressMap`] hasher builder.
pub type BuildAddressHasher = BuildHasherDefault<AddressHasher>;

/// Identity hasher for integer keys; [`AddressMap`] only ever feeds it a `u64`.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct AddressHasher(u64);

impl Hasher for AddressHasher {
    fn write(&mut self, _: &[u8]) {
        panic!("AddressHasher only hashes integer keys")
    }
    fn write_u8(&mut self, n: u8) {
        self.0 = u64::from(n)
    }
    fn write_u16(&mut self, n: u16) {
        self.0 = u64::from(n)
    }
    fn write_u32(&mut self, n: u32) {
        self.0 = u64::from(n)
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = n
    }
    fn write_usize(&mut self, n: usize) {
        self.0 = n as u64
    }
    fn write_i8(&mut self, n: i8) {
        self.0 = n as u64
    }
    fn write_i16(&mut self, n: i16) {
        self.0 = n as u64
    }
    fn write_i32(&mut self, n: i32) {
        self.0 = n as u64
    }
    fn write_i64(&mut self, n: i64) {
        self.0 = n as u64
    }
    fn write_isize(&mut self, n: isize) {
        self.0 = n as u64
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upstream_table() -> subsecond_types::JumpTable {
        let mut map = subsecond_types::AddressMap::default();
        map.insert(0x1_0000_1234, 0x4567);
        map.insert(0x1_0000_9abc, 0xdef0);
        map.insert(u64::MAX, 0);
        subsecond_types::JumpTable {
            lib: PathBuf::from("/tmp/patch-1.dylib"),
            map,
            aslr_reference: 0x1_0000_0000,
            new_base_address: 0x2000,
            ifunc_count: 3,
        }
    }

    #[test]
    fn wire_compat_with_subsecond_types() {
        let upstream = upstream_table();
        let json = serde_json::to_value(&upstream).expect("upstream table serializes");

        let ours: JumpTable = serde_json::from_value(json.clone()).expect("dx's JSON deserializes");
        assert_eq!(ours.lib, upstream.lib);
        assert_eq!(ours.aslr_reference, upstream.aslr_reference);
        assert_eq!(ours.new_base_address, upstream.new_base_address);
        assert_eq!(ours.ifunc_count, upstream.ifunc_count);
        assert_eq!(ours.map.len(), upstream.map.len());
        for (old, new) in &upstream.map {
            assert_eq!(ours.map.get(old), Some(new));
        }

        // Back out: the JSON value is equal (map key order is not part of a JSON object identity).
        let back = serde_json::to_value(&ours).expect("our table serializes");
        assert_eq!(back, json);
        let round: subsecond_types::JumpTable =
            serde_json::from_value(back).expect("upstream reads our JSON");
        assert_eq!(round, upstream);
    }

    #[test]
    fn address_hasher_is_identity() {
        let mut h = AddressHasher::default();
        h.write_u64(0xdead_beef);
        assert_eq!(h.finish(), 0xdead_beef);
    }
}
