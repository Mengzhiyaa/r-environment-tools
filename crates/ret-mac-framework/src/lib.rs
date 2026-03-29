// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ret_core::{
    arch::Architecture,
    env::REnv,
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

pub struct MacFramework {}

impl MacFramework {
    pub fn new() -> MacFramework {
        MacFramework {}
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
        let home_str = home.to_string_lossy();
        if !home_str.starts_with("/Library/Frameworks/R.framework/Versions/") {
            return None;
        }

        let version = env.version.clone().or_else(|| {
            home.parent()
                .and_then(|parent| parent.file_name())
                .map(|name| name.to_string_lossy().to_string())
        });

        let mut extra_executables = find_executables(home.join("bin"));
        let current =
            PathBuf::from("/Library/Frameworks/R.framework/Versions/Current/Resources/bin/R");
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
                .executable(Some(executable))
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

        if let Ok(reader) = fs::read_dir("/Library/Frameworks/R.framework/Versions/") {
            for file in reader.filter_map(Result::ok) {
                let version_dir = file.path();
                if version_dir.ends_with("Current") {
                    continue;
                }
                let home = version_dir.join("Resources");
                let executable = home.join("bin").join("R");
                if !executable.exists() {
                    continue;
                }
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
