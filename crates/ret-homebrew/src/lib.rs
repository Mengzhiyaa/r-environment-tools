// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ret_core::{
    arch::Architecture,
    cache::LocatorCache,
    env::REnv,
    manager::{EnvManager, EnvManagerType},
    os_environment::Environment,
    r_installation::{RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Configuration, Locator, LocatorKind, RefreshStatePersistence,
};
use ret_fs::path::resolve_symlink;
use ret_r_utils::{
    env::ResolvedRInstallation,
    executable::{filter_symlink_paths, find_executables},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct Homebrew {
    reported_executables: Arc<LocatorCache<PathBuf, RInstallation>>,
    manager: Option<EnvManager>,
    cellars: Vec<PathBuf>,
}

impl Homebrew {
    pub fn from(environment: &dyn Environment) -> Homebrew {
        Homebrew {
            reported_executables: Arc::new(LocatorCache::new()),
            manager: find_brew_manager(environment),
            cellars: configured_cellar_roots(environment),
        }
    }

    fn insert_and_report(&self, installation: RInstallation, reporter: Option<&dyn Reporter>) {
        let mut entries = installation
            .known_executables
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|executable| (executable, installation.clone()))
            .collect::<Vec<_>>();
        if entries.is_empty() {
            if let Some(executable) = installation.executable.clone() {
                entries.push((executable, installation.clone()));
            }
        }
        self.reported_executables.insert_many(entries);

        if let Some(reporter) = reporter {
            if let Some(manager) = &installation.manager {
                reporter.report_manager(manager);
            }
            reporter.report_installation(&installation);
        }
    }
}

impl Locator for Homebrew {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::Homebrew
    }

    fn refresh_state(&self) -> RefreshStatePersistence {
        RefreshStatePersistence::SelfHydratingCache
    }

    fn configure(&self, _config: &Configuration) {}

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::Homebrew]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        if !cfg!(unix) {
            return None;
        }

        let executable = resolve_symlink(&env.executable).unwrap_or(env.executable.clone());
        let home = env.home.clone()?;
        if !looks_like_homebrew_path(&executable)
            && !looks_like_homebrew_path(&home)
            && !self.cellars.iter().any(|root| {
                ret_r_utils::executable::is_path_within(&home, root)
                    || ret_r_utils::executable::is_path_within(&executable, root)
            })
        {
            return None;
        }

        let extra_executables = if env.known_executables.is_none() {
            let mut executables = find_executables(home.join("bin"));
            executables.push(env.executable.clone());
            executables
        } else {
            Vec::new()
        };
        let mut symlinks = env.symlinks.clone().unwrap_or_default();
        symlinks.extend(filter_symlink_paths(extra_executables.clone()));
        let mut known_executables = env.known_executables.clone().unwrap_or_default();
        known_executables.extend(extra_executables);

        Some(
            RInstallationBuilder::new(Some(RInstallationKind::Homebrew))
                .display_name(Some("Homebrew R".to_string()))
                .executable(Some(env.executable.clone()))
                .home(Some(home.clone()))
                .version(env.version.clone())
                .arch(
                    env.arch
                        .clone()
                        .or_else(|| Some(Architecture::infer_from_path(&env.executable))),
                )
                .manager(
                    home.ancestors()
                        .find_map(|prefix| {
                            let brew = prefix.join("bin").join("brew");
                            brew.is_file()
                                .then(|| EnvManager::new(brew, EnvManagerType::Homebrew, None))
                        })
                        .or_else(|| self.manager.clone()),
                )
                .known_executables(Some(known_executables))
                .symlinks(Some(symlinks))
                .build(),
        )
    }

    fn find(&self, reporter: &dyn Reporter) {
        if !cfg!(unix) {
            return;
        }

        self.reported_executables.clear();
        for cellar_root in &self.cellars {
            let Ok(formulas) = fs::read_dir(cellar_root) else {
                continue;
            };
            for formula in formulas.filter_map(Result::ok) {
                let name = formula.file_name();
                let name = name.to_string_lossy();
                if name != "r" && !name.starts_with("r@") {
                    continue;
                }

                let Ok(versions) = fs::read_dir(formula.path()) else {
                    continue;
                };
                for version_dir in versions.filter_map(Result::ok) {
                    for executable in find_executables(version_dir.path()) {
                        if let Some(resolved) = ResolvedRInstallation::from(&executable) {
                            if let Some(installation) = self.try_from(&resolved.to_r_env()) {
                                resolved.add_to_cache(installation.clone());
                                self.insert_and_report(installation, Some(reporter));
                            }
                        }
                    }
                }
            }
        }
    }
}

fn cellar_roots() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/opt/homebrew/Cellar"),
        PathBuf::from("/usr/local/Cellar"),
        PathBuf::from("/home/linuxbrew/.linuxbrew/Cellar"),
    ]
}

fn looks_like_homebrew_path(path: &Path) -> bool {
    ret_core::homebrew_utils::looks_like_homebrew_path(path)
}

fn configured_cellar_roots(environment: &dyn Environment) -> Vec<PathBuf> {
    let mut roots = cellar_roots();
    if let Some(cellar) = environment.get_env_var("HOMEBREW_CELLAR".to_string()) {
        roots.push(PathBuf::from(cellar));
    }
    if let Some(prefix) = environment.get_env_var("HOMEBREW_PREFIX".to_string()) {
        roots.push(PathBuf::from(prefix).join("Cellar"));
    }
    if let Some(path) = environment.get_env_var("PATH".to_string()) {
        for bin in std::env::split_paths(&path) {
            if bin.join("brew").is_file() {
                if let Some(prefix) = bin.parent() {
                    roots.push(prefix.join("Cellar"));
                }
            }
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

fn find_brew_manager(environment: &dyn Environment) -> Option<EnvManager> {
    let mut candidates = Vec::new();
    if let Some(prefix) = environment.get_env_var("HOMEBREW_PREFIX".to_string()) {
        candidates.push(PathBuf::from(prefix).join("bin").join("brew"));
    }
    if let Some(path) = environment.get_env_var("PATH".to_string()) {
        candidates.extend(std::env::split_paths(&path).map(|bin| bin.join("brew")));
    }
    candidates.extend(
        cellar_roots()
            .iter()
            .filter_map(|cellar| cellar.parent())
            .map(|prefix| prefix.join("bin").join("brew")),
    );
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| EnvManager::new(path, EnvManagerType::Homebrew, None))
}

#[cfg(test)]
mod tests {
    use super::{cellar_roots, looks_like_homebrew_path};
    #[cfg(unix)]
    use ret_core::{
        env::REnv, os_environment::EnvironmentApi, r_installation::RInstallationKind, Locator,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn recognizes_cellar_paths() {
        assert!(looks_like_homebrew_path(Path::new(
            "/opt/homebrew/Cellar/r/4.4.1/lib/R"
        )));
        assert!(looks_like_homebrew_path(Path::new(
            "/usr/local/Cellar/r@4.3/4.3.3/lib/R"
        )));
        assert!(looks_like_homebrew_path(Path::new("/opt/homebrew/bin/R")));
    }

    #[test]
    fn recognizes_linuxbrew_paths() {
        assert!(looks_like_homebrew_path(Path::new(
            "/home/linuxbrew/.linuxbrew/Cellar/r/4.4.1/lib/R/bin/R"
        )));
        assert!(looks_like_homebrew_path(Path::new(
            "/home/linuxbrew/.linuxbrew/bin/R"
        )));
    }

    #[test]
    fn rejects_non_homebrew_paths() {
        assert!(!looks_like_homebrew_path(Path::new("/usr/bin/R")));
        assert!(!looks_like_homebrew_path(Path::new("/usr/local/bin/R")));
        assert!(!looks_like_homebrew_path(Path::new("/opt/R/4.4.1/bin/R")));
        assert!(!looks_like_homebrew_path(Path::new(
            "/nix/store/hash-R/bin/R"
        )));
    }

    #[test]
    fn cellar_roots_contains_expected_paths() {
        let roots = cellar_roots();
        assert!(roots.contains(&PathBuf::from("/opt/homebrew/Cellar")));
        assert!(roots.contains(&PathBuf::from("/usr/local/Cellar")));
        assert!(roots.contains(&PathBuf::from("/home/linuxbrew/.linuxbrew/Cellar")));
    }

    #[cfg(unix)]
    #[test]
    fn try_from_classifies_homebrew_installation() {
        let locator = super::Homebrew::from(&EnvironmentApi::new());
        let env = REnv::new(
            PathBuf::from("/opt/homebrew/Cellar/r/4.4.1/lib/R/bin/R"),
            Some(PathBuf::from("/opt/homebrew/Cellar/r/4.4.1/lib/R")),
            Some("4.4.1".to_string()),
        );

        let installation = locator
            .try_from(&env)
            .expect("expected homebrew installation");
        assert_eq!(installation.kind, Some(RInstallationKind::Homebrew));
        assert_eq!(installation.version.as_deref(), Some("4.4.1"));
    }

    #[cfg(unix)]
    #[test]
    fn try_from_rejects_non_homebrew_path() {
        let locator = super::Homebrew::from(&EnvironmentApi::new());
        let env = REnv::new(
            PathBuf::from("/usr/bin/R"),
            Some(PathBuf::from("/usr/lib/R")),
            Some("4.4.1".to_string()),
        );

        assert!(locator.try_from(&env).is_none());
    }
}

#[cfg(test)]
mod custom_root_tests {
    use super::*;
    use std::collections::HashMap;
    struct Env(HashMap<String, String>);
    impl Environment for Env {
        fn get_user_home(&self) -> Option<PathBuf> {
            None
        }
        fn get_root(&self) -> Option<PathBuf> {
            None
        }
        fn get_env_var(&self, key: String) -> Option<String> {
            self.0.get(&key).cloned()
        }
        fn get_know_global_search_locations(&self) -> Vec<PathBuf> {
            Vec::new()
        }
    }

    #[test]
    fn custom_cellar_prefix_and_path_brew_are_discovery_roots() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("custom brew");
        let cellar = temp.path().join("packages");
        let bin = prefix.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("brew"), "brew").unwrap();
        let env = Env(HashMap::from([
            (
                "HOMEBREW_PREFIX".into(),
                prefix.to_string_lossy().into_owned(),
            ),
            (
                "HOMEBREW_CELLAR".into(),
                cellar.to_string_lossy().into_owned(),
            ),
            (
                "PATH".into(),
                std::env::join_paths([&bin])
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ),
        ]));
        let roots = configured_cellar_roots(&env);
        assert!(roots.contains(&cellar));
        assert!(roots.contains(&prefix.join("Cellar")));
        assert_eq!(
            find_brew_manager(&env).unwrap().executable,
            bin.join("brew")
        );
        if cfg!(unix) {
            let locator = Homebrew::from(&env);
            let home = cellar.join("r/4.4/lib/R");
            let raw = REnv::new(home.join("bin/R"), Some(home), Some("4.4.0".into()));
            assert_eq!(
                locator.try_from(&raw).unwrap().kind,
                Some(RInstallationKind::Homebrew)
            );
        }
    }
}
