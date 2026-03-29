use ret_core::{
    arch::Architecture,
    env::REnv,
    manager::{EnvManager, EnvManagerType},
    os_environment::Environment,
    r_installation::{RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Locator, LocatorKind,
};
use ret_r_utils::{env::ResolvedRInstallation, executable::find_executable};
use std::path::{Path, PathBuf};

pub struct Chocolatey {
    install_root: Option<PathBuf>,
    shims_dir: Option<PathBuf>,
    manager: Option<EnvManager>,
}

impl Chocolatey {
    pub fn from(environment: &dyn Environment) -> Chocolatey {
        let install_root = chocolatey_install_root(environment);
        let shims_dir = install_root.as_ref().map(|root| root.join("shims"));
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
        if !looks_like_chocolatey_path(&env.executable) && !looks_like_chocolatey_path(&home) {
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

        // Check the Chocolatey shims directory for R shim executables.
        if let Some(shims_dir) = &self.shims_dir {
            for name in ["R.exe", "Rscript.exe"] {
                let shim = shims_dir.join(name);
                if shim.exists() {
                    if let Some(resolved) = ResolvedRInstallation::from(&shim) {
                        let env = resolved.to_r_env();
                        if let Some(installation) = self.try_from(&env) {
                            resolved.add_to_cache(installation.clone());
                            if let Some(manager) = &installation.manager {
                                reporter.report_manager(manager);
                            }
                            reporter.report_installation(&installation);
                            return; // Both shims point to the same installation.
                        }
                    }
                }
            }
        }

        // Also check the Chocolatey lib directory for the R package.
        if let Some(install_root) = &self.install_root {
            let lib_r = install_root.join("lib").join("R");
            if lib_r.is_dir() {
                // The package directory contains a tools/ subdirectory with the R installation.
                let tools = lib_r.join("tools");
                if tools.is_dir() {
                    if let Some(executable) = find_executable(&tools) {
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
    let shim = root.join("shims").join("choco.exe");
    if shim.exists() {
        return Some(EnvManager::new(shim, EnvManagerType::Chocolatey, None));
    }
    None
}

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
