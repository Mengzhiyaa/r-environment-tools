use ret_core::{
    arch::Architecture,
    env::REnv,
    manager::{EnvManager, EnvManagerType},
    os_environment::Environment,
    r_installation::{RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Locator, LocatorKind,
};
use ret_r_utils::{env::ResolvedRInstallation, executable::find_executables};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct Chocolatey {
    install_root: Option<PathBuf>,
    shims_dir: Option<PathBuf>,
    manager: Option<EnvManager>,
}

impl Chocolatey {
    pub fn from(environment: &dyn Environment) -> Chocolatey {
        let install_root = chocolatey_install_root(environment);
        let shims_dir = install_root.as_ref().map(|root| root.join("bin"));
        let manager = find_choco_manager(&install_root);
        Chocolatey {
            install_root,
            shims_dir,
            manager,
        }
    }
}

impl Locator for Chocolatey {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::Chocolatey
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::Chocolatey]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        if !cfg!(windows) {
            return None;
        }

        env.version.as_ref()?;

        let home = env.home.clone()?;
        let root = self.install_root.as_ref()?;
        let owned = ret_r_utils::executable::is_path_within(&env.executable, root)
            || ret_r_utils::executable::is_path_within(&home, root)
            || env.known_executables.as_ref().is_some_and(|paths| {
                paths
                    .iter()
                    .any(|path| ret_r_utils::executable::is_path_within(path, root))
            });
        if !owned {
            return None;
        }

        Some(
            RInstallationBuilder::new(Some(RInstallationKind::Chocolatey))
                .display_name(Some("Chocolatey R".to_string()))
                .executable(Some(env.executable.clone()))
                .home(Some(home))
                .version(env.version.clone())
                .arch(
                    env.arch
                        .clone()
                        .or_else(|| Some(Architecture::infer_from_path(&env.executable))),
                )
                .manager(self.manager.clone())
                .known_executables(env.known_executables.clone())
                .symlinks(env.symlinks.clone())
                .build(),
        )
    }

    fn find(&self, reporter: &dyn Reporter) {
        if !cfg!(windows) {
            return;
        }

        let Some(root) = &self.install_root else {
            return;
        };
        let mut candidates = chocolatey_r_search_roots(root);
        if let Some(shims) = &self.shims_dir {
            candidates.push(shims.clone());
        }
        for directory in candidates {
            for executable in find_executables(directory) {
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

fn chocolatey_r_search_roots(root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![root.join("bin"), root.join("shims")];
    if let Ok(packages) = fs::read_dir(root.join("lib")) {
        for package in packages.filter_map(Result::ok) {
            let name = package.file_name().to_string_lossy().to_ascii_lowercase();
            if !["r", "r.project", "r.portable", "r-project"].contains(&name.as_str()) {
                continue;
            }
            let tools = package.path().join("tools");
            roots.push(tools.clone());
            if let Ok(versions) = fs::read_dir(tools) {
                roots.extend(
                    versions
                        .filter_map(Result::ok)
                        .map(|entry| entry.path())
                        .filter(|path| path.is_dir()),
                );
            }
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

fn chocolatey_install_root(environment: &dyn Environment) -> Option<PathBuf> {
    // ChocolateyInstall env var is the canonical way to find the Chocolatey root.
    if let Some(choco_install) = environment.get_env_var("ChocolateyInstall".to_string()) {
        let path = PathBuf::from(choco_install);
        if path.is_dir() {
            return Some(path);
        }
    }

    // Fall back to well-known default.
    let default = PathBuf::from(r"C:\ProgramData\chocolatey");
    if default.is_dir() {
        return Some(default);
    }

    None
}

fn find_choco_manager(install_root: &Option<PathBuf>) -> Option<EnvManager> {
    let root = install_root.as_ref()?;
    let choco_exe = root.join("bin").join("choco.exe");
    if choco_exe.exists() {
        return Some(EnvManager::new(choco_exe, EnvManagerType::Chocolatey, None));
    }
    // Also check in shims directory.
    let shim = root.join("bin").join("choco.exe");
    if shim.exists() {
        return Some(EnvManager::new(shim, EnvManagerType::Chocolatey, None));
    }
    None
}

#[cfg(test)]
fn looks_like_chocolatey_path(path: &Path) -> bool {
    let path = path.to_string_lossy().to_ascii_lowercase();
    path.contains("\\chocolatey\\")
        || path.contains("\\chocolatey\\shims\\")
        || path.contains("\\chocolatey\\lib\\")
}

// WinGet installs R via the standard CRAN installer, which writes to the
// Windows Registry under `HKLM\SOFTWARE\R-core\R` or `HKCU\SOFTWARE\R-core\R`.
// Therefore WinGet installations are already detected by `ret-windows-registry`
// and do not need a separate locator.

#[cfg(test)]
mod tests {
    use super::looks_like_chocolatey_path;
    use std::path::Path;

    #[test]
    fn recognizes_chocolatey_paths() {
        assert!(looks_like_chocolatey_path(Path::new(
            r"C:\ProgramData\chocolatey\shims\R.exe"
        )));
        assert!(looks_like_chocolatey_path(Path::new(
            r"C:\ProgramData\chocolatey\lib\R\tools\R-4.4.1\bin\R.exe"
        )));
        assert!(!looks_like_chocolatey_path(Path::new(
            r"C:\Program Files\R\R-4.4.1\bin\R.exe"
        )));
    }
}

#[cfg(test)]
mod root_tests {
    use super::*;
    #[test]
    fn standard_bin_and_package_version_layouts_are_scanned() {
        let temp = tempfile::tempdir().unwrap();
        let version = temp.path().join("lib/r.project/tools/R-4.4");
        fs::create_dir_all(version.join("bin")).unwrap();
        let roots = chocolatey_r_search_roots(temp.path());
        assert!(roots.contains(&temp.path().join("bin")));
        assert!(roots.contains(&version));
    }

    #[cfg(windows)]
    #[test]
    fn custom_root_shim_retains_chocolatey_ownership() {
        let temp = tempfile::tempdir().unwrap();
        let locator = Chocolatey {
            install_root: Some(temp.path().to_path_buf()),
            shims_dir: None,
            manager: None,
        };
        let raw = REnv::new(
            temp.path().join("bin/R.exe"),
            Some(PathBuf::from(r"C:\Program Files\R\R-4.4")),
            Some("4.4.0".into()),
        );
        assert_eq!(
            locator.try_from(&raw).unwrap().kind,
            Some(RInstallationKind::Chocolatey)
        );
        let unrelated = REnv::new(
            PathBuf::from(r"C:\unrelated\bin\R.exe"),
            raw.home.clone(),
            raw.version.clone(),
        );
        assert!(locator.try_from(&unrelated).is_none());
    }
}
