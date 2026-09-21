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

        let registered = registry_installations().iter().any(|path| {
            paths_match(path, &env.executable)
                || env.home.as_ref().is_some_and(|home| {
                    REnv::new(path.clone(), None, None)
                        .home
                        .as_ref()
                        .is_some_and(|registered_home| paths_match(home, registered_home))
                })
        });
        if !registered {
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

fn paths_match(left: &Path, right: &Path) -> bool {
    let left = std::fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right = std::fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());
    ret_fs::path::norm_case(left) == ret_fs::path::norm_case(right)
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
    let key_paths = [
        r"SOFTWARE\R-core\R",
        r"SOFTWARE\R-core\R32",
        r"SOFTWARE\R-core\R64",
    ];

    for hive in &hives {
        for key_path in &key_paths {
            for view in [KEY_WOW64_32KEY, KEY_WOW64_64KEY] {
                collect_installations_from_key(hive, key_path, view, &mut installations);
            }
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
    view: u32,
    installations: &mut Vec<PathBuf>,
) {
    use winreg::enums::KEY_READ;
    let Ok(root) = hive.open_subkey_with_flags(key_path, KEY_READ | view) else {
        return;
    };
    let mut prefixes = Vec::new();
    if let Ok(path) = root.get_value::<String, _>("InstallPath") {
        prefixes.push(PathBuf::from(path));
    }
    for name in root.enum_keys().flatten() {
        if let Ok(version) = root.open_subkey_with_flags(name, KEY_READ | view) {
            if let Ok(path) = version.get_value::<String, _>("InstallPath") {
                prefixes.push(PathBuf::from(path));
            }
        }
    }
    for prefix in prefixes {
        installations.extend(ret_r_utils::executable::find_executables(prefix));
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

#[cfg(all(test, windows))]
mod registry_tests {
    use super::*;
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    struct Fixture(String);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.0);
        }
    }

    #[test]
    fn both_root_and_version_install_paths_use_shared_executable_layouts() {
        let temp = tempfile::tempdir().unwrap();
        let root_home = temp.path().join("root");
        let version_home = temp.path().join("version");
        let arm = root_home.join("bin/aarch64/R.exe");
        let x86 = version_home.join("bin/i386/R.exe");
        for path in [&arm, &x86] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "runtime").unwrap();
        }
        let fixture = Fixture(format!(
            r"Software\ret-path-tests-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let hive = RegKey::predef(HKEY_CURRENT_USER);
        let (root, _) = hive.create_subkey(&fixture.0).unwrap();
        root.set_value("InstallPath", &root_home.to_string_lossy().as_ref())
            .unwrap();
        let (version, _) = root.create_subkey("4.1.3").unwrap();
        version
            .set_value("InstallPath", &version_home.to_string_lossy().as_ref())
            .unwrap();
        let mut found = Vec::new();
        collect_installations_from_key(&hive, &fixture.0, 0, &mut found);
        assert!(found.contains(&arm));
        assert!(found.contains(&x86));
    }
}
