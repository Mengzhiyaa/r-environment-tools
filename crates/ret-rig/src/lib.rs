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
use ret_r_utils::{env::ResolvedRInstallation, executable::find_executable};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

pub struct Rig {
    rig_executable: Arc<RwLock<Option<PathBuf>>>,
    install_roots: Vec<PathBuf>,
    classification_roots: Vec<PathBuf>,
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
        *self.rig_executable.write().unwrap() =
            config.rig_executable.clone().or_else(find_rig_executable);
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::Rig]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        let home = env.home.clone()?;
        if !looks_like_rig_path(&home)
            && !looks_like_rig_path(&env.executable)
            && !self
                .classification_roots
                .iter()
                .any(|root| home.starts_with(root) || env.executable.starts_with(root))
        {
            return None;
        }

        self.installation_from_env(env)
    }

    fn find(&self, reporter: &dyn Reporter) {
        for root in &self.install_roots {
            let Ok(entries) = fs::read_dir(root) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let candidates = [
                    entry.path(),
                    entry.path().join("Resources"),
                    entry.path().join("lib").join("R"),
                ];

                for home in candidates {
                    let Some(executable) = find_executable(&home) else {
                        continue;
                    };
                    if let Some(resolved) = ResolvedRInstallation::from(&executable) {
                        let env = resolved.to_r_env();
                        // The directory was enumerated from a rig install root, so
                        // it remains authoritative even when the root is customized.
                        if let Some(installation) = self.installation_from_env(&env) {
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

fn looks_like_rig_path(path: &Path) -> bool {
    let path = path.to_string_lossy();
    path.contains("/opt/R/")
        || path.contains("\\Program Files\\R\\")
        || path.contains("\\rig\\")
        || path.contains("/rig/")
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
    use super::{looks_like_rig_path, rig_roots};
    use ret_core::{
        env::REnv, os_environment::Environment, r_installation::RInstallationKind, Locator,
    };
    use std::path::{Path, PathBuf};

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
    fn recognizes_rig_linux_path() {
        assert!(looks_like_rig_path(Path::new("/opt/R/4.4.1/lib/R")));
        assert!(looks_like_rig_path(Path::new("/opt/R/4.4.1/bin/R")));
    }

    #[test]
    fn recognizes_rig_windows_path() {
        assert!(looks_like_rig_path(Path::new(
            r"C:\Program Files\R\R-4.4.1\bin\x64\R.exe"
        )));
        assert!(looks_like_rig_path(Path::new(
            r"C:\Users\user\rig\R-4.4.1\bin\R.exe"
        )));
    }

    #[test]
    fn rejects_non_rig_paths() {
        assert!(!looks_like_rig_path(Path::new("/usr/bin/R")));
        assert!(!looks_like_rig_path(Path::new("/usr/local/bin/R")));
        assert!(!looks_like_rig_path(Path::new(
            "/opt/homebrew/Cellar/r/4.4.1/bin/R"
        )));
        assert!(!looks_like_rig_path(Path::new("/nix/store/hash-R/bin/R")));
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
    fn try_from_classifies_rig_installation() {
        let locator = super::Rig::new();
        let env = REnv::new(
            PathBuf::from("/opt/R/4.4.1/bin/R"),
            Some(PathBuf::from("/opt/R/4.4.1/lib/R")),
            Some("4.4.1".to_string()),
        );

        let installation = locator.try_from(&env).expect("expected rig installation");
        assert_eq!(installation.kind, Some(RInstallationKind::Rig));
        assert_eq!(installation.version.as_deref(), Some("4.4.1"));
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
