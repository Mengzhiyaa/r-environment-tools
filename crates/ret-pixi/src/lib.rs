// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ret_core::{
    arch::Architecture,
    env::REnv,
    r_installation::{RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Locator, LocatorKind,
};
use ret_r_utils::executable::find_executables;
use std::path::{Path, PathBuf};

/// Returns `true` if the given path is a Pixi environment.
///
/// Pixi environments are conda environments that contain a `conda-meta/pixi`
/// marker file. This distinguishes them from plain conda environments.
pub fn is_pixi_env(path: &Path) -> bool {
    path.join("conda-meta").join("pixi").is_file()
}

/// Attempts to find the Pixi environment prefix from the R executable path.
///
/// Pixi environments live under `<project>/.pixi/envs/<env_name>/`.
/// The R executable is typically at `<prefix>/lib/R/bin/R` or `<prefix>/bin/R`.
fn get_pixi_prefix(env: &REnv) -> Option<PathBuf> {
    // Walk up from the executable looking for the pixi marker.
    for ancestor in env.executable.ancestors().skip(1) {
        if is_pixi_env(ancestor) {
            return Some(ancestor.to_path_buf());
        }
    }

    // Also check from R home directory, if available.
    if let Some(home) = &env.home {
        for ancestor in home.ancestors().skip(1) {
            if is_pixi_env(ancestor) {
                return Some(ancestor.to_path_buf());
            }
        }
    }

    None
}

pub struct Pixi {}

impl Pixi {
    pub fn new() -> Pixi {
        Pixi {}
    }
}

impl Default for Pixi {
    fn default() -> Self {
        Self::new()
    }
}

impl Locator for Pixi {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::Pixi
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::Pixi]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        env.version.as_ref()?;

        let prefix = get_pixi_prefix(env)?;
        if !is_pixi_env(&prefix) {
            return None;
        }

        let name = prefix
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();

        let display_name = if name.is_empty() || name == "default" {
            "Pixi R".to_string()
        } else {
            format!("Pixi R ({name})")
        };

        let home = env.home.clone();

        let mut symlinks = env.symlinks.clone().unwrap_or_default();
        symlinks.extend(find_executables(&prefix));

        Some(
            RInstallationBuilder::new(Some(RInstallationKind::Pixi))
                .display_name(Some(display_name))
                .name(Some(name))
                .executable(Some(env.executable.clone()))
                .home(home)
                .version(env.version.clone())
                .arch(
                    env.arch
                        .clone()
                        .or_else(|| Some(Architecture::infer_from_path(&env.executable))),
                )
                .symlinks(Some(symlinks))
                .build(),
        )
    }

    /// Pixi environments are workspace-local (`.pixi/envs/<name>/`).
    /// They are discovered during workspace directory scanning in `find.rs`,
    /// not via a global search. This method is intentionally empty.
    fn find(&self, _reporter: &dyn Reporter) {}
}

#[cfg(test)]
mod tests {
    use super::{get_pixi_prefix, is_pixi_env};
    use std::path::PathBuf;

    #[test]
    fn pixi_env_detection() {
        let tmp = tempfile::TempDir::new().expect("failed to create tempdir");
        let prefix = tmp.path().join(".pixi").join("envs").join("default");
        let conda_meta = prefix.join("conda-meta");
        std::fs::create_dir_all(&conda_meta).expect("failed to create conda-meta");

        // Without the pixi marker, it's NOT a pixi env.
        assert!(!is_pixi_env(&prefix));

        // With the pixi marker, it IS a pixi env.
        std::fs::write(conda_meta.join("pixi"), "").expect("failed to create pixi marker");
        assert!(is_pixi_env(&prefix));
    }

    #[test]
    fn get_prefix_from_executable() {
        let tmp = tempfile::TempDir::new().expect("failed to create tempdir");
        let prefix = tmp.path().join(".pixi").join("envs").join("default");
        let conda_meta = prefix.join("conda-meta");
        let bin = prefix.join("lib").join("R").join("bin");
        std::fs::create_dir_all(&conda_meta).expect("failed to create conda-meta");
        std::fs::create_dir_all(&bin).expect("failed to create bin dir");
        std::fs::write(conda_meta.join("pixi"), "").expect("failed to create pixi marker");

        let executable = bin.join("R");
        std::fs::write(&executable, "").expect("failed to create fake R");

        let env = ret_core::env::REnv::new(executable, None, None);
        let found = get_pixi_prefix(&env);
        assert_eq!(found, Some(prefix));
    }

    #[test]
    fn non_pixi_path_returns_none() {
        let env = ret_core::env::REnv::new(PathBuf::from("/usr/bin/R"), None, None);
        assert_eq!(get_pixi_prefix(&env), None);
    }
}
