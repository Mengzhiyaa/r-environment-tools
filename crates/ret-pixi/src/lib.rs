// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ret_core::{
    arch::Architecture,
    env::REnv,
    os_environment::{Environment, EnvironmentApi},
    r_installation::{LocatorMetadata, RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Locator, LocatorKind,
};
use ret_r_utils::{
    env::ResolvedRInstallation,
    executable::{filter_symlink_paths, find_executable, find_executables},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Returns `true` if the given path is a Pixi environment.
///
/// Pixi environments are conda environments that contain a `conda-meta/pixi`
/// marker file. This distinguishes them from plain conda environments.
pub fn is_pixi_env(path: &Path) -> bool {
    path.join("conda-meta").join("pixi").is_file()
}

fn infer_manifest_path(prefix: &Path) -> Option<PathBuf> {
    let envs_dir = prefix.parent()?;
    if envs_dir.file_name()?.to_string_lossy() != "envs" {
        return None;
    }

    let pixi_dir = envs_dir.parent()?;
    if pixi_dir.file_name()?.to_string_lossy() != ".pixi" {
        return None;
    }

    let project_root = pixi_dir.parent()?;
    let pixi_toml = project_root.join("pixi.toml");
    if pixi_toml.is_file() {
        return Some(pixi_toml);
    }

    let pyproject = project_root.join("pyproject.toml");
    if pyproject.is_file()
        && fs::read_to_string(&pyproject)
            .map(|content| content.contains("[tool.pixi]"))
            .unwrap_or(false)
    {
        return Some(pyproject);
    }

    None
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

pub struct Pixi {
    global_env_dirs: Vec<PathBuf>,
}

impl Pixi {
    pub fn new() -> Pixi {
        Self::from(&EnvironmentApi::new())
    }

    pub fn from(environment: &dyn Environment) -> Pixi {
        Pixi {
            global_env_dirs: pixi_global_env_dirs(environment),
        }
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

        let home = env.home.clone();

        let extra_executables = find_executables(&prefix);
        let mut symlinks = env.symlinks.clone().unwrap_or_default();
        symlinks.extend(filter_symlink_paths(extra_executables.clone()));
        let mut known_executables = env.known_executables.clone().unwrap_or_default();
        known_executables.extend(extra_executables);

        Some(
            RInstallationBuilder::new(Some(RInstallationKind::Pixi))
                .name(Some(name.clone()))
                .executable(Some(env.executable.clone()))
                .home(home)
                .version(env.version.clone())
                .arch(
                    env.arch
                        .clone()
                        .or_else(|| Some(Architecture::infer_from_path(&env.executable))),
                )
                .known_executables(Some(known_executables))
                .symlinks(Some(symlinks))
                .locator_metadata(Some(LocatorMetadata::Pixi {
                    environment_path: prefix.clone(),
                    manifest_path: infer_manifest_path(&prefix),
                    environment_name: Some(name),
                }))
                .build(),
        )
    }

    fn find(&self, reporter: &dyn Reporter) {
        for envs_dir in &self.global_env_dirs {
            let Ok(entries) = fs::read_dir(envs_dir) else {
                continue;
            };
            for prefix in entries.filter_map(Result::ok).map(|entry| entry.path()) {
                if !is_pixi_env(&prefix) {
                    continue;
                }
                let Some(executable) = find_executable(&prefix) else {
                    continue;
                };
                if let Some(resolved) = ResolvedRInstallation::from(&executable) {
                    let env = resolved.to_r_env();
                    if let Some(installation) = self.try_from(&env) {
                        resolved.add_to_cache(installation.clone());
                        reporter.report_installation(&installation);
                    }
                }
            }
        }
    }
}

fn pixi_global_env_dirs(environment: &dyn Environment) -> Vec<PathBuf> {
    let pixi_home = environment
        .get_env_var("PIXI_HOME".to_string())
        .map(PathBuf::from)
        .or_else(|| environment.get_user_home().map(|home| home.join(".pixi")));
    pixi_home
        .map(|home| vec![home.join("envs")])
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{get_pixi_prefix, infer_manifest_path, is_pixi_env, pixi_global_env_dirs};
    use ret_core::{os_environment::Environment, Locator};
    use std::path::PathBuf;

    struct TestEnvironment {
        pixi_home: Option<String>,
    }

    impl Environment for TestEnvironment {
        fn get_user_home(&self) -> Option<PathBuf> {
            Some(PathBuf::from("/home/tester"))
        }

        fn get_root(&self) -> Option<PathBuf> {
            None
        }

        fn get_env_var(&self, key: String) -> Option<String> {
            (key == "PIXI_HOME")
                .then(|| self.pixi_home.clone())
                .flatten()
        }

        fn get_know_global_search_locations(&self) -> Vec<PathBuf> {
            Vec::new()
        }
    }

    #[test]
    fn global_env_dirs_use_pixi_home_or_user_default() {
        assert_eq!(
            pixi_global_env_dirs(&TestEnvironment {
                pixi_home: Some("/srv/pixi".to_string())
            }),
            vec![PathBuf::from("/srv/pixi/envs")]
        );
        assert_eq!(
            pixi_global_env_dirs(&TestEnvironment { pixi_home: None }),
            vec![PathBuf::from("/home/tester/.pixi/envs")]
        );
    }

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
        // Canonicalize both paths to resolve Windows 8.3 short names
        // (e.g., RUNNER~1 vs runneradmin on GitHub Actions runners).
        let found = found.map(|p| std::fs::canonicalize(&p).unwrap_or(p));
        let expected = std::fs::canonicalize(&prefix).unwrap_or(prefix);
        assert_eq!(found, Some(expected));
    }

    #[test]
    fn pixi_identity_generates_display_name_from_structured_fields() {
        let tmp = tempfile::TempDir::new().expect("failed to create tempdir");
        let prefix = tmp.path().join(".pixi").join("envs").join("analysis");
        let conda_meta = prefix.join("conda-meta");
        let bin = prefix.join("lib").join("R").join("bin");
        std::fs::create_dir_all(&conda_meta).expect("failed to create conda-meta");
        std::fs::create_dir_all(&bin).expect("failed to create bin dir");
        std::fs::write(conda_meta.join("pixi"), "").expect("failed to create pixi marker");

        let executable = bin.join("R");
        std::fs::write(&executable, "").expect("failed to create fake R");
        let env = ret_core::env::REnv::new(
            executable,
            Some(prefix.join("lib").join("R")),
            Some("4.4.2".to_string()),
        );

        let installation = super::Pixi::new()
            .try_from(&env)
            .expect("expected a Pixi R installation");

        assert_eq!(
            installation.display_name.as_deref(),
            Some("R 4.4.2 (Pixi: analysis)")
        );
        assert_eq!(installation.name.as_deref(), Some("analysis"));
        assert_eq!(installation.version.as_deref(), Some("4.4.2"));
    }

    #[test]
    fn non_pixi_path_returns_none() {
        let env = ret_core::env::REnv::new(PathBuf::from("/usr/bin/R"), None, None);
        assert_eq!(get_pixi_prefix(&env), None);
    }

    #[test]
    fn infer_manifest_path_prefers_pixi_toml() {
        let tmp = tempfile::TempDir::new().expect("failed to create tempdir");
        let project = tmp.path().join("project");
        let prefix = project.join(".pixi").join("envs").join("default");
        let conda_meta = prefix.join("conda-meta");
        std::fs::create_dir_all(&conda_meta).expect("failed to create conda-meta");
        std::fs::write(conda_meta.join("pixi"), "").expect("failed to create pixi marker");
        let manifest = project.join("pixi.toml");
        std::fs::create_dir_all(&project).expect("failed to create project dir");
        std::fs::write(&manifest, "[project]\nname='demo'\n").expect("failed to write manifest");

        assert_eq!(infer_manifest_path(&prefix), Some(manifest));
    }

    #[test]
    fn infer_manifest_path_accepts_tool_pixi_pyproject() {
        let tmp = tempfile::TempDir::new().expect("failed to create tempdir");
        let project = tmp.path().join("project");
        let prefix = project.join(".pixi").join("envs").join("default");
        let conda_meta = prefix.join("conda-meta");
        std::fs::create_dir_all(&conda_meta).expect("failed to create conda-meta");
        std::fs::write(conda_meta.join("pixi"), "").expect("failed to create pixi marker");
        let pyproject = project.join("pyproject.toml");
        std::fs::create_dir_all(&project).expect("failed to create project dir");
        std::fs::write(&pyproject, "[tool.pixi]\nchannels=[]\n")
            .expect("failed to write pyproject");

        assert_eq!(infer_manifest_path(&prefix), Some(pyproject));
    }
}
