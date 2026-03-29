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
use ret_r_utils::{env::ResolvedRInstallation, executable::find_executables};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};

pub struct LinuxGlobalR {
    reported_executables: Arc<LocatorCache<PathBuf, RInstallation>>,
}

impl LinuxGlobalR {
    pub fn new() -> LinuxGlobalR {
        LinuxGlobalR {
            reported_executables: Arc::new(LocatorCache::new()),
        }
    }

    fn find_cached(&self, reporter: Option<&dyn Reporter>) {
        if std::env::consts::OS == "macos" || std::env::consts::OS == "windows" {
            return;
        }

        // Standard bin directories for system-installed R.
        let mut scan_dirs: Vec<PathBuf> = vec![
            PathBuf::from("/bin"),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/opt/bin"),
        ];

        // Server library directories (from RStudio's RVersionsPosix.cpp and
        // Positron's discoverServerBinaries). Each of these may contain
        // per-version subdirectories, e.g. /usr/lib/R/R-4.3.0/.
        let server_hq_dirs = [
            "/usr/lib/R",
            "/usr/lib64/R",
            "/usr/local/lib/R",
            "/usr/local/lib64/R",
            "/opt/local/lib/R",
            "/opt/local/lib64/R",
            "/opt/local/R",
            "/opt/R",
        ];
        for hq in &server_hq_dirs {
            let hq_path = Path::new(hq);
            if !hq_path.is_dir() {
                continue;
            }
            // Add the HQ directory's own bin/ (for single-R installs like /usr/lib/R/bin/R).
            scan_dirs.push(hq_path.join("bin"));

            // Enumerate version subdirectories like /opt/R/4.3.0/bin/.
            if let Ok(entries) = fs::read_dir(hq_path) {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if path.is_dir() {
                        // Standard Unix layout: <version>/bin/R
                        scan_dirs.push(path.join("bin"));
                        // Some installs put R in <version>/lib/R/bin/R
                        scan_dirs.push(path.join("lib").join("R").join("bin"));
                    }
                }
            }
        }

        // Canonicalize and deduplicate.
        let bin_dirs: HashSet<PathBuf> = scan_dirs
            .into_iter()
            .map(|p| fs::canonicalize(&p).unwrap_or(p))
            .collect();

        thread::scope(|scope| {
            for bin in bin_dirs {
                scope.spawn(move || {
                    find_and_report_global_r_in(&bin, reporter, &self.reported_executables);
                });
            }
        });
    }
}

impl Default for LinuxGlobalR {
    fn default() -> Self {
        Self::new()
    }
}

impl Locator for LinuxGlobalR {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::LinuxGlobal
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::LinuxGlobal]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        if std::env::consts::OS == "macos" || std::env::consts::OS == "windows" {
            return None;
        }

        env.version.as_ref()?;
        let executable = env.executable.clone();
        if !is_global_bin(executable.parent()?) || looks_like_homebrew_path(&executable) {
            return None;
        }

        self.find_cached(None);
        self.reported_executables.get(&executable).or_else(|| {
            let arch = env
                .arch
                .clone()
                .unwrap_or_else(|| Architecture::infer_from_path(&executable));
            Some(
                RInstallationBuilder::new(Some(RInstallationKind::LinuxGlobal))
                    .display_name(Some("System R".to_string()))
                    .executable(Some(executable))
                    .home(env.home.clone())
                    .version(env.version.clone())
                    .arch(Some(arch))
                    .known_executables(env.known_executables.clone())
                    .symlinks(env.symlinks.clone())
                    .build(),
            )
        })
    }

    fn find(&self, reporter: &dyn Reporter) {
        if std::env::consts::OS == "macos" || std::env::consts::OS == "windows" {
            return;
        }
        self.reported_executables.clear();
        self.find_cached(Some(reporter))
    }
}

fn find_and_report_global_r_in(
    bin: &Path,
    reporter: Option<&dyn Reporter>,
    reported_executables: &Arc<LocatorCache<PathBuf, RInstallation>>,
) {
    for executable in find_executables(bin) {
        if reported_executables.contains_key(&executable) || looks_like_homebrew_path(&executable) {
            continue;
        }

        if let Some(resolved) = ResolvedRInstallation::from(&executable) {
            let env = resolved.to_r_env();
            let installation = RInstallationBuilder::new(Some(RInstallationKind::LinuxGlobal))
                .display_name(Some("System R".to_string()))
                .executable(Some(env.executable.clone()))
                .home(env.home.clone())
                .version(env.version.clone())
                .arch(Some(resolved.arch.clone()))
                .known_executables(env.known_executables.clone())
                .symlinks(env.symlinks.clone())
                .build();
            resolved.add_to_cache(installation.clone());

            let mut entries = installation
                .known_executables
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|executable| (executable, installation.clone()))
                .collect::<Vec<_>>();
            if entries.is_empty() {
                entries.push((env.executable.clone(), installation.clone()));
            }
            reported_executables.insert_many(entries);

            if let Some(reporter) = reporter {
                reporter.report_installation(&installation);
            }
        }
    }
}

fn is_global_bin(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    // Standard system bin directories
    path == Path::new("/bin")
        || path == Path::new("/usr/bin")
        || path == Path::new("/usr/local/bin")
        || path == Path::new("/opt/bin")
        // Server library directories and their version subdirectories
        || path_str.starts_with("/usr/lib/R")
        || path_str.starts_with("/usr/lib64/R")
        || path_str.starts_with("/usr/local/lib/R")
        || path_str.starts_with("/usr/local/lib64/R")
        || path_str.starts_with("/opt/local/lib/R")
        || path_str.starts_with("/opt/local/lib64/R")
        || path_str.starts_with("/opt/local/R")
        || path_str.starts_with("/opt/R")
}

fn looks_like_homebrew_path(path: &Path) -> bool {
    ret_core::homebrew_utils::looks_like_homebrew_path(path)
}

#[cfg(test)]
mod tests {
    use super::is_global_bin;
    use std::path::Path;

    #[test]
    fn recognizes_standard_bin_dirs() {
        assert!(is_global_bin(Path::new("/bin")));
        assert!(is_global_bin(Path::new("/usr/bin")));
        assert!(is_global_bin(Path::new("/usr/local/bin")));
        assert!(is_global_bin(Path::new("/opt/bin")));
    }

    #[test]
    fn recognizes_server_library_dirs() {
        assert!(is_global_bin(Path::new("/usr/lib/R/bin")));
        assert!(is_global_bin(Path::new("/usr/lib64/R/bin")));
        assert!(is_global_bin(Path::new("/usr/local/lib/R/bin")));
        assert!(is_global_bin(Path::new("/opt/R/4.4.1/bin")));
        assert!(is_global_bin(Path::new("/opt/local/R/4.3.2/bin")));
        assert!(is_global_bin(Path::new(
            "/opt/local/lib/R/R-4.3.0/lib/R/bin"
        )));
    }

    #[test]
    fn rejects_non_global_bins() {
        assert!(!is_global_bin(Path::new("/home/user/bin")));
        assert!(!is_global_bin(Path::new("/tmp/R/bin")));
        assert!(!is_global_bin(Path::new(
            "/opt/homebrew/Cellar/r/4.4.1/bin"
        )));
        assert!(!is_global_bin(Path::new("/nix/store/hash/bin")));
    }

    #[test]
    fn homebrew_exclusion_works() {
        use super::looks_like_homebrew_path;
        assert!(looks_like_homebrew_path(Path::new(
            "/opt/homebrew/Cellar/r/4.4.1/lib/R/bin/R"
        )));
        assert!(!looks_like_homebrew_path(Path::new("/usr/bin/R")));
    }
}
