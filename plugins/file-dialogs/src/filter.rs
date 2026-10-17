//! File-type filters: a display name plus a normalised extension list.

/// A named set of file extensions shown in the dialog's type selector.
///
/// Filters are hints to the dialog, not a security boundary: the user can
/// often override them, and returned paths must be validated by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFilter {
    name: String,
    extensions: Vec<String>,
}

impl FileFilter {
    /// Builds a filter. Extensions are normalised: a leading `*.` or `.` is
    /// stripped, case is folded to lowercase, and empty or duplicate entries
    /// are dropped (`"*.PNG"`, `".png"` and `"png"` all become `png`).
    pub fn new(name: &str, extensions: &[&str]) -> Self {
        let mut out: Vec<String> = Vec::new();
        for raw in extensions {
            let ext = normalize_extension(raw);
            if !ext.is_empty() && !out.contains(&ext) {
                out.push(ext);
            }
        }
        Self {
            name: name.trim().to_owned(),
            extensions: out,
        }
    }

    /// The display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The normalised extensions, without dots.
    pub fn extensions(&self) -> &[String] {
        &self.extensions
    }
}

fn normalize_extension(raw: &str) -> String {
    let t = raw.trim();
    let t = t.strip_prefix("*.").unwrap_or(t);
    t.trim_start_matches('.').to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_are_normalised() {
        let f = FileFilter::new(" Images ", &["*.PNG", ".jpg", "png", "", " gif "]);
        assert_eq!(f.name(), "Images");
        assert_eq!(f.extensions(), ["png", "jpg", "gif"]);
    }

    #[test]
    fn empty_extension_list_is_kept_empty() {
        assert!(FileFilter::new("None", &["", "."]).extensions().is_empty());
    }
}
