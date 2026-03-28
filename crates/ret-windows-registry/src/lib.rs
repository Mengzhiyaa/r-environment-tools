// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ret_core::{
    arch::Architecture,
    cache::LocatorCache,
    env::REnv,
    r_installation::{RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Locator, LocatorKind,
};
use ret_r_utils::env::ResolvedRInstallation;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct WindowsRegistry {
    reported_executables: Arc<LocatorCache<PathBuf, RInstallation>>,
}

impl WindowsRegistry {
    pub fn new() -> WindowsRegistry {
        WindowsRegistry {
            reported_executables: Arc::new(LocatorCache::new()),
        }
    }
}

impl Default for WindowsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Locator for WindowsRegistry {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::WindowsRegistry
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::WindowsRegistry]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        if !cfg!(windows) {
            return None;
        }

        let home = env.home.clone()?;
        if !looks_like_windows_r_path(&home) {
            return None;
        }

        Some(
            RInstallationBuilder::new(Some(RInstallationKind::WindowsRegistry))
                .display_name(Some("Windows Registry R".to_string()))
                .executable(Some(env.executable.clone()))
                .home(Some(home))
                .version(env.version.clone())
                .arch(
                    env.arch
                        .clone()
                        .or_else(|| Some(Architecture::infer_from_path(&env.executable))),
                )
                .symlinks(env.symlinks.clone())
                .build(),
        )
    }

    fn find(&self, reporter: &dyn Reporter) {
        if !cfg!(windows) {
            return;
        }

        self.reported_executables.clear();
        for executable in registry_installations() {
            if let Some(resolved) = ResolvedRInstallation::from(&executable) {
                let env = resolved.to_r_env();
                if let Some(installation) = self.try_from(&env) {
                    resolved.add_to_cache(installation.clone());
                    if let Some(executable) = installation.executable.clone() {
                        self.reported_executables
                            .insert(executable, installation.clone());
                    }
                    reporter.report_installation(&installation);
                }
            }
        }
    }
}

fn looks_like_windows_r_path(path: &Path) -> bool {
    let path = path.to_string_lossy();
    // Standard admin install: C:\Program Files\R\R-x.x.x
    path.contains(r"\Program Files\R\")
        // User-level install: any path containing \R\R-x.x.x pattern
        || path.contains(r"\R\R-")
}

#[cfg(windows)]
fn registry_installations() -> Vec<PathBuf> {
    use winreg::enums::*;
    use winreg::RegKey;

    let mut installations = vec![];

    // Scan both HKLM (admin installs) and HKCU (user-level installs).
    // R's Windows installer writes to HKCU when run without administrator
    // privileges, so omitting it would hide all user-level installations.
    let hives = [
        RegKey::predef(HKEY_LOCAL_MACHINE),
        RegKey::predef(HKEY_CURRENT_USER),
    ];
    let key_paths = [r"SOFTWARE\R-core\R", r"SOFTWARE\WOW6432Node\R-core\R"];

    for hive in &hives {
        for key_path in &key_paths {
            collect_installations_from_key(hive, key_path, &mut installations);
        }
    }

    installations.sort();
    installations.dedup();
    installations
}

#[cfg(windows)]
fn collect_installations_from_key(
    hive: &winreg::RegKey,
    key_path: &str,
    installations: &mut Vec<PathBuf>,
) {
    let Ok(root) = hive.open_subkey(key_path) else {
        return;
    };
    for subkey_name in root.enum_keys().flatten() {
        let Ok(version_key) = root.open_subkey(&subkey_name) else {
            continue;
        };
        let Ok(install_path) = version_key.get_value::<String, _>("InstallPath") else {
            continue;
        };
        let base = PathBuf::from(install_path);
        for candidate in [
            base.join("bin").join("x64").join("R.exe"),
            base.join("bin").join("R.exe"),
        ] {
            if candidate.exists() {
                installations.push(candidate);
            }
        }
    }
}

#[cfg(not(windows))]
fn registry_installations() -> Vec<PathBuf> {
    vec![]
}
