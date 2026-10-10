//! The source edits the canary makes to the running app and to its local
//! path dependency, and the guard that puts each file back.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// One edit: every `(old, new)` replacement must match exactly once, so a
/// drifted fixture fails the canary instead of patching something else.
pub struct Edit {
    pub name: &'static str,
    pub replacements: &'static [(&'static str, &'static str)],
}

/// Changes the hot function's return value: a body-only edit L3 must pass.
pub const VALUE_EDIT: Edit = Edit {
    name: "value edit (`reading()` returns 2)",
    replacements: &[(
        "let value = 1 + offset.value;",
        "let value = 2 + offset.value;",
    )],
};

/// The value the app must answer once [`VALUE_EDIT`] is applied.
pub const EDITED_VALUE: u64 = 2;

/// Changes the path dependency's `offset()` value: a body-only edit outside
/// the workspace, which must replay the dependency and the app and pass L3.
pub const SHARED_VALUE_EDIT: Edit = Edit {
    name: "shared value edit (`offset()` returns 1)",
    replacements: &[("Offset { value: 0 }", "Offset { value: 1 }")],
};

/// The value the app must answer once [`SHARED_VALUE_EDIT`] is applied too.
pub const SHARED_EDITED_VALUE: u64 = 3;

/// RESULTS.md row D2's shape: a field added to a type whose values cross the
/// seam (`Reading`, the hot function's return). L3 must refuse it.
pub const D2_EDIT: Edit = Edit {
    name: "D2 edit (`Reading` gains a field)",
    replacements: &[
        (
            "    pub offset: Offset,\n}",
            "    pub offset: Offset,\n    pub extra: u64,\n}",
        ),
        (
            "value = 2 + offset.value;\n    Reading { value, offset }",
            "value = 2 + offset.value;\n    Reading { value, offset, extra: 0 }",
        ),
    ],
};

/// The type D2 changes, as the layout table names it.
pub const D2_TYPE: &str = "hotpatch_canary_app::Reading";

/// D2's shape in the path dependency: a field added to the type the app's
/// `Reading` holds by value. L3 must refuse it.
pub const SHARED_D2_EDIT: Edit = Edit {
    name: "shared D2 edit (`Offset` gains a field)",
    replacements: &[
        (
            "    pub value: u64,\n}",
            "    pub value: u64,\n    pub extra: u64,\n}",
        ),
        ("Offset { value: 1 }", "Offset { value: 1, extra: 0 }"),
    ],
};

/// The type the shared D2 edit changes, as the layout table names it.
pub const SHARED_D2_TYPE: &str = "hotpatch_canary_shared::Offset";

impl Edit {
    /// `text` with every replacement made.
    pub fn apply_to(&self, text: &str) -> Result<String> {
        let mut text = text.to_string();
        for (old, new) in self.replacements {
            let found = text.matches(old).count();
            if found != 1 {
                bail!(
                    "{}: `{}` occurs {found} times in the fixture, expected exactly once",
                    self.name,
                    old.escape_debug()
                );
            }
            text = text.replacen(old, new, 1);
        }
        Ok(text)
    }
}

/// Holds a source file's original text and writes it back when dropped, so
/// an edit never outlives the canary run (a panic included).
pub struct SourceGuard {
    path: PathBuf,
    original: String,
    current: String,
}

impl SourceGuard {
    pub fn new(path: &Path) -> Result<Self> {
        let original = std::fs::read_to_string(path)
            .with_context(|| format!("reading `{}`", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            current: original.clone(),
            original,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Makes `edit` on top of the edits already made.
    pub fn apply(&mut self, edit: &Edit) -> Result<()> {
        let next = edit.apply_to(&self.current)?;
        std::fs::write(&self.path, &next)
            .with_context(|| format!("writing `{}`", self.path.display()))?;
        self.current = next;
        Ok(())
    }
}

impl Drop for SourceGuard {
    fn drop(&mut self) {
        if self.current != self.original
            && let Err(err) = std::fs::write(&self.path, &self.original)
        {
            eprintln!("canary: could not restore `{}`: {err}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../app/src/lib.rs");
    const SHARED_FIXTURE: &str = include_str!("../../shared/src/lib.rs");

    #[test]
    fn both_edits_apply_to_the_fixture_in_order() {
        let valued = VALUE_EDIT.apply_to(FIXTURE).unwrap();
        assert!(valued.contains("let value = 2 + offset.value;"));
        let d2 = D2_EDIT.apply_to(&valued).unwrap();
        assert!(d2.contains("pub extra: u64,"));
        assert!(d2.contains("Reading { value, offset, extra: 0 }"));
    }

    #[test]
    fn both_shared_edits_apply_to_the_path_dependency_in_order() {
        let valued = SHARED_VALUE_EDIT.apply_to(SHARED_FIXTURE).unwrap();
        assert!(valued.contains("Offset { value: 1 }"));
        let d2 = SHARED_D2_EDIT.apply_to(&valued).unwrap();
        assert!(d2.contains("pub extra: u64,"));
        assert!(d2.contains("Offset { value: 1, extra: 0 }"));
        let err = SHARED_D2_EDIT.apply_to(SHARED_FIXTURE).unwrap_err();
        assert!(err.to_string().contains("occurs 0 times"), "{err}");
    }

    #[test]
    fn the_d2_edit_needs_the_value_edit_first() {
        let err = D2_EDIT.apply_to(FIXTURE).unwrap_err().to_string();
        assert!(err.contains("occurs 0 times"), "{err}");
    }

    #[test]
    fn the_guard_restores_the_original_text() {
        let dir = std::env::temp_dir().join(format!("canary-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("lib.rs");
        std::fs::write(&file, FIXTURE).unwrap();
        {
            let mut guard = SourceGuard::new(&file).unwrap();
            guard.apply(&VALUE_EDIT).unwrap();
            assert_ne!(std::fs::read_to_string(&file).unwrap(), FIXTURE);
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), FIXTURE);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
