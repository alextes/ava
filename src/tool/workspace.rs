use std::path::PathBuf;

use crate::config;

/// returns true if the given path is inside the workspace root.
/// empty path or "." resolves to cwd which is inside the workspace.
/// resolves relative paths against cwd and canonicalizes (following symlinks).
/// for non-existent paths, canonicalizes the nearest existing ancestor.
pub fn is_inside_workspace(path: &str) -> bool {
    let workspace = config::workspace_root();

    if path.is_empty() || path == "." {
        return true;
    }

    let p = PathBuf::from(path);
    let resolved = if p.is_absolute() {
        p
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(&p),
            Err(_) => return false,
        }
    };

    // try to canonicalize the full path first
    if let Ok(canonical) = resolved.canonicalize() {
        return canonical.starts_with(workspace);
    }

    // path doesn't exist — walk up to find the nearest existing ancestor
    let mut ancestor = resolved.as_path();
    loop {
        match ancestor.parent() {
            Some(parent) => {
                if let Ok(canonical) = parent.canonicalize() {
                    return canonical.starts_with(workspace);
                }
                ancestor = parent;
            }
            None => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_path_is_inside() {
        config::init_workspace_root();
        assert!(is_inside_workspace(""));
    }

    #[test]
    fn test_dot_is_inside() {
        config::init_workspace_root();
        assert!(is_inside_workspace("."));
    }

    #[test]
    fn test_relative_path_inside_workspace() {
        config::init_workspace_root();
        // "src/main.rs" relative to cwd should be inside workspace
        assert!(is_inside_workspace("src/main.rs"));
    }

    #[test]
    fn test_absolute_path_outside_workspace() {
        config::init_workspace_root();
        assert!(!is_inside_workspace("/etc/passwd"));
    }

    #[test]
    fn test_absolute_path_inside_workspace() {
        config::init_workspace_root();
        let workspace = config::workspace_root();
        let inside = workspace.join("src/main.rs");
        assert!(is_inside_workspace(inside.to_str().unwrap()));
    }

    #[test]
    fn test_nonexistent_path_inside_workspace() {
        config::init_workspace_root();
        // a non-existent file under workspace should still be considered inside
        assert!(is_inside_workspace("src/nonexistent_file_xyz.rs"));
    }

    #[test]
    fn test_nonexistent_path_outside_workspace() {
        config::init_workspace_root();
        assert!(!is_inside_workspace("/tmp/nonexistent_ava_test_xyz/foo.rs"));
    }
}
