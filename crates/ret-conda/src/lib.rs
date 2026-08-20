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
    Configuration, Locator, LocatorKind, RefreshStatePersistence, RefreshStateSyncScope,
};
use ret_fs::path::norm_case;
use ret_r_utils::{
    env::ResolvedRInstallation,
    executable::{filter_symlink_paths, find_executable},
};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    thread,
    time::SystemTime,
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
    package_info_cache: PackageInfoCache,
    /// User home directory from the `Environment` trait, used for filesystem
    /// discovery without calling `std::env::var` directly.
    home: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileFingerprint {
    modified: SystemTime,
    len: u64,
}

impl FileFingerprint {
    fn from_path(path: &Path) -> Option<Self> {
        let metadata = fs::metadata(path).ok()?;
        Some(Self {
            modified: metadata.modified().ok()?,
            len: metadata.len(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CondaEnvironmentFingerprint {
    conda_meta: FileFingerprint,
    history: Option<FileFingerprint>,
    r_base_packages: Vec<(PathBuf, FileFingerprint)>,
}

impl CondaEnvironmentFingerprint {
    fn from_prefix(prefix: &Path) -> Option<Self> {
        let conda_meta = prefix.join("conda-meta");
        let history_path = conda_meta.join("history");
        let history = if history_path.exists() {
            Some(FileFingerprint::from_path(&history_path)?)
        } else {
            None
        };
        let mut r_base_packages = fs::read_dir(&conda_meta)
            .ok()?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_name()?.to_string_lossy();
                (name.starts_with("r-base-") && name.ends_with(".json"))
                    .then(|| FileFingerprint::from_path(&path).map(|value| (path, value)))?
            })
            .collect::<Vec<_>>();
        r_base_packages.sort_by(|left, right| left.0.cmp(&right.0));

        Some(Self {
            conda_meta: FileFingerprint::from_path(&conda_meta)?,
            history,
            r_base_packages,
        })
    }
}

#[derive(Clone)]
struct CachedPackageInfo {
    fingerprint: CondaEnvironmentFingerprint,
    info: RCondaPackageInfo,
}

type PackageInfoCache = Arc<RwLock<HashMap<PathBuf, CachedPackageInfo>>>;

impl Conda {
    pub fn from(env: &dyn Environment) -> Conda {
        Self::with_package_info_cache(env, Arc::new(RwLock::new(HashMap::new())))
    }

    /// Creates a request-local locator that shares only fingerprint-validated
    /// metadata with the long-lived locator.
    pub fn from_shared_environment_cache(env: &dyn Environment, source: &Conda) -> Conda {
        Self::with_package_info_cache(env, source.package_info_cache.clone())
    }

    fn with_package_info_cache(
        env: &dyn Environment,
        package_info_cache: PackageInfoCache,
    ) -> Conda {
        Conda {
            environments: Arc::new(LocatorCache::new()),
            managers: Arc::new(LocatorCache::new()),
            mamba_managers: Arc::new(LocatorCache::new()),
            conda_executable: Arc::new(RwLock::new(None)),
            package_info_cache,
            home: env.get_user_home(),
        }
    }

    fn get_package_info(&self, prefix: &Path) -> Option<RCondaPackageInfo> {
        let cache_key = norm_case(prefix);
        let fingerprint_before = CondaEnvironmentFingerprint::from_prefix(prefix);

        // Keep the write lock through the metadata read. This makes concurrent
        // requests for the same prefix perform the expensive parse only once.
        let mut cache = self
            .package_info_cache
            .write()
            .expect("conda package info cache lock poisoned");
        if let Some(fingerprint) = &fingerprint_before {
            if let Some(cached) = cache
                .get(&cache_key)
                .filter(|cached| &cached.fingerprint == fingerprint)
            {
                return Some(cached.info.clone());
            }
        }

        let Some(info) = RCondaPackageInfo::from(prefix) else {
            cache.remove(&cache_key);
            return None;
        };
        let fingerprint_after = CondaEnvironmentFingerprint::from_prefix(prefix);
        if fingerprint_before.is_some() && fingerprint_before == fingerprint_after {
            cache.insert(
                cache_key,
                CachedPackageInfo {
                    fingerprint: fingerprint_after.expect("fingerprint checked as present"),
                    info: info.clone(),
                },
            );
        } else {
            cache.remove(&cache_key);
        }
        Some(info)
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

        RInstallationBuilder::new(Some(RInstallationKind::Conda))
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
        let pkg_info = self.get_package_info(prefix)?;

        // Derive R home — returns None if lib/R doesn't exist
        let r_home = Self::infer_conda_r_home(prefix)?;

        let known_executables = collect_r_executables(prefix, &r_home);
        let symlinks = filter_symlink_paths(known_executables.clone());
        let preferred_exe = pick_preferred_executable(executable, &known_executables);

        let conda_dir = manager.as_ref().and_then(|m| m.conda_dir.clone());
        let name = get_conda_env_name(prefix, &conda_dir);

        let installation = RInstallationBuilder::new(Some(RInstallationKind::Conda))
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

    fn refresh_state(&self) -> RefreshStatePersistence {
        RefreshStatePersistence::SyncedDiscoveryState
    }

    fn sync_refresh_state_from(&self, source: &dyn Locator, scope: &RefreshStateSyncScope) {
        let source = source.as_any().downcast_ref::<Conda>().unwrap_or_else(|| {
            panic!("attempted to sync Conda state from {:?}", source.get_kind())
        });

        match scope {
            RefreshStateSyncScope::Full => {
                self.environments.clear();
                self.environments
                    .insert_many(source.environments.clone_map());
                self.managers.clear();
                self.managers.insert_many(source.managers.clone_map());
                self.mamba_managers.clear();
                self.mamba_managers
                    .insert_many(source.mamba_managers.clone_map());
            }
            RefreshStateSyncScope::GlobalFiltered(kind)
                if self.supported_categories().contains(kind) =>
            {
                self.environments
                    .insert_many(source.environments.clone_map());
                self.managers.insert_many(source.managers.clone_map());
                self.mamba_managers
                    .insert_many(source.mamba_managers.clone_map());
            }
            RefreshStateSyncScope::GlobalFiltered(_) | RefreshStateSyncScope::Workspace => {}
        }
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
        if let Some(pkg_info) = self.get_package_info(&prefix) {
            if let Some(r_home) = Self::infer_conda_r_home(&prefix) {
                let manager = self.get_manager_for_prefix(&prefix);
                let known_executables = collect_r_executables(&prefix, &r_home);
                let symlinks = filter_symlink_paths(known_executables.clone());
                let preferred_exe = pick_preferred_executable(&env.executable, &known_executables);
                let conda_dir = manager.as_ref().and_then(|m| m.conda_dir.clone());
                let name = get_conda_env_name(&prefix, &conda_dir);

                let installation = RInstallationBuilder::new(Some(RInstallationKind::Conda))
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
        self.package_info_cache
            .write()
            .expect("conda package info cache lock poisoned")
            .retain(|prefix, _| prefix.join("conda-meta").is_dir());

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
    let r_home_norm = norm_case(r_home);
    let prefix_norm = norm_case(prefix);
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
        .unwrap_or_else(|| norm_case(found))
}

#[cfg(test)]
mod tests {
    use super::Conda;
    use ret_core::{
        arch::Architecture,
        env::REnv,
        manager::EnvManager,
        os_environment::EnvironmentApi,
        r_installation::{RInstallation, RInstallationKind},
        reporter::Reporter,
        Locator, RefreshStateSyncScope,
    };
    use ret_r_utils::env::ResolvedRInstallation;
    use std::{path::PathBuf, sync::Mutex};

    #[derive(Default)]
    struct TestReporter {
        installations: Mutex<Vec<RInstallation>>,
    }

    impl Reporter for TestReporter {
        fn report_manager(&self, _manager: &EnvManager) {}

        fn report_installation(&self, installation: &RInstallation) {
            self.installations
                .lock()
                .expect("installations mutex poisoned")
                .push(installation.clone());
        }
    }

    fn create_conda_r_prefix() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp_dir = tempfile::TempDir::new().expect("failed to create tempdir");
        let prefix = temp_dir.path().join("envs").join("seurat4");
        let conda_meta = prefix.join("conda-meta");
        let executable = prefix.join("bin").join("R");
        std::fs::create_dir_all(&conda_meta).expect("failed to create conda-meta");
        std::fs::create_dir_all(prefix.join("lib").join("R")).expect("failed to create R home");
        std::fs::create_dir_all(
            executable
                .parent()
                .expect("R executable must have a parent"),
        )
        .expect("failed to create bin directory");
        std::fs::write(&executable, "").expect("failed to create fake R executable");
        std::fs::write(
            conda_meta.join("history"),
            "+https://conda.example/linux-64::r-base-4.3.3-h123_0\n",
        )
        .expect("failed to write conda history");
        std::fs::write(
            conda_meta.join("r-base-4.3.3-h123_0.json"),
            r#"{"version":"4.3.3","subdir":"linux-64","arch":"x86_64"}"#,
        )
        .expect("failed to write r-base metadata");

        (temp_dir, prefix, executable)
    }

    #[test]
    fn conda_identity_uses_structured_fields_instead_of_a_synthetic_display_name() {
        let prefix = std::path::PathBuf::from("/opt/conda/envs/seurat4");
        let resolved = ResolvedRInstallation {
            executable: prefix.join("bin/R"),
            home: prefix.join("lib/R"),
            version: "4.3.3".to_string(),
            arch: Architecture::X64,
            known_executables: None,
            symlinks: None,
        };

        let installation =
            Conda::from(&EnvironmentApi::new()).build_installation(&prefix, resolved, None);

        assert_eq!(installation.display_name, None);
        assert_eq!(installation.name.as_deref(), Some("seurat4"));
        assert_eq!(installation.version.as_deref(), Some("4.3.3"));
        assert_eq!(installation.kind, Some(RInstallationKind::Conda));
    }

    #[test]
    fn fast_conda_paths_preserve_structured_identity_without_a_display_name() {
        let (_temp_dir, prefix, executable) = create_conda_r_prefix();
        let environment = EnvironmentApi::new();

        let identified = Conda::from(&environment)
            .try_from(&REnv::new(executable.clone(), None, None))
            .expect("expected try_from to identify the Conda R installation");
        assert_eq!(identified.display_name, None);
        assert_eq!(identified.name.as_deref(), Some("seurat4"));
        assert_eq!(identified.version.as_deref(), Some("4.3.3"));

        let reporter = TestReporter::default();
        assert!(Conda::from(&environment)
            .report_prefix_fast(&reporter, &prefix, &executable, None)
            .is_some());
        let reported = reporter
            .installations
            .lock()
            .expect("installations mutex poisoned");
        assert_eq!(reported.len(), 1);
        assert_eq!(reported[0].display_name, None);
        assert_eq!(reported[0].name.as_deref(), Some("seurat4"));
        assert_eq!(reported[0].version.as_deref(), Some("4.3.3"));
    }

    #[test]
    fn shared_package_cache_reuses_and_invalidates_fingerprinted_metadata() {
        let (_temp_dir, prefix, _) = create_conda_r_prefix();
        let environment = EnvironmentApi::new();
        let shared = Conda::from(&environment);
        assert_eq!(
            shared
                .get_package_info(&prefix)
                .expect("expected initial package info")
                .version,
            "4.3.3"
        );

        let refresh = Conda::from_shared_environment_cache(&environment, &shared);
        assert!(std::sync::Arc::ptr_eq(
            &refresh.package_info_cache,
            &shared.package_info_cache
        ));
        assert_eq!(
            refresh
                .get_package_info(&prefix)
                .expect("expected cached package info")
                .version,
            "4.3.3"
        );

        let conda_meta = prefix.join("conda-meta");
        std::fs::remove_file(conda_meta.join("r-base-4.3.3-h123_0.json"))
            .expect("failed to remove old metadata");
        std::fs::write(
            conda_meta.join("history"),
            "+https://conda.example/linux-64::r-base-4.4.2-h987654_0\n",
        )
        .expect("failed to update history");
        std::fs::write(
            conda_meta.join("r-base-4.4.2-h987654_0.json"),
            r#"{"version":"4.4.2","subdir":"linux-64","arch":"x86_64"}"#,
        )
        .expect("failed to update package metadata");
        assert_eq!(
            refresh
                .get_package_info(&prefix)
                .expect("expected updated package info")
                .version,
            "4.4.2"
        );

        std::fs::remove_dir_all(&prefix).expect("failed to remove environment");
        assert!(refresh.get_package_info(&prefix).is_none());
        assert!(!refresh
            .package_info_cache
            .read()
            .unwrap()
            .contains_key(&super::norm_case(&prefix)));
    }

    #[test]
    fn conda_refresh_state_respects_sync_scope() {
        let environment = EnvironmentApi::new();
        let target = Conda::from(&environment);
        let source = Conda::from_shared_environment_cache(&environment, &target);
        let prefix = PathBuf::from("/tmp/ret-conda-state/env");
        let installation = RInstallation::default();
        source.environments.insert(prefix.clone(), installation);

        target.sync_refresh_state_from(&source, &RefreshStateSyncScope::Workspace);
        assert!(target.environments.is_empty());

        target.sync_refresh_state_from(&source, &RefreshStateSyncScope::Full);
        assert!(target.environments.contains_key(&prefix));
    }
}
