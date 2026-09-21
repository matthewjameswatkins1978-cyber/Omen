//! Canonical authority for filesystem resource identity.
//!
//! Presentation (a caller path or `file://` URI) is deliberately kept
//! separate from the identity used for containment and semantic comparison.

use crate::error::{CoreError, ErrorCode};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresentedResource {
    Path(String),
    FileUri(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedResource {
    pub presented: PresentedResource,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalResource {
    pub presented: PresentedResource,
    pub normalized_path: PathBuf,
    pub physical_path: Option<PathBuf>,
    pub workspace_relative: Option<PathBuf>,
    pub identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceAuthority {
    workspace_root: PathBuf,
    workspace_identity: String,
}

impl ResourceAuthority {
    pub fn new(workspace_root: impl AsRef<Path>) -> Result<Self, CoreError> {
        let root = workspace_root.as_ref();
        let physical = root.canonicalize().map_err(|error| {
            CoreError::NotFound(format!(
                "workspace root '{}' cannot be resolved: {error}",
                root.display()
            ))
        })?;
        let workspace_identity = path_key(&physical);
        Ok(Self {
            workspace_root: physical,
            workspace_identity,
        })
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn normalize(&self, presented: &str) -> Result<NormalizedResource, CoreError> {
        let (presented_form, raw_path) = if presented
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file://"))
        {
            (
                PresentedResource::FileUri(presented.to_owned()),
                parse_file_uri(presented)?,
            )
        } else {
            (
                PresentedResource::Path(presented.to_owned()),
                PathBuf::from(presented),
            )
        };

        let joined = if raw_path.is_absolute() {
            raw_path
        } else {
            self.workspace_root.join(raw_path)
        };
        let normalized_path = lexical_normalize(&joined);
        Ok(NormalizedResource {
            presented: presented_form,
            path: normalized_path,
        })
    }

    pub fn resolve(&self, presented: &str) -> Result<CanonicalResource, CoreError> {
        let normalized = self.normalize(presented)?;
        let physical_path = normalized.path.canonicalize().ok();
        let authority_path = physical_path.as_ref().unwrap_or(&normalized.path);
        let workspace_relative = workspace_relative_path(authority_path, &self.workspace_root);

        if workspace_relative.is_none() {
            return Err(CoreError::ExecutionFailedCode {
                code: ErrorCode::WorkspaceScopeViolation,
                message: format!("resource outside workspace scope: '{}'", presented),
            });
        }

        let identity_path = workspace_relative
            .as_ref()
            .map(|relative| path_key(relative))
            .unwrap_or_else(|| path_key(authority_path));
        Ok(CanonicalResource {
            presented: normalized.presented,
            normalized_path: normalized.path,
            physical_path,
            workspace_relative,
            identity: format!("workspace:{}:{}", self.workspace_identity, identity_path),
        })
    }

    pub fn resolve_existing(&self, presented: &str) -> Result<CanonicalResource, CoreError> {
        let resource = self.resolve(presented)?;
        if resource.physical_path.is_none() {
            return Err(CoreError::NotFound(format!(
                "resource does not exist: '{}'",
                presented
            )));
        }
        Ok(resource)
    }

    pub fn to_file_uri(&self, presented: &str) -> Result<String, CoreError> {
        let normalized = self.normalize(presented)?;
        let path = normalized.path.to_string_lossy().replace('\\', "/");
        #[cfg(windows)]
        let path = path
            .strip_prefix("//?/")
            .unwrap_or(&path)
            .trim_start_matches('/')
            .to_owned();
        #[cfg(not(windows))]
        let path = path.trim_start_matches('/').to_owned();
        Ok(format!("file:///{path}"))
    }

    pub fn workspace_relative(&self, presented: &str) -> Result<String, CoreError> {
        let resource = self.resolve(presented)?;
        resource
            .workspace_relative
            .map(|path| path_key(&path))
            .ok_or_else(|| CoreError::ExecutionFailed("resource outside workspace scope".into()))
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn workspace_relative_path(path: &Path, workspace_root: &Path) -> Option<PathBuf> {
    if let Ok(relative) = path.strip_prefix(workspace_root) {
        return Some(relative.to_path_buf());
    }
    #[cfg(windows)]
    {
        let path_text = path_key(path);
        let mut root_text = path_key(workspace_root);
        if let Some(stripped) = root_text.strip_prefix("//?/") {
            root_text = stripped.to_owned();
        }
        let root_text = root_text.trim_end_matches('/').to_owned();
        let path_lower = path_text.to_ascii_lowercase();
        let root_lower = root_text.to_ascii_lowercase();
        if path_lower == root_lower {
            return Some(PathBuf::new());
        }
        if path_lower.starts_with(&(root_lower.clone() + "/")) {
            return Some(PathBuf::from(&path_text[root_text.len() + 1..]));
        }
    }
    None
}

fn parse_file_uri(uri: &str) -> Result<PathBuf, CoreError> {
    let remainder = uri
        .get(7..)
        .ok_or_else(|| CoreError::InvalidUri(format!("invalid file URI: '{uri}'")))?;
    let (authority, path) = remainder.split_once('/').unwrap_or((remainder, ""));
    if authority.is_empty() || authority.eq_ignore_ascii_case("localhost") {
        let decoded = percent_decode(path)?;
        #[cfg(windows)]
        {
            return Ok(PathBuf::from(decoded.trim_start_matches('/')));
        }
        #[cfg(not(windows))]
        {
            return Ok(PathBuf::from(format!("/{decoded}")));
        }
    }

    #[cfg(windows)]
    {
        let decoded = percent_decode(path)?;
        Ok(PathBuf::from(format!(r"\\{}\{}", authority, decoded)))
    }
    #[cfg(not(windows))]
    {
        let _ = authority;
        Err(CoreError::InvalidUri(format!(
            "file URI authority is unsupported on this host: '{uri}'"
        )))
    }
}

fn percent_decode(value: &str) -> Result<String, CoreError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(CoreError::InvalidUri(format!(
                    "invalid percent escape in file URI: '{value}'"
                )));
            }
            let hi = (bytes[index + 1] as char).to_digit(16);
            let lo = (bytes[index + 2] as char).to_digit(16);
            let (Some(hi), Some(lo)) = (hi, lo) else {
                return Err(CoreError::InvalidUri(format!(
                    "invalid percent escape in file URI: '{value}'"
                )));
            };
            decoded.push((hi * 16 + lo) as u8);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded)
        .map_err(|_| CoreError::InvalidUri("file URI is not valid UTF-8".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn relative_and_file_uri_converge() {
        let workspace = tempdir().unwrap();
        fs::create_dir_all(workspace.path().join("src")).unwrap();
        fs::write(workspace.path().join("src/lib.rs"), "fn f() {}").unwrap();
        let authority = ResourceAuthority::new(workspace.path()).unwrap();
        let relative = authority
            .resolve_existing("src/./module/../lib.rs")
            .unwrap();
        let uri = format!(
            "file:///{}",
            workspace
                .path()
                .join("src/lib.rs")
                .to_string_lossy()
                .replace('\\', "/")
                .trim_start_matches('/')
        );
        let from_uri = authority.resolve_existing(&uri).unwrap();
        assert_eq!(relative.identity, from_uri.identity);
        assert_eq!(
            relative.workspace_relative,
            Some(PathBuf::from("src/lib.rs"))
        );
    }

    #[test]
    fn same_relative_path_in_two_workspaces_is_distinct() {
        let first = tempdir().unwrap();
        let second = tempdir().unwrap();
        fs::write(first.path().join("same.rs"), "one").unwrap();
        fs::write(second.path().join("same.rs"), "two").unwrap();
        let a = ResourceAuthority::new(first.path())
            .unwrap()
            .resolve_existing("same.rs")
            .unwrap();
        let b = ResourceAuthority::new(second.path())
            .unwrap()
            .resolve_existing("same.rs")
            .unwrap();
        assert_ne!(a.identity, b.identity);
    }

    #[test]
    fn escape_is_not_in_workspace_identity() {
        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        fs::write(outside.path().join("outside.rs"), "outside").unwrap();
        let authority = ResourceAuthority::new(workspace.path()).unwrap();
        let escaped = workspace
            .path()
            .join("..")
            .join(outside.path().file_name().unwrap())
            .join("outside.rs");
        assert!(matches!(
            authority.resolve_existing(escaped.to_str().unwrap()),
            Err(CoreError::ExecutionFailedCode {
                code: ErrorCode::WorkspaceScopeViolation,
                ..
            })
        ));
    }

    #[test]
    fn invalid_file_uri_is_structured() {
        let workspace = tempdir().unwrap();
        let authority = ResourceAuthority::new(workspace.path()).unwrap();
        assert!(matches!(
            authority.resolve("file:///%ZZ"),
            Err(CoreError::InvalidUri(_))
        ));
    }
}
