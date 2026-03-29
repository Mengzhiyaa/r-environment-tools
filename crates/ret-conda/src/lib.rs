// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use conda_rc::{get_conda_rc_env_dirs, get_conda_rc_search_paths};
use environment_locations::{
    get_conda_dir_from_exe_path, get_conda_environment_paths, get_conda_envs_from_environment_txt,
};
use manager::{get_conda_manager, get_mamba_manager, is_mamba_executable, CondaManager};
use package::RCondaPackageInfo;
use ret_core::{
    cache::LocatorCache,
    env::REnv,
    manager::EnvManagerType,
    os_environment::Environment,
    r_installation::{LocatorMetadata, RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Configuration, Locator, LocatorKind,
};
use ret_fs::path::norm_case;
use ret_r_utils::{
    env::ResolvedRInstallation,
    executable::{filter_symlink_paths, find_executable},
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    thread,
};
use utils::{get_conda_env_name, get_conda_installation_used_to_create_conda_env, is_conda_env};

mod conda_info;
mod conda_rc;
mod environment_locations;
pub mod manager;
mod package;
pub mod utils;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CondaTelemetryInfo {
    pub can_spawn_conda: bool,
    pub conda_rcs: Vec<PathBuf>,
    pub env_dirs: Vec<PathBuf>,
    pub environments_txt: Option<PathBuf>,
    pub environments_txt_exists: Option<bool>,
    pub user_provided_env_found: Option<bool>,
    pub environments_from_txt: Vec<PathBuf>,
    pub executable: Option<PathBuf>,
    pub conda_version: Option<String>,
    pub root_prefix: Option<PathBuf>,
    pub conda_prefix: Option<PathBuf>,
}

pub struct Conda {
    environments: Arc<LocatorCache<PathBuf, RInstallation>>,
    managers: Arc<LocatorCache<PathBuf, CondaManager>>,
    mamba_managers: Arc<LocatorCache<PathBuf, CondaManager>>,
    conda_executable: Arc<RwLock<Option<PathBuf>>>,
    /// User home directory from the `Environment` trait, used for filesystem
    /// discovery without calling `std::env::var` directly.
    home: Option<PathBuf>,
}

impl Conda {
    pub fn from(env: &dyn Environment) -> Conda {
        Conda {
            environments: Arc::new(LocatorCache::new()),
            managers: Arc::new(LocatorCache::new()),
            mamba_managers: Arc::new(LocatorCache::new()),
            conda_executable: Arc::new(RwLock::new(None)),
            home: env.get_user_home(),
        }
    }

    fn clear(&self) {
        self.environments.clear();
        self.managers.clear();
        self.mamba_managers.clear();
    }

    fn cache_manager(&self, manager: CondaManager) {
        if let Some(conda_dir) = &manager.conda_dir {
            if manager.manager_type == EnvManagerType::Mamba {
                self.mamba_managers.insert(conda_dir.clone(), manager);
            } else {
                self.managers.insert(conda_dir.clone(), manager);
            }
        }
    }

    fn get_manager_for_prefix(&self, prefix: &Path) -> Option<CondaManager> {
        let conda_dir = get_conda_installation_used_to_create_conda_env(prefix)?;
        let prefer_mamba = self
            .conda_executable
            .read()
            .unwrap()
            .clone()
            .map(|executable| is_mamba_executable(&executable))
            .unwrap_or(false);

        let conda_manager = self
            .managers
            .get_or_insert_with(conda_dir.clone(), || get_conda_manager(&conda_dir));
        let mamba_manager = self
            .mamba_managers
            .get_or_insert_with(conda_dir.clone(), || get_mamba_manager(&conda_dir));

        if prefer_mamba {
            mamba_manager.or(conda_manager)
        } else {
            conda_manager.or(mamba_manager)
        }
    }

    fn report_manager_family(&self, reporter: &dyn Reporter, manager: &CondaManager) {
        self.cache_manager(manager.clone());
        reporter.report_manager(&manager.to_manager());

        if let Some(conda_dir) = &manager.conda_dir {
            if let Some(conda_manager) = get_conda_manager(conda_dir) {
                self.cache_manager(conda_manager.clone());
                reporter.report_manager(&conda_manager.to_manager());
            }
            if let Some(mamba_manager) = get_mamba_manager(conda_dir) {
                self.cache_manager(mamba_manager.clone());
                reporter.report_manager(&mamba_manager.to_manager());
            }
        }
    }

    fn build_installation(
        &self,
        prefix: &Path,
        resolved: ResolvedRInstallation,
        manager: Option<CondaManager>,
    ) -> RInstallation {
        let conda_dir = manager
            .as_ref()
            .and_then(|manager| manager.conda_dir.clone());
        let name = get_conda_env_name(prefix, &conda_dir);
        let display_name = name
            .as_ref()
            .map(|name| format!("Conda R ({name})"))
            .or(Some("Conda R".to_string()));

        RInstallationBuilder::new(Some(RInstallationKind::Conda))
            .display_name(display_name)
            .name(name)
            .executable(Some(resolved.executable.clone()))
            .home(Some(resolved.home.clone()))
            .version(Some(resolved.version.clone()))
            .arch(Some(resolved.arch.clone()))
            .manager(manager.map(|manager| manager.to_manager()))
            .known_executables(resolved.known_executables.clone())
            .symlinks(resolved.symlinks.clone())
            .startup_command(build_conda_startup_command(prefix))
            .locator_metadata(Some(LocatorMetadata::Conda {
                environment_path: prefix.to_path_buf(),
            }))
            .build()
    }

    /// Fast-path report: extract metadata from conda-meta.
    /// Returns `None` if conda-meta doesn't have R info or home can't be inferred,
    /// signaling the caller to fall back to the slow path.
    fn report_prefix_fast(
        &self,
        reporter: &dyn Reporter,
        prefix: &Path,
        executable: &Path,
        manager: Option<CondaManager>,
    ) -> Option<()> {
        let pkg_info = RCondaPackageInfo::from(prefix)?;

        // Derive R home — returns None if lib/R doesn't exist
        let r_home = Self::infer_conda_r_home(prefix)?;

        let known_executables = collect_r_executables(prefix, &r_home);
        let symlinks = filter_symlink_paths(known_executables.clone());
        let preferred_exe = pick_preferred_executable(executable, &known_executables);

        let conda_dir = manager.as_ref().and_then(|m| m.conda_dir.clone());
        let name = get_conda_env_name(prefix, &conda_dir);
        let display_name = name
            .as_ref()
            .map(|n| format!("Conda R ({n})"))
            .or(Some("Conda R".to_string()));

        let installation = RInstallationBuilder::new(Some(RInstallationKind::Conda))
            .display_name(display_name)
            .name(name)
            .executable(Some(preferred_exe))
            .home(Some(r_home))
            .version(Some(pkg_info.version))
            .arch(pkg_info.arch)
            .manager(manager.map(|m| m.to_manager()))
            .known_executables(Some(known_executables))
            .symlinks(Some(symlinks))
            .startup_command(build_conda_startup_command(prefix))
            .locator_metadata(Some(LocatorMetadata::Conda {
                environment_path: prefix.to_path_buf(),
            }))
            .build();

        self.environments
            .insert(prefix.to_path_buf(), installation.clone());
        reporter.report_installation(&installation);
        Some(())
    }

    /// Slow-path report: spawn R to resolve metadata.
    fn report_prefix_slow(
        &self,
        reporter: &dyn Reporter,
        prefix: &Path,
        executable: &Path,
        manager: Option<CondaManager>,
    ) -> Option<()> {
        let resolved = ResolvedRInstallation::from(executable)?;
        let installation = self.build_installation(prefix, resolved.clone(), manager);
        resolved.add_to_cache(installation.clone());
        self.environments
            .insert(prefix.to_path_buf(), installation.clone());
        reporter.report_installation(&installation);
        Some(())
    }

    fn report_prefix(
        &self,
        reporter: &dyn Reporter,
        prefix: &Path,
        manager: Option<CondaManager>,
    ) -> Option<()> {
        if !is_conda_env(prefix) || self.environments.contains_key(&prefix.to_path_buf()) {
            return None;
        }

        let executable = find_executable(prefix)?;

        // Try fast path first (conda-meta), fall back to slow path (spawn R)
        if self
            .report_prefix_fast(reporter, prefix, &executable, manager.clone())
            .is_some()
        {
            return Some(());
        }

        self.report_prefix_slow(reporter, prefix, &executable, manager)
    }

    fn get_prefix_from_env(&self, env: &REnv) -> Option<PathBuf> {
        let mut search_roots = vec![env.executable.clone()];
        if let Some(home) = &env.home {
            search_roots.push(home.clone());
        }

        for root in search_roots {
            for ancestor in root.ancestors() {
                if is_conda_env(ancestor) {
                    return Some(ancestor.to_path_buf());
                }
            }
        }

        None
    }

    /// Derive R.home() equivalent from a conda prefix.
    ///
    /// Conda R installs to `<prefix>/lib/R` on both Unix and Windows.
    /// Returns `None` if the directory doesn't exist, signaling that
    /// we can't infer home without spawning R.
    fn infer_conda_r_home(prefix: &Path) -> Option<PathBuf> {
        let lib_r = prefix.join("lib").join("R");
        if lib_r.is_dir() {
            return Some(norm_case(lib_r));
        }
        // Cannot determine R home without spawning R
        None
    }

    pub fn get_info_for_telemetry(&self, conda_executable: Option<PathBuf>) -> CondaTelemetryInfo {
        let user_provided = conda_executable.is_some();
        let info = conda_info::CondaInfo::from(conda_executable.clone());
        let environment = EnvironmentAdapter {
            home: self.home.clone(),
        };
        let configured_executable = self
            .conda_executable
            .read()
            .unwrap()
            .clone()
            .or(conda_executable.clone());
        let envs_found = get_conda_environment_paths(&environment, &configured_executable);
        let conda_rcs = get_conda_rc_search_paths(&environment)
            .into_iter()
            .filter(|path| path.exists())
            .collect::<Vec<_>>();
        let env_dirs = get_conda_rc_env_dirs(&environment);
        let environments_txt = self
            .home
            .as_ref()
            .map(|home| home.join(".conda").join("environments.txt"));
        let environments_txt_exists = environments_txt.as_ref().map(|path| path.exists());
        let environments_from_txt = get_conda_envs_from_environment_txt(&environment);
        let user_provided_env_found = if user_provided {
            conda_executable
                .as_ref()
                .and_then(|exe| get_conda_dir_from_exe_path(exe))
                .map(|dir| envs_found.contains(&dir))
        } else {
            None
        };

        CondaTelemetryInfo {
            can_spawn_conda: info.is_some(),
            conda_rcs,
            env_dirs,
            environments_txt,
            environments_txt_exists,
            user_provided_env_found,
            environments_from_txt,
            executable: info.as_ref().map(|value| value.executable.clone()),
            conda_version: info
                .as_ref()
                .map(|value| value.conda_version.clone())
                .filter(|value| !value.is_empty()),
            root_prefix: info.as_ref().and_then(|value| value.root_prefix.clone()),
            conda_prefix: info.as_ref().and_then(|value| value.conda_prefix.clone()),
        }
    }
}

impl Locator for Conda {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::Conda
    }

    fn configure(&self, config: &Configuration) {
        *self.conda_executable.write().unwrap() = config.conda_executable.clone();
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::Conda]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        let prefix = self.get_prefix_from_env(env)?;

        // Check cache FIRST, before requiring version.
        // This ensures PATH-based discovery (version: None) hits the cache
        // for environments already discovered by find().
        if let Some(installation) = self.environments.get(&prefix) {
            return Some(installation);
        }

        // Try fast path (conda-meta) — resolves metadata without spawning R.
        // This is important for the find API where try_from is called with
        // a bare executable and no version hint.
        if let Some(pkg_info) = RCondaPackageInfo::from(&prefix) {
            if let Some(r_home) = Self::infer_conda_r_home(&prefix) {
                let manager = self.get_manager_for_prefix(&prefix);
                let known_executables = collect_r_executables(&prefix, &r_home);
                let symlinks = filter_symlink_paths(known_executables.clone());
                let preferred_exe = pick_preferred_executable(&env.executable, &known_executables);
                let conda_dir = manager.as_ref().and_then(|m| m.conda_dir.clone());
                let name = get_conda_env_name(&prefix, &conda_dir);
                let display_name = name
                    .as_ref()
                    .map(|n| format!("Conda R ({n})"))
                    .or(Some("Conda R".to_string()));

                let installation = RInstallationBuilder::new(Some(RInstallationKind::Conda))
                    .display_name(display_name)
                    .name(name)
                    .executable(Some(preferred_exe))
                    .home(Some(r_home))
                    .version(Some(pkg_info.version))
                    .arch(pkg_info.arch)
                    .manager(manager.map(|m| m.to_manager()))
                    .known_executables(Some(known_executables))
                    .symlinks(Some(symlinks))
                    .startup_command(build_conda_startup_command(&prefix))
                    .locator_metadata(Some(LocatorMetadata::Conda {
                        environment_path: prefix.clone(),
                    }))
                    .build();

                self.environments.insert(prefix, installation.clone());
                return Some(installation);
            }
        }

        // Fall back to slow path (spawn R) — only if we have a version hint
        env.version.as_ref()?;

        let manager = self.get_manager_for_prefix(&prefix);
        let resolved = ResolvedRInstallation::from(&env.executable)?;
        let installation = self.build_installation(&prefix, resolved, manager);
        self.environments.insert(prefix, installation.clone());
        Some(installation)
    }

    fn find(&self, reporter: &dyn Reporter) {
        self.clear();

        let executable = self.conda_executable.read().unwrap().clone();

        // Phase 1: Filesystem-only environment discovery (no process spawn)
        let prefixes = get_conda_environment_paths(
            &EnvironmentAdapter {
                home: self.home.clone(),
            },
            &executable,
        );

        // Phase 2: Parallel metadata extraction
        thread::scope(|s| {
            for prefix in &prefixes {
                let prefix = prefix.clone();
                s.spawn(move || {
                    let manager = self.get_manager_for_prefix(&prefix);
                    if let Some(ref manager) = manager {
                        self.report_manager_family(reporter, manager);
                    }
                    let _ = self.report_prefix(reporter, &prefix, manager);
                });
            }
        });
    }
}

/// Minimal adapter to pass stored `home` through the `Environment` trait
/// for filesystem discovery, without requiring the full `EnvironmentApi`.
struct EnvironmentAdapter {
    home: Option<PathBuf>,
}

impl Environment for EnvironmentAdapter {
    fn get_user_home(&self) -> Option<PathBuf> {
        self.home.clone()
    }

    fn get_root(&self) -> Option<PathBuf> {
        None
    }

    fn get_env_var(&self, key: String) -> Option<String> {
        std::env::var(key).ok()
    }

    fn get_know_global_search_locations(&self) -> Vec<PathBuf> {
        vec![]
    }
}

fn build_conda_startup_command(prefix: &Path) -> Option<String> {
    if cfg!(windows) {
        None
    } else {
        Some(format!("conda activate {}", prefix.display()))
    }
}

/// Collect all known R executables for a conda environment.
/// Handles both Unix and Windows layouts.
fn collect_r_executables(prefix: &Path, r_home: &Path) -> Vec<PathBuf> {
    let mut exes = vec![];

    if cfg!(windows) {
        // Windows candidates matching find_executable in ret-r-utils
        for candidate in [
            prefix.join("Scripts").join("R.exe"),
            prefix.join("Scripts").join("Rscript.exe"),
            prefix.join("Library").join("bin").join("R.exe"),
            prefix.join("Library").join("bin").join("Rscript.exe"),
            prefix.join("bin").join("x64").join("R.exe"),
            prefix.join("bin").join("R.exe"),
            prefix.join("bin").join("Rscript.exe"),
        ] {
            if candidate.exists() {
                exes.push(norm_case(candidate));
            }
        }
    } else {
        // Unix candidates
        for name in ["R", "Rscript"] {
            let p = prefix.join("bin").join(name);
            if p.exists() {
                exes.push(norm_case(p));
            }
        }
    }

    // Also check <r_home>/bin if different from <prefix>
    let r_home_norm = norm_case(r_home.to_path_buf());
    let prefix_norm = norm_case(prefix.to_path_buf());
    if r_home_norm != prefix_norm {
        if cfg!(windows) {
            for name in ["R.exe", "Rscript.exe"] {
                let p = r_home.join("bin").join(name);
                if p.exists() {
                    exes.push(norm_case(p));
                }
            }
        } else {
            for name in ["R", "Rscript"] {
                let p = r_home.join("bin").join(name);
                if p.exists() {
                    exes.push(norm_case(p));
                }
            }
        }
    }

    exes.sort();
    exes.dedup();
    exes
}

/// Pick the preferred executable from the collected set.
/// Prefers `<prefix>/bin/R` (or `R.exe` on Windows).
fn pick_preferred_executable(found: &Path, known_executables: &[PathBuf]) -> PathBuf {
    let target = if cfg!(windows) { "R.exe" } else { "R" };
    known_executables
        .iter()
        .find(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().eq_ignore_ascii_case(target))
                .unwrap_or(false)
                && p.parent().map(|par| par.ends_with("bin")).unwrap_or(false)
        })
        .cloned()
        .unwrap_or_else(|| norm_case(found.to_path_buf()))
}
