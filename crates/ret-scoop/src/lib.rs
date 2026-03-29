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
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct Scoop {
    install_roots: Vec<PathBuf>,
    manager: Option<EnvManager>,
}

impl Scoop {
    pub fn from(environment: &dyn Environment) -> Scoop {
        let install_roots = scoop_install_roots(environment);
        let manager = scoop_manager(environment);
        Scoop {
            install_roots,
            manager,
        }
    }
}

impl Locator for Scoop {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::Scoop
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::Scoop]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        if !cfg!(windows) {
            return None;
        }

        env.version.as_ref()?;

        let home = env.home.clone()?;
        if !looks_like_scoop_path(&env.executable) && !looks_like_scoop_path(&home) {
            return None;
        }

        Some(
            RInstallationBuilder::new(Some(RInstallationKind::Scoop))
                .display_name(Some("Scoop R".to_string()))
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

        for root in &self.install_roots {
            let Ok(entries) = fs::read_dir(root) else {
                continue;
            };

            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path
                    .file_name()
                    .map(|name| name == "current")
                    .unwrap_or(false)
                    || path.is_dir()
                {
                    let Some(executable) = find_executable(&path) else {
                        continue;
                    };
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

fn scoop_install_roots(environment: &dyn Environment) -> Vec<PathBuf> {
    let mut roots = vec![];
    if let Some(home) = environment.get_user_home() {
        let apps = home.join("scoop").join("apps");
        roots.push(apps.join("r"));
        roots.push(apps.join("r-base"));
    }
    roots.sort();
    roots.dedup();
    roots
}

fn scoop_manager(environment: &dyn Environment) -> Option<EnvManager> {
    let Some(home) = environment.get_user_home() else {
        return None;
    };

    for candidate in [
        home.join("scoop").join("shims").join("scoop.cmd"),
        home.join("scoop").join("shims").join("scoop.ps1"),
        home.join("scoop").join("shims").join("scoop"),
    ] {
        if candidate.exists() {
            return Some(EnvManager::new(candidate, EnvManagerType::Scoop, None));
        }
    }

    None
}

fn looks_like_scoop_path(path: &Path) -> bool {
    let path = path.to_string_lossy().to_ascii_lowercase();
    path.contains("\\scoop\\apps\\r\\")
        || path.contains("\\scoop\\apps\\r-base\\")
        || path.contains("\\scoop\\shims\\r")
}

#[cfg(test)]
mod tests {
    use super::looks_like_scoop_path;
    use std::path::Path;

    #[test]
    fn recognizes_scoop_paths() {
        assert!(looks_like_scoop_path(Path::new(
            r"C:\Users\me\scoop\apps\r\current\bin\R.exe"
        )));
        assert!(looks_like_scoop_path(Path::new(
            r"C:\Users\me\scoop\apps\r-base\4.4.1\bin\R.exe"
        )));
        assert!(!looks_like_scoop_path(Path::new(
            r"C:\Program Files\R\R-4.4.1\bin\R.exe"
        )));
    }
}
