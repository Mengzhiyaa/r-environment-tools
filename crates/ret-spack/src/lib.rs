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
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct Spack {
    reported_executables: Arc<LocatorCache<PathBuf, RInstallation>>,
    opt_dirs: Vec<PathBuf>,
    manager: Option<EnvManager>,
}

impl Spack {
    pub fn from(environment: &dyn Environment) -> Spack {
        Spack {
            reported_executables: Arc::new(LocatorCache::new()),
            opt_dirs: spack_opt_dirs(environment),
            manager: find_spack_manager(environment),
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

impl Locator for Spack {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::Spack
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::Spack]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        if cfg!(windows) {
            return None;
        }

        env.version.as_ref()?;

        let resolved_executable =
            resolve_any_symlink(&env.executable).unwrap_or(env.executable.clone());
        let home = env.home.clone()?;
        if !looks_like_spack_path(&resolved_executable)
            && !looks_like_spack_path(&env.executable)
            && !looks_like_spack_path(&home)
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
            RInstallationBuilder::new(Some(RInstallationKind::Spack))
                .display_name(Some("Spack R".to_string()))
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
        for opt_dir in &self.opt_dirs {
            if !opt_dir.is_dir() {
                continue;
            }

            // Spack layout: <opt_dir>/<platform>/<compiler>/r-<version>/bin/R
            // We need to walk through platform dirs, then compiler dirs, then r-* dirs.
            let Ok(platform_dirs) = fs::read_dir(opt_dir) else {
                continue;
            };
            for platform_entry in platform_dirs.filter_map(Result::ok) {
                let platform_path = platform_entry.path();
                if !platform_path.is_dir() {
                    continue;
                }
                let Ok(compiler_dirs) = fs::read_dir(&platform_path) else {
                    continue;
                };
                for compiler_entry in compiler_dirs.filter_map(Result::ok) {
                    let compiler_path = compiler_entry.path();
                    if !compiler_path.is_dir() {
                        continue;
                    }
                    let Ok(package_dirs) = fs::read_dir(&compiler_path) else {
                        continue;
                    };
                    for package_entry in package_dirs.filter_map(Result::ok) {
                        let package_name = package_entry.file_name();
                        let name = package_name.to_string_lossy();
                        if !name.starts_with("r-") || name.starts_with("r-lib") {
                            continue;
                        }
                        // Only match the R base package, not R packages like r-ggplot2.
                        // r-<version> but not r-<packagename>-<version>.
                        // The version part starts with a digit.
                        let suffix = &name[2..];
                        if !suffix.starts_with(|c: char| c.is_ascii_digit()) {
                            continue;
                        }

                        let package_path = package_entry.path();
                        for executable in find_executables(&package_path) {
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
        }
    }
}

fn looks_like_spack_path(path: &Path) -> bool {
    let path = path.to_string_lossy();
    path.contains("/spack/opt/spack/") || path.contains("/spack/var/spack/")
}

/// Returns the `opt/spack` directories where Spack installs packages.
fn spack_opt_dirs(environment: &dyn Environment) -> Vec<PathBuf> {
    let mut dirs = vec![];

    // SPACK_ROOT is the canonical env var pointing to the Spack installation.
    if let Some(spack_root) = environment
        .get_env_var("SPACK_ROOT".to_string())
        .or_else(|| env::var("SPACK_ROOT").ok())
    {
        dirs.push(PathBuf::from(spack_root).join("opt").join("spack"));
    }

    // Common default locations.
    if let Some(home) = environment.get_user_home() {
        dirs.push(home.join("spack").join("opt").join("spack"));
        dirs.push(home.join(".spack").join("opt").join("spack"));
    }
    dirs.push(PathBuf::from("/opt/spack/opt/spack"));

    dirs.sort();
    dirs.dedup();
    dirs
}

fn find_spack_manager(environment: &dyn Environment) -> Option<EnvManager> {
    let mut candidates = vec![];

    if let Some(spack_root) = environment
        .get_env_var("SPACK_ROOT".to_string())
        .or_else(|| env::var("SPACK_ROOT").ok())
    {
        candidates.push(PathBuf::from(spack_root).join("bin").join("spack"));
    }

    if let Some(home) = environment.get_user_home() {
        candidates.push(home.join("spack").join("bin").join("spack"));
        candidates.push(home.join(".spack").join("bin").join("spack"));
    }
    candidates.push(PathBuf::from("/opt/spack/bin/spack"));

    // Also check PATH.
    if let Ok(path) = env::var("PATH") {
        for directory in env::split_paths(&path) {
            candidates.push(directory.join("spack"));
        }
    }

    candidates.sort();
    candidates.dedup();
    candidates
        .into_iter()
        .find(|candidate| candidate.exists())
        .map(|path| EnvManager::new(path, EnvManagerType::Spack, None))
}

#[cfg(test)]
mod tests {
    use super::looks_like_spack_path;
    use std::path::Path;

    #[test]
    fn recognizes_spack_paths() {
        assert!(looks_like_spack_path(Path::new(
            "/home/user/spack/opt/spack/linux-ubuntu22.04-x86_64/gcc-12/r-4.4.1-abcdef/lib/R"
        )));
        assert!(looks_like_spack_path(Path::new(
            "/opt/spack/opt/spack/linux-rhel8-zen3/gcc-11/r-4.3.2-xyz123/bin/R"
        )));
        assert!(!looks_like_spack_path(Path::new("/usr/local/bin/R")));
        assert!(!looks_like_spack_path(Path::new("/opt/R/4.4.1/bin/R")));
    }
}
