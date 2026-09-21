// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ret_core::{
    arch::Architecture,
    env::REnv,
    manager::{EnvManager, EnvManagerType},
    os_environment::{Environment, EnvironmentApi},
    r_installation::{RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Configuration, Locator, LocatorKind, RefreshStatePersistence,
};
use ret_r_utils::{env::ResolvedRInstallation, executable::find_executables};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, RwLock},
};

pub struct Rig {
    rig_executable: Arc<RwLock<Option<PathBuf>>>,
    install_roots: Vec<PathBuf>,
    classification_roots: Vec<PathBuf>,
    configured_manager: RwLock<bool>,
}

impl Rig {
    pub fn new() -> Rig {
        Self::from(&EnvironmentApi::new())
    }

    pub fn from(environment: &dyn Environment) -> Rig {
        let (install_roots, classification_roots) = rig_roots(environment);
        Rig {
            rig_executable: Arc::new(RwLock::new(find_rig_executable())),
            install_roots,
            classification_roots,
            configured_manager: RwLock::new(false),
        }
    }

    fn installation_from_env(&self, env: &REnv) -> Option<RInstallation> {
        let home = env.home.clone()?;
        Some(
            RInstallationBuilder::new(Some(RInstallationKind::Rig))
                .display_name(Some("rig managed R".to_string()))
                .executable(Some(env.executable.clone()))
                .home(Some(home))
                .version(env.version.clone())
                .arch(
                    env.arch
                        .clone()
                        .or_else(|| Some(Architecture::infer_from_path(&env.executable))),
                )
                .manager(
                    self.rig_executable
                        .read()
                        .unwrap()
                        .clone()
                        .map(|path| EnvManager::new(path, EnvManagerType::Rig, None)),
                )
                .known_executables(env.known_executables.clone())
                .symlinks(env.symlinks.clone())
                .build(),
        )
    }
}

impl Default for Rig {
    fn default() -> Self {
        Self::new()
    }
}

impl Locator for Rig {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::Rig
    }

    fn refresh_state(&self) -> RefreshStatePersistence {
        RefreshStatePersistence::ConfiguredOnly
    }

    fn configure(&self, config: &Configuration) {
        *self.configured_manager.write().unwrap() = config.rig_executable.is_some();
        *self.rig_executable.write().unwrap() =
            config.rig_executable.clone().or_else(find_rig_executable);
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::Rig]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        let home = env.home.clone()?;
        let owned = self.classification_roots.iter().any(|root| {
            ret_r_utils::executable::is_path_within(&home, root)
                || ret_r_utils::executable::is_path_within(&env.executable, root)
        });
        let configured = *self.configured_manager.read().unwrap()
            && self.install_roots.iter().any(|root| {
                ret_r_utils::executable::is_path_within(&home, root)
                    || ret_r_utils::executable::is_path_within(&env.executable, root)
            });
        if !owned && !configured {
            return None;
        }

        self.installation_from_env(env)
    }

    fn find(&self, reporter: &dyn Reporter) {
        for root in &self.install_roots {
            if !self.classification_roots.contains(root)
                && !*self.configured_manager.read().unwrap()
            {
                continue;
            }
            let Ok(entries) = fs::read_dir(root) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                for executable in find_executables(entry.path()) {
                    if let Some(resolved) = ResolvedRInstallation::from(&executable) {
                        if let Some(installation) = self.try_from(&resolved.to_r_env()) {
                            resolved.add_to_cache(installation.clone());
                            if let Some(manager) = &installation.manager {
                                reporter.report_manager(manager);
                            }
                            reporter.report_installation(&installation);
                        }
                    }
                }
            }
        }
    }
}

fn rig_roots(environment: &dyn Environment) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut roots = vec![
        PathBuf::from("/opt/R"),
        PathBuf::from("/Library/Frameworks/R.framework/Versions"),
        PathBuf::from(r"C:\Program Files\R"),
    ];
    let mut classification_roots = Vec::new();

    if let Some(custom_root) = environment.get_env_var("RIG_R_INSTALL_DIR".to_string()) {
        let custom_root = PathBuf::from(custom_root);
        classification_roots.push(custom_root.clone());
        roots.push(custom_root);
    }
    if let Some(home) = environment.get_user_home() {
        let user_root = if cfg!(windows) {
            environment
                .get_env_var("APPDATA".to_string())
                .map(PathBuf::from)
                .unwrap_or(home)
                .join("rig")
                .join("data")
                .join("r")
        } else {
            home.join(".local").join("share").join("rig").join("r")
        };
        classification_roots.push(user_root.clone());
        roots.push(user_root);
    }

    roots.sort();
    roots.dedup();
    classification_roots.sort();
    classification_roots.dedup();
    (roots, classification_roots)
}

fn find_rig_executable() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join(if cfg!(windows) { "rig.exe" } else { "rig" });
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    [
        PathBuf::from("/usr/local/bin/rig"),
        PathBuf::from("/opt/homebrew/bin/rig"),
        PathBuf::from("/usr/bin/rig"),
        PathBuf::from(r"C:\Program Files\rig\bin\rig.exe"),
    ]
    .into_iter()
    .find(|path| path.exists())
}

#[cfg(test)]
mod tests {
    use super::rig_roots;
    use ret_core::{
        env::REnv, os_environment::Environment, r_installation::RInstallationKind, Locator,
    };
    use std::path::PathBuf;

    struct TestEnvironment;

    fn test_home() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\Users\tester")
        } else {
            PathBuf::from("/home/tester")
        }
    }

    fn test_custom_root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"D:\managed-r")
        } else {
            PathBuf::from("/srv/managed-r")
        }
    }

    fn test_user_root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\Users\tester\AppData\Roaming\rig\data\r")
        } else {
            PathBuf::from("/home/tester/.local/share/rig/r")
        }
    }

    impl Environment for TestEnvironment {
        fn get_user_home(&self) -> Option<PathBuf> {
            Some(test_home())
        }

        fn get_root(&self) -> Option<PathBuf> {
            None
        }

        fn get_env_var(&self, key: String) -> Option<String> {
            match key.as_str() {
                "RIG_R_INSTALL_DIR" => Some(test_custom_root().to_string_lossy().into_owned()),
                "APPDATA" if cfg!(windows) => Some(r"C:\Users\tester\AppData\Roaming".to_string()),
                _ => None,
            }
        }

        fn get_know_global_search_locations(&self) -> Vec<PathBuf> {
            Vec::new()
        }
    }

    #[test]
    fn shared_install_roots_do_not_prove_rig_ownership() {
        let locator = super::Rig::from(&TestEnvironment);
        for executable in [
            "/opt/R/4.4.1/bin/R",
            "/Library/Frameworks/R.framework/Versions/4.4/Resources/bin/R",
        ] {
            let env = REnv::new(PathBuf::from(executable), None, Some("4.4.1".to_string()));
            assert!(locator.try_from(&env).is_none());
        }
    }

    #[test]
    fn rig_roots_contains_expected_paths() {
        let (roots, classification_roots) = rig_roots(&TestEnvironment);
        assert!(roots.contains(&PathBuf::from("/opt/R")));
        assert!(roots.contains(&PathBuf::from("/Library/Frameworks/R.framework/Versions")));
        assert!(roots.contains(&PathBuf::from(r"C:\Program Files\R")));
        assert!(roots.contains(&test_user_root()));
        assert!(roots.contains(&test_custom_root()));
        assert!(classification_roots.contains(&test_custom_root()));
    }

    #[test]
    fn try_from_accepts_custom_rig_root() {
        let locator = super::Rig::from(&TestEnvironment);
        let version_root = test_custom_root().join("4.4.1");
        let env = REnv::new(
            version_root
                .join("bin")
                .join(if cfg!(windows) { "R.exe" } else { "R" }),
            Some(version_root.join("lib").join("R")),
            Some("4.4.1".to_string()),
        );

        assert_eq!(
            locator.try_from(&env).and_then(|value| value.kind),
            Some(RInstallationKind::Rig)
        );
    }

    #[test]
    fn explicitly_configured_rig_can_classify_a_shared_root() {
        let locator = super::Rig::from(&TestEnvironment);
        locator.configure(&ret_core::Configuration {
            rig_executable: Some(PathBuf::from("rig")),
            ..Default::default()
        });
        let env = REnv::new(
            PathBuf::from("/opt/R/4.4.1/bin/R"),
            Some(PathBuf::from("/opt/R/4.4.1/lib/R")),
            Some("4.4.1".to_string()),
        );
        assert_eq!(
            locator.try_from(&env).unwrap().kind,
            Some(RInstallationKind::Rig)
        );
    }

    #[test]
    fn try_from_rejects_non_rig_path() {
        let locator = super::Rig::new();
        let env = REnv::new(
            PathBuf::from("/usr/bin/R"),
            Some(PathBuf::from("/usr/lib/R")),
            Some("4.4.1".to_string()),
        );

        assert!(locator.try_from(&env).is_none());
    }
}
