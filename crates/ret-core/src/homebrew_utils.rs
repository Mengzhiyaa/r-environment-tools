//! Shared path utilities for Homebrew-related detection.

use std::path::Path;

/// Returns `true` if the given path looks like it belongs to a Homebrew installation.
pub fn looks_like_homebrew_path(path: &Path) -> bool {
    let path = path.to_string_lossy();
    path.contains("/Cellar/r/")
        || path.contains("/Cellar/r@")
        || path.contains("/home/linuxbrew/.linuxbrew/")
        || path.contains("/opt/homebrew/")
}
