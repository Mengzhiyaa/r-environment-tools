// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ret_core::{
    arch::Architecture,
    cache::LocatorCache,
    env::REnv,
    r_installation::{RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Locator, LocatorKind, RefreshStatePersistence, RefreshStateSyncScope,
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

    fn installation_from_env(&self, env: &REnv) -> Option<RInstallation> {
        let home = env.home.clone()?;
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
                .known_executables(env.known_executables.clone())
                .symlinks(env.symlinks.clone())
                .build(),
        )
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

    fn refresh_state(&self) -> RefreshStatePersistence {
        RefreshStatePersistence::SyncedDiscoveryState
    }

    fn sync_refresh_state_from(&self, source: &dyn Locator, scope: &RefreshStateSyncScope) {
        let source = source
            .as_any()
            .downcast_ref::<WindowsRegistry>()
            .unwrap_or_else(|| {
                panic!(
                    "attempted to sync WindowsRegistry state from {:?}",
                    source.get_kind()
                )
            });

        match scope {
            RefreshStateSyncScope::Full => {
                self.reported_executables.clear();
                self.reported_executables
                    .insert_many(source.reported_executables.clone_map());
            }
            RefreshStateSyncScope::GlobalFiltered(kind)
                if self.supported_categories().contains(kind) =>
            {
                self.reported_executables
                    .insert_many(source.reported_executables.clone_map());
            }
            RefreshStateSyncScope::GlobalFiltered(_) | RefreshStateSyncScope::Workspace => {}
        }
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::WindowsRegistry]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        if !cfg!(windows) {
            return None;
        }

        if let Some(installation) = self.reported_executables.get(&env.executable) {
            return Some(installation);
        }

        let home = env.home.as_ref()?;
        if !looks_like_windows_r_path(home) {
            return None;
        }

        self.installation_from_env(env)
    }

    fn find(&self, reporter: &dyn Reporter) {
        if !cfg!(windows) {
            return;
        }

        self.reported_executables.clear();
        for executable in registry_installations() {
            if let Some(resolved) = ResolvedRInstallation::from(&executable) {
                let env = resolved.to_r_env();
                // InstallPath is authoritative here. It may point outside the
                // installer's usual Program Files directory.
                if let Some(installation) = self.installation_from_env(&env) {
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

#[cfg(test)]
mod tests {
    use super::WindowsRegistry;
    use ret_core::{env::REnv, r_installation::RInstallationKind};
    use std::path::PathBuf;

    #[test]
    fn registry_entries_accept_custom_install_paths() {
        let locator = WindowsRegistry::new();
        let env = REnv::new(
            PathBuf::from(r"D:\Apps\R-4.4.1\bin\x64\R.exe"),
            Some(PathBuf::from(r"D:\Apps\R-4.4.1")),
            Some("4.4.1".to_string()),
        );

        let installation = locator
            .installation_from_env(&env)
            .expect("registry entry should be authoritative");
        assert_eq!(installation.kind, Some(RInstallationKind::WindowsRegistry));
        assert_eq!(installation.home, env.home);
    }
}
