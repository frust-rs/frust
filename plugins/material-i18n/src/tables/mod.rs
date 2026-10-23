//! One hand-written [`MaterialStrings`](frust_material::MaterialStrings)
//! table per supported language. Month and weekday names are not here: the
//! tables carry placeholders for them and [`crate::material_strings_for`]
//! fills them from CLDR.

pub(crate) mod de;
pub(crate) mod en;
pub(crate) mod es;
pub(crate) mod fr;
pub(crate) mod ja;
pub(crate) mod th;
pub(crate) mod vi;
