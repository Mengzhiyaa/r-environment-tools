// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use conda_info::CondaInfo;
use manager::{
    find_conda_binary, find_mamba_binary, get_conda_manager, get_mamba_manager,
    is_mamba_executable, CondaManager,
};
use ret_core::{
    cache::LocatorCache,
    env::REnv,
    manager::EnvManagerType,
    os_environment::Environment,
    r_installation::{RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Configuration, Locator, LocatorKind,
};
use ret_r_utils::{env::ResolvedRInstallation, executable::find_executable};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};
use utils::{get_conda_env_name, get_conda_installation_used_to_create_conda_env, is_conda_env};

mod conda_info;
pub mod manager;
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
}

impl Conda {
    pub fn from(_env: &dyn Environment) -> Conda {
        Conda {
            environments: Arc::new(LocatorCache::new()),
            managers: Arc::new(LocatorCache::new()),
            mamba_managers: Arc::new(LocatorCache::new()),
            conda_executable: Arc::new(RwLock::new(None)),
        }
    }

    fn clear(&self) {
        self.environments.clear();
        self.managers.clear();
        self.mamba_managers.clear();
    }

    fn get_known_manager_executables(&self) -> Vec<PathBuf> {
        let mut executables = vec![];
        if let Some(executable) = self.conda_executable.read().unwrap().clone() {
            executables.push(executable);
        }
        if let Some(executable) = find_conda_binary() {
            executables.push(executable);
        }
        if let Some(executable) = find_mamba_binary() {
            executables.push(executable);
        }
        executables.sort();
        executables.dedup();
        executables
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
            .symlinks(resolved.symlinks.clone())
            .build()
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
        let resolved = ResolvedRInstallation::from(&executable)?;
        let installation = self.build_installation(prefix, resolved.clone(), manager);
        resolved.add_to_cache(installation.clone());
        self.environments
            .insert(prefix.to_path_buf(), installation.clone());
        reporter.report_installation(&installation);

        Some(())
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

    pub fn get_info_for_telemetry(&self, conda_executable: Option<PathBuf>) -> CondaTelemetryInfo {
        let user_provided = conda_executable.is_some();
        let info = CondaInfo::from(conda_executable);
        CondaTelemetryInfo {
            can_spawn_conda: info.is_some(),
            conda_rcs: vec![],
            env_dirs: info
                .as_ref()
                .map(|value| value.all_envs())
                .unwrap_or_default(),
            environments_txt: None,
            environments_txt_exists: None,
            user_provided_env_found: user_provided.then_some(info.is_some()),
            environments_from_txt: vec![],
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
        env.version.as_ref()?;

        let prefix = self.get_prefix_from_env(env)?;

        if let Some(installation) = self.environments.get(&prefix) {
            return Some(installation);
        }

        let manager = self.get_manager_for_prefix(&prefix);
        let resolved = ResolvedRInstallation::from(&env.executable)?;
        let installation = self.build_installation(&prefix, resolved, manager);
        self.environments.insert(prefix, installation.clone());
        Some(installation)
    }

    fn find(&self, reporter: &dyn Reporter) {
        self.clear();

        for executable in self.get_known_manager_executables() {
            let Some(info) = CondaInfo::from(Some(executable.clone())) else {
                continue;
            };

            let manager_type = if is_mamba_executable(&info.executable) {
                EnvManagerType::Mamba
            } else {
                EnvManagerType::Conda
            };
            let Some(manager) = CondaManager::from_info(&info.executable, &info, manager_type)
            else {
                continue;
            };

            self.report_manager_family(reporter, &manager);
            for prefix in info.all_envs() {
                let _ = self.report_prefix(reporter, &prefix, Some(manager.clone()));
            }
        }
    }
}
