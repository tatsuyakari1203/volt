//! Turning a rule's root into a real folder, and refusing the dangerous ones.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use super::folders::KnownFolders;
use super::rule::CleaningRule;
use crate::activity::error_class;

/// Why a rule's root cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RootError {
    /// The location could not be resolved; the text is the error class of the
    /// port (such as "not found"), never its message.
    Unavailable(&'static str),
    /// The resolved folder is not an absolute path.
    NotAbsolute,
    /// The root is a drive root, a top-level folder (such as `C:\Windows`) or
    /// contains `.` or `..` components.
    TooBroad,
    /// The root is, or lies above, a protected folder such as the user
    /// profile. Holds the protected folder.
    Protected(PathBuf),
}

impl fmt::Display for RootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(class) => write!(f, "the location could not be resolved ({class})"),
            Self::NotAbsolute => f.write_str("the location is not an absolute path"),
            Self::TooBroad => {
                f.write_str("the root is a drive root or a top-level folder, which is refused")
            }
            Self::Protected(folder) => write!(
                f,
                "the root is, or contains, the protected folder {}",
                folder.display()
            ),
        }
    }
}

/// Lower-cased components, for comparisons that ignore case.
fn folded(path: &Path) -> Vec<String> {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

/// Whether `path` is `base` or below it, ignoring case.
pub(crate) fn starts_with_ci(path: &Path, base: &Path) -> bool {
    let (path, base) = (folded(path), folded(base));
    path.len() >= base.len() && path[..base.len()] == base[..]
}

/// Whether two paths are the same, ignoring case.
pub(crate) fn same_path_ci(a: &Path, b: &Path) -> bool {
    folded(a) == folded(b)
}

/// The folder a rule starts from.
///
/// Refused: a location that cannot be resolved or is not absolute, a root that
/// is a drive root or a top-level folder, and a root that equals or contains a
/// protected folder (the profile, the Windows directory, Program Files, ...).
/// This runs on every scan and again before every clean, so a rule is judged
/// against the machine as it is now.
pub(super) fn resolve_root(
    rule: &CleaningRule,
    folders: &dyn KnownFolders,
) -> Result<PathBuf, RootError> {
    let base = folders
        .location(rule.root.location)
        .map_err(|e| RootError::Unavailable(error_class(&e)))?;
    if !base.is_absolute() {
        return Err(RootError::NotAbsolute);
    }
    let mut root = base;
    for part in &rule.root.relative {
        root.push(part);
    }
    if root
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(RootError::TooBroad);
    }
    // Drive root: depth 0. A top-level folder such as C:\Windows: depth 1.
    let depth = root
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .count();
    if depth < 2 {
        return Err(RootError::TooBroad);
    }
    let protected = folders
        .protected_folders()
        .map_err(|e| RootError::Unavailable(error_class(&e)))?;
    if let Some(folder) = protected
        .into_iter()
        .find(|folder| starts_with_ci(folder, &root))
    {
        return Err(RootError::Protected(folder));
    }
    Ok(root)
}
