// Windows R "Headquarters" — directory scanning for R installations in
// standard filesystem locations that may not appear in the registry.
//
// Mirrors Positron's `rHeadquarters()` Windows branch, which scans:
//   %PROGRAMFILES%\R
//   %ProgramW6432%\R
//   %LOCALAPPDATA%\Programs\R
// And on ARM64 Windows also:
//   %PROGRAMFILES%\R-aarch64
//   %LOCALAPPDATA%\Programs\R-aarch64

use ret_core::{
    arch::Architecture,
    cache::LocatorCache,
    env::REnv,
    os_environment::Environment,
    r_installation::{RInstallation, RInstallationBuilder, RInstallationKind},
    reporter::Reporter,
    Locator, LocatorKind,
};
use ret_r_utils::{env::ResolvedRInstallation, executable::find_executable};
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct WindowsHq {
    hq_dirs: Vec<PathBuf>,
    reported: Arc<LocatorCache<PathBuf, RInstallation>>,
}

impl WindowsHq {
    pub fn from(environment: &dyn Environment) -> WindowsHq {
        WindowsHq {
            hq_dirs: windows_hq_dirs(environment),
            reported: Arc::new(LocatorCache::new()),
        }
    }
}

impl Locator for WindowsHq {
    fn get_kind(&self) -> LocatorKind {
        LocatorKind::WindowsHq
    }

    fn supported_categories(&self) -> Vec<RInstallationKind> {
        vec![RInstallationKind::WindowsHq]
    }

    fn try_from(&self, env: &REnv) -> Option<RInstallation> {
        if !cfg!(windows) {
            return None;
        }
        env.version.as_ref()?;

        let home = env.home.clone()?;
        if !is_windows_hq_path(&env.executable, &self.hq_dirs)
            && !is_windows_hq_path(&home, &self.hq_dirs)
        {
            return None;
        }

        Some(
            RInstallationBuilder::new(Some(RInstallationKind::WindowsHq))
                .display_name(Some("System R".to_string()))
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

    fn find(&self, reporter: &dyn Reporter) {
        if !cfg!(windows) {
            return;
        }

        self.reported.clear();

        for hq_dir in &self.hq_dirs {
            if !hq_dir.is_dir() {
                continue;
            }

            // Enumerate version subdirectories: <hq>/R-4.3.2/, <hq>/4.3.2/, etc.
            let Ok(entries) = fs::read_dir(hq_dir) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let version_dir = entry.path();
                if !version_dir.is_dir() {
                    continue;
                }
                // Skip rig's 'bin/' directory (contains .bat shims, not real R).
                if version_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().eq_ignore_ascii_case("bin"))
                    .unwrap_or(false)
                {
                    continue;
                }
                // Skip 'Current' / 'current' symlinks (handled separately).
                if version_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().eq_ignore_ascii_case("current"))
                    .unwrap_or(false)
                {
                    continue;
                }

                if let Some(executable) = find_executable(&version_dir) {
                    if self.reported.contains_key(&executable) {
                        continue;
                    }
                    if let Some(resolved) = ResolvedRInstallation::from(&executable) {
                        let env = resolved.to_r_env();
                        if let Some(installation) = self.try_from(&env) {
                            resolved.add_to_cache(installation.clone());
                            if let Some(exe) = installation.executable.clone() {
                                self.reported.insert(exe, installation.clone());
                            }
                            reporter.report_installation(&installation);
                        }
                    }
                }
            }
        }
    }
}

/// Returns the R "headquarters" directories for Windows.
///
/// These are directories where the CRAN installer, rig, and other tools
/// place R version directories (e.g. R-4.3.2/).
fn windows_hq_dirs(environment: &dyn Environment) -> Vec<PathBuf> {
    if !cfg!(windows) {
        return vec![];
    }

    let mut dirs = Vec::new();

    // %PROGRAMFILES%\R  (usually C:\Program Files\R)
    let program_dirs: Vec<String> = [
        environment.get_env_var("PROGRAMFILES".to_string()),
        environment.get_env_var("ProgramFiles".to_string()),
        environment.get_env_var("ProgramW6432".to_string()),
    ]
    .into_iter()
    .flatten()
    .collect();

    let mut seen = std::collections::HashSet::new();
    for base in &program_dirs {
        if seen.insert(base.clone()) {
            dirs.push(PathBuf::from(base).join("R"));
            // ARM64 Windows: rig installs R in R-aarch64/
            if is_arm64() {
                dirs.push(PathBuf::from(base).join("R-aarch64"));
            }
        }
    }

    // Fallback if no env vars provided a location.
    if dirs.is_empty() {
        dirs.push(PathBuf::from(r"C:\Program Files\R"));
        if is_arm64() {
            dirs.push(PathBuf::from(r"C:\Program Files\R-aarch64"));
        }
    }

    // %LOCALAPPDATA%\Programs\R  (user-level installs without admin)
    if let Some(local_app_data) = environment
        .get_env_var("LOCALAPPDATA".to_string())
        .or_else(|| env::var("LOCALAPPDATA").ok())
    {
        dirs.push(PathBuf::from(&local_app_data).join("Programs").join("R"));
        if is_arm64() {
            dirs.push(
                PathBuf::from(&local_app_data)
                    .join("Programs")
                    .join("R-aarch64"),
            );
        }
    }

    dirs.sort();
    dirs.dedup();
    dirs
}

fn is_arm64() -> bool {
    env::consts::ARCH == "aarch64"
}

fn is_windows_hq_path(path: &Path, hq_dirs: &[PathBuf]) -> bool {
    hq_dirs.iter().any(|hq| path.starts_with(hq))
}

#[cfg(test)]
#[cfg(windows)]
mod tests {
    use super::is_windows_hq_path;
    use std::path::{Path, PathBuf};

    #[test]
    fn recognizes_hq_paths() {
        let hq_dirs = vec![
            PathBuf::from(r"C:\Program Files\R"),
            PathBuf::from(r"C:\Users\me\AppData\Local\Programs\R"),
        ];

        assert!(is_windows_hq_path(
            Path::new(r"C:\Program Files\R\R-4.4.1\bin\R.exe"),
            &hq_dirs
        ));
        assert!(is_windows_hq_path(
            Path::new(r"C:\Users\me\AppData\Local\Programs\R\R-4.4.1\bin\R.exe"),
            &hq_dirs
        ));
        assert!(!is_windows_hq_path(
            Path::new(r"C:\ProgramData\chocolatey\shims\R.exe"),
            &hq_dirs
        ));
    }
}
