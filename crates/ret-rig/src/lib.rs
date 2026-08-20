// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ret_core::{
    arch::Architecture,
    env::REnv,
    manager::{EnvManager, EnvManagerType},
    r_installation::{RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Configuration, Locator, LocatorKind, RefreshStatePersistence,
};
use ret_r_utils::env::ResolvedRInstallation;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

pub struct Rig {
    rig_executable: Arc<RwLock<Option<PathBuf>>>,
}

impl Rig {
    pub fn new() -> Rig {
        Rig {
            rig_executable: Arc::new(RwLock::new(find_rig_executable())),
        }
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
        if !looks_like_rig_path(&home) && !looks_like_rig_path(&env.executable) {
            return None;
        }

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

    fn find(&self, reporter: &dyn Reporter) {
        for root in rig_roots() {
            let Ok(entries) = fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let candidates = [
                    entry.path(),
                    entry.path().join("Resources"),
                    entry.path().join("lib").join("R"),
                ];

                for home in candidates {
                    let executable = if cfg!(windows) {
                        home.join("bin").join("x64").join("R.exe")
                    } else {
                        home.join("bin").join("R")
                    };
                    if !looks_like_rig_path(&home) && !looks_like_rig_path(&executable) {
                        continue;
                    }
                    if !executable.exists() {
                        continue;
                    }
                    if let Some(resolved) = ResolvedRInstallation::from(&executable) {
                        let env = resolved.to_r_env();
                        if let Some(installation) = self.try_from(&env) {
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

fn rig_roots() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/opt/R"),
        PathBuf::from("/Library/Frameworks/R.framework/Versions"),
        PathBuf::from(r"C:\Program Files\R"),
    ]
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
    use ret_core::{env::REnv, r_installation::RInstallationKind, Locator};
    use std::path::{Path, PathBuf};

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
        let roots = rig_roots();
        assert!(roots.contains(&PathBuf::from("/opt/R")));
        assert!(roots.contains(&PathBuf::from("/Library/Frameworks/R.framework/Versions")));
        assert!(roots.contains(&PathBuf::from(r"C:\Program Files\R")));
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
