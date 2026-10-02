//! Path-traversal defence for every file the server touches.
//!
//! Video ids are opaque and resolved through the store, so clients never send
//! a path. Still, the server treats the stored relative path as untrusted:
//! it is canonicalized and proven to live under its configured root before a
//! single byte is read. Symlinks that point outside a root are therefore
//! rejected too, because `canonicalize` resolves them.

use std::path::{Component, Path, PathBuf};

use crate::error::{Result, ServerError};

/// One configured library root (the original, possibly not-yet-existing path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRoot {
    original: PathBuf,
}

impl LibraryRoot {
    /// The configured (uncanonicalized) path.
    pub fn path(&self) -> &Path {
        &self.original
    }
}

/// The set of authorized library roots, indexed by `root_index`.
#[derive(Debug, Clone, Default)]
pub struct LibraryRoots {
    roots: Vec<LibraryRoot>,
}

impl LibraryRoots {
    /// Builds the root set from configured paths. Roots need not exist yet:
    /// resolution fails closed until they do.
    pub fn new(paths: &[PathBuf]) -> Self {
        Self {
            roots: paths
                .iter()
                .cloned()
                .map(|original| LibraryRoot { original })
                .collect(),
        }
    }

    /// The configured root at `index`.
    pub fn get(&self, index: usize) -> Option<&LibraryRoot> {
        self.roots.get(index)
    }

    /// Resolves `relative_path` under the root at `root_index`, returning a
    /// canonical absolute path proven to be a regular file under that root.
    pub fn resolve(&self, root_index: i64, relative_path: &str) -> Result<PathBuf> {
        let root = self
            .roots
            .get(usize::try_from(root_index).map_err(|_| {
                ServerError::PathRejected(format!("negative root index {root_index}"))
            })?)
            .ok_or_else(|| ServerError::PathRejected(format!("unknown root index {root_index}")))?;

        let relative = self.validate_relative(relative_path)?;

        let canonical_root = std::fs::canonicalize(root.path()).map_err(|error| {
            ServerError::PathRejected(format!("library root is unavailable: {error}"))
        })?;
        let candidate = canonical_root.join(relative);
        let canonical = std::fs::canonicalize(&candidate)
            .map_err(|error| ServerError::PathRejected(format!("file is unavailable: {error}")))?;

        if !canonical.starts_with(&canonical_root) {
            return Err(ServerError::PathEscape {
                path: canonical,
                root: canonical_root,
            });
        }
        if !canonical.is_file() {
            return Err(ServerError::PathRejected(
                "resolved path is not a regular file".into(),
            ));
        }
        Ok(canonical)
    }

    /// Rejects absolute paths, drive/UNC prefixes and any `..` component.
    fn validate_relative<'a>(&self, relative_path: &'a str) -> Result<&'a Path> {
        if relative_path.is_empty() {
            return Err(ServerError::PathRejected("empty relative path".into()));
        }
        if relative_path.contains('\0') {
            return Err(ServerError::PathRejected("path contains a NUL byte".into()));
        }
        let path = Path::new(relative_path);
        if path.is_absolute() {
            return Err(ServerError::PathRejected(
                "absolute paths are not permitted".into(),
            ));
        }
        for component in path.components() {
            match component {
                Component::Normal(_) | Component::CurDir => {}
                Component::ParentDir => {
                    return Err(ServerError::PathRejected(
                        "'..' components are not permitted".into(),
                    ));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(ServerError::PathRejected(
                        "rooted paths are not permitted".into(),
                    ));
                }
            }
        }
        Ok(path)
    }
}

#[cfg(test)]
mod tests;
