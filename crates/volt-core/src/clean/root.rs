//! Turning a rule's root into a real folder, and refusing the dangerous ones.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use super::folders::{KnownFolders, Location};
use super::rule::{CleaningRule, is_output_folder_name};
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
    /// The rule works in a folder the user chooses and none is chosen yet.
    FolderNotChosen,
    /// The chosen folder is, or lies in, an output folder (`output`), which is never cleaned.
    OutputFolder,
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
            Self::FolderNotChosen => f.write_str("no folder is chosen for this rule yet"),
            Self::OutputFolder => {
                f.write_str("the chosen folder is an output folder, which is never cleaned")
            }
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

/// The root of a rule, and the environment variable it was found through, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedRoot {
    pub(super) path: PathBuf,
    /// The variable that moved the root (see [`Follow`](super::Follow)).
    pub(super) followed: Option<String>,
}

/// Whether the folder an environment variable names can be where a tool keeps its cache: an
/// absolute path without `.` or `..`, not a drive root, not a protected folder or one that
/// contains a protected folder (the profile, `AppData`), and not inside a system folder.
fn plausible_cache_folder(folder: &Path, folders: &dyn KnownFolders) -> bool {
    if !folder.is_absolute()
        || folder
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || !folder
            .components()
            .any(|c| matches!(c, Component::Normal(_)))
    {
        return false;
    }
    let Ok(protected) = folders.protected_folders() else {
        return false;
    };
    if protected
        .iter()
        .any(|protected| starts_with_ci(protected, folder))
    {
        return false;
    }
    // A system folder list the adapter cannot give is not a reason to refuse: the protected list
    // above already holds the Windows directory and Program Files.
    folders.system_folders().map_or(true, |system| {
        system.iter().all(|s| !starts_with_ci(folder, s))
    })
}

/// The root a tool's documented variable moves the rule to, when one is set to a plausible folder.
fn followed_root(rule: &CleaningRule, folders: &dyn KnownFolders) -> Option<ResolvedRoot> {
    rule.follow.iter().find_map(|follow| {
        let base = folders.env_folder(&follow.var)?;
        if !plausible_cache_folder(&base, folders) {
            return None;
        }
        let mut path = base;
        path.extend(&follow.path);
        Some(ResolvedRoot {
            path,
            followed: Some(follow.var.clone()),
        })
    })
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
    chosen: Option<&Path>,
) -> Result<ResolvedRoot, RootError> {
    if let Some(found) = followed_root(rule, folders) {
        return Ok(found);
    }
    let base = if rule.root.location == Location::ChosenFolder {
        let chosen = chosen.ok_or(RootError::FolderNotChosen)?;
        if chosen
            .components()
            .any(|c| is_output_folder_name(&c.as_os_str().to_string_lossy()))
        {
            return Err(RootError::OutputFolder);
        }
        // A folder the user chose is judged like a folder a variable names: never a drive root,
        // the profile, a system folder or one that holds protected data.
        if !plausible_cache_folder(chosen, folders) {
            return Err(RootError::TooBroad);
        }
        chosen.to_path_buf()
    } else {
        folders
            .location(rule.root.location)
            .map_err(|e| RootError::Unavailable(error_class(&e)))?
    };
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
    Ok(ResolvedRoot {
        path: root,
        followed: None,
    })
}
