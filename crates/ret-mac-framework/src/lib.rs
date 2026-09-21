// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ret_core::{
    arch::Architecture,
    env::REnv,
    os_environment::{Environment, EnvironmentApi},
    r_installation::{RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Locator, LocatorKind,
};
use ret_fs::path::resolve_symlink;
use ret_r_utils::{
    env::ResolvedRInstallation,
    executable::{filter_symlink_paths, find_executables},
};
use std::{fs, path::PathBuf};

pub struct MacFramework {
    version_roots: Vec<PathBuf>,
}

impl MacFramework {
    pub fn from(environment: &dyn Environment) -> Self {
        Self {
            version_roots: framework_version_roots(environment),
        }
    }

    pub fn new() -> MacFramework {
        Self::from(&EnvironmentApi::new())
    }
}

impl Default for MacFramework {
    fn default() -> Self {
        Self::new()
    }
}

impl Locator for MacFramework {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::MacFramework
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::MacFramework]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        if std::env::consts::OS != "macos" {
            return None;
        }

        let executable = resolve_symlink(&env.executable).unwrap_or(env.executable.clone());
        let home = env.home.clone()?;
        if !is_framework_home(&home) {
            return None;
        }

        let version = env.version.clone().or_else(|| {
            home.parent()
                .and_then(|parent| parent.file_name())
                .map(|name| name.to_string_lossy().to_string())
        });

        let mut extra_executables = find_executables(home.join("bin"));
        let versions = home
            .ancestors()
            .find(|path| path.file_name().is_some_and(|name| name == "Versions"))?;
        let current = versions
            .join("Current")
            .join("Resources")
            .join("bin")
            .join("R");
        if let Some(target) = resolve_symlink(&current) {
            if target == executable {
                extra_executables.push(current);
            }
        }
        let mut symlinks = env.symlinks.clone().unwrap_or_default();
        symlinks.extend(filter_symlink_paths(extra_executables.clone()));
        let mut known_executables = env.known_executables.clone().unwrap_or_default();
        known_executables.extend(extra_executables);

        let arch = env
            .arch
            .clone()
            .unwrap_or_else(|| Architecture::infer_from_path(&executable));

        Some(
            RInstallationBuilder::new(Some(RInstallationKind::MacFramework))
                .display_name(Some("macOS Framework R".to_string()))
                .executable(Some(env.executable.clone()))
                .home(Some(home))
                .version(version)
                .arch(Some(arch))
                .known_executables(Some(known_executables))
                .symlinks(Some(symlinks))
                .build(),
        )
    }

    fn find(&self, reporter: &dyn Reporter) {
        if std::env::consts::OS != "macos" {
            return;
        }

        for versions in &self.version_roots {
            let Ok(reader) = fs::read_dir(versions) else {
                continue;
            };
            for file in reader.filter_map(Result::ok) {
                for executable in find_executables(file.path()) {
                    if let Some(resolved) = ResolvedRInstallation::from(&executable) {
                        if let Some(installation) = self.try_from(&resolved.to_r_env()) {
                            resolved.add_to_cache(installation.clone());
                            reporter.report_installation(&installation);
                        }
                    }
                }
            }
        }
    }
}

fn is_framework_home(home: &std::path::Path) -> bool {
    home.ancestors().any(|path| {
        path.file_name().is_some_and(|name| name == "Versions")
            && path
                .parent()
                .and_then(|path| path.file_name())
                .is_some_and(|name| name == "R.framework")
    }) && !home.to_string_lossy().contains("/Cellar/")
        && !home.starts_with("/opt/local")
}

fn framework_version_roots(environment: &dyn Environment) -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/Library/Frameworks/R.framework/Versions")];
    if let Some(home) = environment.get_user_home() {
        roots.push(home.join("Library/Frameworks/R.framework/Versions"));
    }
    if let Some(home) = environment.get_env_var("R_HOME".to_string()) {
        let home = PathBuf::from(home);
        if let Some(versions) = home.ancestors().find(|path| {
            path.file_name().is_some_and(|name| name == "Versions")
                && path
                    .parent()
                    .and_then(|path| path.file_name())
                    .is_some_and(|name| name == "R.framework")
        }) {
            roots.push(versions.to_path_buf());
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

#[cfg(test)]
mod root_tests {
    use super::*;
    struct Env;
    impl Environment for Env {
        fn get_user_home(&self) -> Option<PathBuf> {
            Some("/users/tester".into())
        }
        fn get_root(&self) -> Option<PathBuf> {
            None
        }
        fn get_env_var(&self, key: String) -> Option<String> {
            (key == "R_HOME").then(|| "/custom/R.framework/Versions/4.4-arm64/Resources".into())
        }
        fn get_know_global_search_locations(&self) -> Vec<PathBuf> {
            Vec::new()
        }
    }
    #[test]
    fn user_and_custom_framework_roots_are_discovered_and_classifiable() {
        let roots = framework_version_roots(&Env);
        assert!(roots.contains(&PathBuf::from(
            "/users/tester/Library/Frameworks/R.framework/Versions"
        )));
        assert!(roots.contains(&PathBuf::from("/custom/R.framework/Versions")));
        assert!(is_framework_home(std::path::Path::new(
            "/custom/R.framework/Versions/4.4-arm64/Resources"
        )));
        assert!(!is_framework_home(std::path::Path::new(
            "/custom/Other.framework/Versions/4.4/Resources"
        )));
    }
}
