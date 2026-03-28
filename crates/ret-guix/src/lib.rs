use ret_core::{
    arch::Architecture,
    cache::LocatorCache,
    env::REnv,
    manager::{EnvManager, EnvManagerType},
    os_environment::Environment,
    r_installation::{RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Locator, LocatorKind,
};
use ret_fs::path::resolve_any_symlink;
use ret_r_utils::{env::ResolvedRInstallation, executable::find_executables};
use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct Guix {
    reported_executables: Arc<LocatorCache<PathBuf, RInstallation>>,
    profile_bins: Vec<PathBuf>,
    manager: Option<EnvManager>,
}

impl Guix {
    pub fn from(environment: &dyn Environment) -> Guix {
        Guix {
            reported_executables: Arc::new(LocatorCache::new()),
            profile_bins: guix_profile_bins(environment),
            manager: find_guix_manager(environment),
        }
    }

    fn insert_and_report(&self, installation: RInstallation, reporter: Option<&dyn Reporter>) {
        let mut entries = vec![];
        if let Some(executable) = installation.executable.clone() {
            entries.push((executable, installation.clone()));
        }
        if let Some(symlinks) = &installation.symlinks {
            for symlink in symlinks {
                entries.push((symlink.clone(), installation.clone()));
            }
        }
        self.reported_executables.insert_many(entries);

        if let Some(reporter) = reporter {
            if let Some(manager) = &installation.manager {
                reporter.report_manager(manager);
            }
            reporter.report_installation(&installation);
        }
    }
}

impl Locator for Guix {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::Guix
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::Guix]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        if cfg!(windows) {
            return None;
        }

        env.version.as_ref()?;

        let resolved_executable =
            resolve_any_symlink(&env.executable).unwrap_or(env.executable.clone());
        let home = env.home.clone()?;
        if !looks_like_guix_path(&resolved_executable)
            && !looks_like_guix_path(&env.executable)
            && !looks_like_guix_path(&home)
        {
            return None;
        }

        let mut symlinks = env.symlinks.clone().unwrap_or_default();
        symlinks.push(env.executable.clone());
        symlinks.push(resolved_executable.clone());
        if let Some(parent) = env.executable.parent() {
            symlinks.extend(find_executables(parent));
        }

        Some(
            RInstallationBuilder::new(Some(RInstallationKind::Guix))
                .display_name(Some("Guix R".to_string()))
                .executable(Some(resolved_executable.clone()))
                .home(Some(home))
                .version(env.version.clone())
                .arch(
                    env.arch
                        .clone()
                        .or_else(|| Some(Architecture::infer_from_path(&resolved_executable))),
                )
                .manager(self.manager.clone())
                .symlinks(Some(symlinks))
                .build(),
        )
    }

    fn find(&self, reporter: &dyn Reporter) {
        if cfg!(windows) {
            return;
        }

        self.reported_executables.clear();
        for bin in &self.profile_bins {
            if !bin.is_dir() {
                continue;
            }

            for executable in find_executables(bin) {
                if self.reported_executables.contains_key(&executable) {
                    continue;
                }

                if let Some(resolved) = ResolvedRInstallation::from(&executable) {
                    let env = resolved.to_r_env();
                    if let Some(installation) = self.try_from(&env) {
                        resolved.add_to_cache(installation.clone());
                        self.insert_and_report(installation, Some(reporter));
                    }
                }
            }
        }
    }
}

fn looks_like_guix_path(path: &Path) -> bool {
    let path = path.to_string_lossy();
    path.starts_with("/gnu/store/")
        || path.contains("/.guix-profile/")
        || path.contains("/.guix-home/profile/")
        || path.starts_with("/var/guix/profiles/")
        || path.starts_with("/run/current-system/profile/")
}

fn guix_profile_bins(environment: &dyn Environment) -> Vec<PathBuf> {
    let mut bins = vec![
        PathBuf::from("/run/current-system/profile/bin"),
        PathBuf::from("/var/guix/profiles/default/bin"),
    ];

    if let Some(home) = environment.get_user_home() {
        bins.push(home.join(".guix-profile").join("bin"));
        bins.push(home.join(".guix-home").join("profile").join("bin"));
    }

    let username = environment
        .get_env_var("USER".to_string())
        .or_else(|| environment.get_env_var("USERNAME".to_string()))
        .or_else(|| env::var("USER").ok())
        .or_else(|| env::var("USERNAME").ok());

    if let Some(username) = username {
        bins.push(
            PathBuf::from("/var/guix/profiles/per-user")
                .join(username)
                .join("bin"),
        );
    }

    // GUIX_PROFILE env var points to the active profile.
    if let Some(guix_profile) = environment
        .get_env_var("GUIX_PROFILE".to_string())
        .or_else(|| env::var("GUIX_PROFILE").ok())
    {
        bins.push(PathBuf::from(guix_profile).join("bin"));
    }

    bins.sort();
    bins.dedup();
    bins
}

fn find_guix_manager(environment: &dyn Environment) -> Option<EnvManager> {
    let mut candidates = vec![
        PathBuf::from("/run/current-system/profile/bin/guix"),
        PathBuf::from("/var/guix/profiles/default/bin/guix"),
        PathBuf::from("/usr/local/bin/guix"),
    ];

    if let Some(home) = environment.get_user_home() {
        candidates.push(home.join(".guix-profile").join("bin").join("guix"));
        candidates.push(
            home.join(".guix-home")
                .join("profile")
                .join("bin")
                .join("guix"),
        );
        candidates.push(
            home.join(".config")
                .join("guix")
                .join("current")
                .join("bin")
                .join("guix"),
        );
    }

    if let Ok(path) = env::var("PATH") {
        for directory in env::split_paths(&path) {
            candidates.push(directory.join("guix"));
        }
    }

    candidates.sort();
    candidates.dedup();
    candidates
        .into_iter()
        .find(|candidate| candidate.exists())
        .map(|path| EnvManager::new(path, EnvManagerType::Guix, None))
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::looks_like_guix_path;
    #[cfg(unix)]
    use ret_core::{
        env::REnv, os_environment::EnvironmentApi, r_installation::RInstallationKind, Locator,
    };
    #[cfg(unix)]
    use std::path::{Path, PathBuf};

    #[cfg(unix)]
    #[test]
    fn recognizes_guix_paths() {
        assert!(looks_like_guix_path(Path::new(
            "/gnu/store/abc123-r-4.4.1/lib/R"
        )));
        assert!(looks_like_guix_path(Path::new(
            "/home/user/.guix-profile/bin/R"
        )));
        assert!(looks_like_guix_path(Path::new(
            "/var/guix/profiles/per-user/alice/bin/R"
        )));
        assert!(looks_like_guix_path(Path::new(
            "/run/current-system/profile/bin/R"
        )));
        assert!(!looks_like_guix_path(Path::new("/usr/local/bin/R")));
        assert!(!looks_like_guix_path(Path::new("/nix/store/hash-R/bin/R")));
    }

    #[cfg(unix)]
    #[test]
    fn try_from_classifies_guix_installation() {
        let locator = super::Guix::from(&EnvironmentApi::new());
        let env = REnv::new(
            PathBuf::from("/home/user/.guix-profile/bin/R"),
            Some(PathBuf::from("/gnu/store/abc123-r-4.4.1/lib/R")),
            Some("4.4.1".to_string()),
        );

        let installation = locator.try_from(&env).expect("expected guix installation");
        assert_eq!(installation.kind, Some(RInstallationKind::Guix));
        assert_eq!(installation.version.as_deref(), Some("4.4.1"));
    }
}
