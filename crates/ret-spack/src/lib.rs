use ret_core::{
    arch::Architecture,
    cache::LocatorCache,
    env::REnv,
    manager::{EnvManager, EnvManagerType},
    os_environment::Environment,
    r_installation::{RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Locator, LocatorKind, RefreshStatePersistence,
};
use ret_fs::path::resolve_any_symlink;
use ret_r_utils::{
    env::ResolvedRInstallation,
    executable::{filter_symlink_paths, find_executables},
};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
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
        let mut entries = installation
            .known_executables
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|executable| (executable, installation.clone()))
            .collect::<Vec<_>>();
        if entries.is_empty() {
            if let Some(executable) = installation.executable.clone() {
                entries.push((executable, installation.clone()));
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

    fn installation_from_env(&self, env: &REnv) -> Option<RInstallation> {
        env.version.as_ref()?;
        let resolved_executable =
            resolve_any_symlink(&env.executable).unwrap_or(env.executable.clone());
        let home = env.home.clone()?;
        let mut extra_executables = vec![env.executable.clone(), resolved_executable.clone()];
        if let Some(parent) = env.executable.parent() {
            extra_executables.extend(find_executables(parent));
        }
        let mut symlinks = env.symlinks.clone().unwrap_or_default();
        symlinks.extend(filter_symlink_paths(extra_executables.clone()));
        let mut known_executables = env.known_executables.clone().unwrap_or_default();
        known_executables.extend(extra_executables);

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
                .known_executables(Some(known_executables))
                .symlinks(Some(symlinks))
                .build(),
        )
    }
}

impl Locator for Spack {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::Spack
    }

    fn refresh_state(&self) -> RefreshStatePersistence {
        RefreshStatePersistence::SelfHydratingCache
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::Spack]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        if cfg!(windows) {
            return None;
        }

        env.version.as_ref()?;

        if let Some(installation) = self.reported_executables.get(&env.executable) {
            return Some(installation);
        }

        let resolved_executable =
            resolve_any_symlink(&env.executable).unwrap_or(env.executable.clone());
        let home = env.home.as_ref()?;
        if !looks_like_spack_path(&resolved_executable)
            && !looks_like_spack_path(&env.executable)
            && !looks_like_spack_path(home)
            && !self.opt_dirs.iter().any(|root| {
                resolved_executable.starts_with(root)
                    || env.executable.starts_with(root)
                    || home.starts_with(root)
            })
        {
            return None;
        }

        self.installation_from_env(env)
    }

    fn find(&self, reporter: &dyn Reporter) {
        if cfg!(windows) {
            return;
        }

        self.reported_executables.clear();
        let mut prefixes = self
            .manager
            .as_ref()
            .map(spack_prefixes_from_manager)
            .unwrap_or_default();
        for opt_dir in &self.opt_dirs {
            prefixes.extend(spack_prefixes_in_tree(opt_dir));
        }
        prefixes.sort();
        prefixes.dedup();

        for prefix in prefixes {
            for executable in find_executables(&prefix) {
                if self.reported_executables.contains_key(&executable) {
                    continue;
                }
                if let Some(resolved) = ResolvedRInstallation::from(&executable) {
                    let env = resolved.to_r_env();
                    // Prefixes returned by Spack are authoritative even when
                    // install_tree uses a custom root or projection.
                    if let Some(installation) = self.installation_from_env(&env) {
                        resolved.add_to_cache(installation.clone());
                        self.insert_and_report(installation, Some(reporter));
                    }
                }
            }
        }
    }
}

fn spack_prefixes_from_manager(manager: &EnvManager) -> Vec<PathBuf> {
    let Ok(output) = Command::new(&manager.executable)
        .args(["find", "--paths", "r"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_spack_find_paths(&String::from_utf8_lossy(&output.stdout))
}

fn parse_spack_find_paths(output: &str) -> Vec<PathBuf> {
    let mut paths = output
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

fn spack_prefixes_in_tree(root: &Path) -> Vec<PathBuf> {
    let mut prefixes = Vec::new();
    let mut directories = vec![(root.to_path_buf(), 0usize)];

    while let Some((directory, depth)) = directories.pop() {
        if !find_executables(&directory).is_empty() {
            prefixes.push(directory);
            continue;
        }
        if depth == 3 {
            continue;
        }
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        directories.extend(entries.filter_map(Result::ok).filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| (entry.path(), depth + 1))
        }));
    }

    prefixes.sort();
    prefixes.dedup();
    prefixes
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
    #[cfg(unix)]
    use super::{looks_like_spack_path, parse_spack_find_paths, spack_prefixes_in_tree};
    #[cfg(unix)]
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[test]
    fn parses_custom_prefixes_reported_by_spack() {
        let paths = parse_spack_find_paths(
            "-- linux-ubuntu22.04-x86_64 / gcc@13 --\nr@4.4.1  abcdefg  /srv/runtimes/R/4.4.1\n",
        );

        assert_eq!(paths, vec![PathBuf::from("/srv/runtimes/R/4.4.1")]);
    }

    #[cfg(unix)]
    #[test]
    fn finds_current_and_legacy_install_tree_layouts() {
        let temp = tempfile::tempdir().expect("failed to create temp directory");
        let current = temp.path().join("linux-x86_64").join("r-4.4.1-hash");
        let legacy = temp
            .path()
            .join("linux-ubuntu-x86_64")
            .join("gcc-13")
            .join("r-4.3.3-hash");
        for prefix in [&current, &legacy] {
            fs::create_dir_all(prefix.join("bin")).expect("failed to create Spack prefix");
            fs::write(prefix.join("bin").join("R"), "").expect("failed to create fake R");
        }

        let prefixes = spack_prefixes_in_tree(temp.path());
        assert!(prefixes.contains(&current));
        assert!(prefixes.contains(&legacy));
    }
}
